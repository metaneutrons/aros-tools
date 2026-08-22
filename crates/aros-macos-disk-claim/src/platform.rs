//! Audited macOS Disk Arbitration boundary.
//!
//! All callbacks are delivered on the dedicated worker's Core Foundation run
//! loop during normal operation. Callback state nevertheless uses interior
//! synchronization, and the C callback owns a separate raw `Arc` strong
//! reference. Therefore an exceptional late callback can at worst retain that
//! allocation; it cannot dereference worker-owned memory after it was freed.

use std::ffi::{c_void, CStr, CString};
use std::ptr::{self, NonNull};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use objc2_core_foundation::{kCFRunLoopDefaultMode, CFRetained, CFRunLoop, CFRunLoopMode};
use objc2_disk_arbitration::{
    kDADiskClaimOptionDefault, kDADiskDescriptionVolumePathKey, kDAReturnBusy, DADisk, DADissenter,
    DASession,
};

use crate::{BsdDiskName, ClaimError};

const RUN_LOOP_SLICE_SECONDS: f64 = 0.010;
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct ClaimHandle {
    name: BsdDiskName,
    commands: Sender<WorkerCommand>,
    events: Receiver<WorkerEvent>,
    worker: Option<JoinHandle<()>>,
}

impl ClaimHandle {
    pub fn acquire(name: BsdDiskName, timeout: Duration) -> Result<Self, ClaimError> {
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let worker_name = name.clone();
        let worker = thread::Builder::new()
            .name(format!("aros-da-claim-{name}"))
            .spawn(move || worker_main(&worker_name, &command_receiver, &event_sender))
            .map_err(ClaimError::WorkerSpawn)?;

        match events.recv_timeout(timeout) {
            Ok(WorkerEvent::Acquired) => Ok(Self {
                name,
                commands,
                events,
                worker: Some(worker),
            }),
            Ok(WorkerEvent::Failed(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Ok(WorkerEvent::Released) | Err(RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                Err(ClaimError::WorkerStopped(name))
            }
            Err(RecvTimeoutError::Timeout) => {
                // The caller's wait is bounded, but callback memory must not be
                // reclaimed just because that deadline elapsed. Ownership
                // stays entirely in the worker. We request cleanup and detach
                // only if even the separate cleanup deadline is exceeded.
                let _ = commands.send(WorkerCommand::Release);
                if wait_for_release(&events, RELEASE_TIMEOUT).is_ok() {
                    let _ = worker.join();
                } else {
                    // A detached worker still owns all DA objects and callback
                    // memory until the pending completion eventually arrives.
                    drop(worker);
                }
                Err(ClaimError::ClaimTimedOut {
                    disk: name,
                    timeout,
                })
            }
        }
    }

    pub fn release(&mut self) -> Result<(), ClaimError> {
        if self.worker.is_none() {
            return Ok(());
        }

        if self.commands.send(WorkerCommand::Release).is_err() {
            self.worker.take();
            return Err(ClaimError::WorkerStopped(self.name.clone()));
        }

        if wait_for_release(&self.events, RELEASE_TIMEOUT).is_ok() {
            if let Some(worker) = self.worker.take() {
                worker
                    .join()
                    .map_err(|_| ClaimError::WorkerStopped(self.name.clone()))?;
            }
            Ok(())
        } else {
            // Dropping a JoinHandle detaches rather than terminating the
            // thread. The worker continues to own every pointer Disk
            // Arbitration may still use, including after a late success.
            self.worker.take();
            Err(ClaimError::ReleaseTimedOut(self.name.clone()))
        }
    }
}

impl Drop for ClaimHandle {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn wait_for_release(events: &Receiver<WorkerEvent>, timeout: Duration) -> Result<(), ()> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(());
        }
        match events.recv_timeout(remaining) {
            Ok(WorkerEvent::Released) => return Ok(()),
            Ok(WorkerEvent::Acquired | WorkerEvent::Failed(_)) => {}
            Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => return Err(()),
        }
    }
}

#[derive(Clone, Copy)]
enum WorkerCommand {
    Release,
}

enum WorkerEvent {
    Acquired,
    Failed(ClaimError),
    Released,
}

fn worker_main(
    name: &BsdDiskName,
    commands: &Receiver<WorkerCommand>,
    events: &Sender<WorkerEvent>,
) {
    let result = run_claim_worker(name, commands, events);
    match result {
        Ok(()) => {
            let _ = events.send(WorkerEvent::Released);
        }
        Err(error) => {
            let _ = events.send(WorkerEvent::Failed(error));
        }
    }
}

fn run_claim_worker(
    name: &BsdDiskName,
    commands: &Receiver<WorkerCommand>,
    events: &Sender<WorkerEvent>,
) -> Result<(), ClaimError> {
    // Schedule first: even DADisk creation and description calls may contact
    // Disk Arbitration. ScheduledSession's Drop unschedules all early-return
    // and unwind paths on this same worker/run-loop pair.
    let session = ScheduledSession::create()?;
    let disk = create_exact_whole_disk(session.session(), name)?;
    verify_unmounted(&disk, name)?;

    let mut operation = ClaimOperation::new(session, disk);
    operation.issue_claim();

    let mut cancelled = false;
    loop {
        cancelled |= release_requested(commands);

        operation.pump_once();
        match operation.take_completion() {
            ClaimCompletion::Pending => {}
            ClaimCompletion::Rejected(status) => {
                if cancelled {
                    return Ok(());
                }
                return Err(ClaimError::ClaimRejected {
                    disk: name.clone(),
                    status,
                });
            }
            ClaimCompletion::Acquired => {
                // A successful completion callback is the ownership proof.
                // `DADiskIsClaimed` only reports global state and cannot prove
                // that this operation owns the claim, so it is not consulted.
                operation.mark_acquired();
                // A caller that timed out cannot own a late claim. Keep the
                // context and session alive until this completion arrives,
                // then return so ClaimOperation immediately unclaims it.
                if cancelled {
                    return Ok(());
                }
                if events.send(WorkerEvent::Acquired).is_err() {
                    return Ok(());
                }
                break;
            }
        }
    }

    loop {
        if release_requested(commands) {
            return Ok(());
        }
        operation.pump_once();
    }
}

fn release_requested(commands: &Receiver<WorkerCommand>) -> bool {
    matches!(
        commands.try_recv(),
        Ok(WorkerCommand::Release) | Err(TryRecvError::Disconnected)
    )
}

fn create_session() -> Result<CFRetained<DASession>, ClaimError> {
    // SAFETY: `None` requests Core Foundation's default allocator. The
    // returned create-rule reference is immediately owned by `CFRetained`.
    unsafe { DASession::new(None) }.ok_or(ClaimError::ObjectUnavailable {
        object: "a DASession",
    })
}

fn create_exact_whole_disk(
    session: &DASession,
    requested: &BsdDiskName,
) -> Result<CFRetained<DADisk>, ClaimError> {
    let c_name = CString::new(requested.as_str()).map_err(|_| ClaimError::InvalidDiskName {
        value: requested.to_string(),
        reason: "the BSD name contains an interior NUL byte",
    })?;
    let name_pointer =
        NonNull::new(c_name.as_ptr().cast_mut()).ok_or(ClaimError::ObjectUnavailable {
            object: "the BSD-name C string",
        })?;

    // SAFETY: `name_pointer` points into `c_name`, which remains alive for the
    // duration of this call. Disk Arbitration copies/uses the name only while
    // constructing the returned create-rule `DADisk` reference.
    let disk = unsafe { DADisk::from_bsd_name(None, session, name_pointer) }
        .ok_or_else(|| ClaimError::DiskNotFound(requested.clone()))?;

    // SAFETY: `disk` is a retained, live Disk Arbitration object. The returned
    // create-rule object is independently owned by `CFRetained`.
    let whole =
        unsafe { disk.whole_disk() }.ok_or_else(|| ClaimError::DiskNotFound(requested.clone()))?;
    let actual = read_bsd_name(&whole)?;
    if actual != requested.as_str() {
        return Err(ClaimError::NotExactWholeDisk {
            requested: requested.clone(),
            actual,
        });
    }

    Ok(whole)
}

fn read_bsd_name(disk: &DADisk) -> Result<String, ClaimError> {
    // SAFETY: `disk` is retained by the caller for this entire operation.
    // Disk Arbitration owns the NUL-terminated name returned by
    // `DADiskGetBSDName`; the pointer remains valid while `disk` is alive and
    // is copied into a Rust `String` before returning.
    let pointer = unsafe { disk.bsd_name() };
    if pointer.is_null() {
        return Err(ClaimError::ObjectUnavailable {
            object: "the DADisk BSD name",
        });
    }
    // SAFETY: The non-null pointer came directly from `DADiskGetBSDName`,
    // whose contract guarantees a NUL-terminated C string.
    Ok(unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned())
}

fn verify_unmounted(disk: &DADisk, name: &BsdDiskName) -> Result<(), ClaimError> {
    // SAFETY: `disk` is retained for the call; the create-rule dictionary is
    // returned in a `CFRetained` owner.
    let description = unsafe { disk.description() }.ok_or(ClaimError::ObjectUnavailable {
        object: "the DADisk description",
    })?;

    // SAFETY: This is an immutable Core Foundation dictionary created by Disk
    // Arbitration. The exported key is a valid process-lifetime CFString, and
    // we use the returned pointer only to test presence. This is an additional
    // check for a mount path on the exact whole-disk object, not proof that all
    // descendant partitions are unmounted; the caller must verify the complete
    // descendant topology while holding the claim. This crate never unmounts.
    let volume_path = unsafe {
        let key = kDADiskDescriptionVolumePathKey;
        description.value(ptr::from_ref(key).cast::<c_void>())
    };
    if volume_path.is_null() {
        Ok(())
    } else {
        Err(ClaimError::DiskMounted(name.clone()))
    }
}

fn default_run_loop_mode() -> Result<&'static CFRunLoopMode, ClaimError> {
    // SAFETY: Core Foundation exports this immutable, process-lifetime global.
    unsafe { kCFRunLoopDefaultMode }.ok_or(ClaimError::ObjectUnavailable {
        object: "kCFRunLoopDefaultMode",
    })
}

struct ScheduledSession {
    session: CFRetained<DASession>,
    run_loop: CFRetained<CFRunLoop>,
    mode: &'static CFRunLoopMode,
    scheduled: bool,
}

impl ScheduledSession {
    fn create() -> Result<Self, ClaimError> {
        let session = create_session()?;
        let run_loop = CFRunLoop::current().ok_or(ClaimError::ObjectUnavailable {
            object: "the worker CFRunLoop",
        })?;
        let mode = default_run_loop_mode()?;
        let mut scheduled = Self {
            session,
            run_loop,
            mode,
            scheduled: false,
        };

        // SAFETY: All three objects are owned by `scheduled`, which remains on
        // this worker thread. Drop always uses this exact run loop and mode.
        unsafe {
            scheduled
                .session
                .schedule_with_run_loop(&scheduled.run_loop, scheduled.mode);
        }
        scheduled.scheduled = true;
        Ok(scheduled)
    }

    fn session(&self) -> &DASession {
        &self.session
    }

    fn pump_once(&self) {
        CFRunLoop::run_in_mode(Some(self.mode), RUN_LOOP_SLICE_SECONDS, true);
    }

    fn unschedule(&mut self) {
        if !self.scheduled {
            return;
        }

        // SAFETY: This is the same retained session, worker-owned run loop,
        // process-lifetime mode, and thread used by `create`.
        unsafe {
            self.session
                .unschedule_from_run_loop(&self.run_loop, self.mode);
        }
        self.scheduled = false;
    }
}

impl Drop for ScheduledSession {
    fn drop(&mut self) {
        self.unschedule();
    }
}

struct ClaimOperation {
    session: ScheduledSession,
    disk: CFRetained<DADisk>,
    callback: Arc<CallbackState>,
    owns_claim: bool,
}

impl ClaimOperation {
    fn new(session: ScheduledSession, disk: CFRetained<DADisk>) -> Self {
        Self {
            session,
            disk,
            callback: Arc::new(CallbackState::new()),
            owns_claim: false,
        }
    }

    fn issue_claim(&mut self) {
        // The callback receives its own +1 strong reference. It consumes that
        // reference exactly once with Arc::from_raw. If Disk Arbitration never
        // invokes the one-shot completion callback, this one allocation leaks
        // by design instead of permitting a use-after-free.
        let callback_context = Arc::into_raw(Arc::clone(&self.callback))
            .cast_mut()
            .cast::<c_void>();

        // SAFETY: `callback_context` is a raw +1 Arc strong reference, not a
        // borrow from worker-owned storage. DADiskClaim specifies exactly one
        // completion callback, which balances that reference. CallbackState is
        // synchronized for any exceptional delivery thread. Both callbacks
        // have the exact C signatures expected by Disk Arbitration. The release
        // callback has no context and returns a transferred (+1) Busy
        // dissenter, denying all release requests while the claim is held.
        unsafe {
            self.disk.claim(
                kDADiskClaimOptionDefault,
                Some(deny_release_callback),
                ptr::null_mut(),
                Some(claim_completion_callback),
                callback_context,
            );
        }
    }

    fn pump_once(&self) {
        self.session.pump_once();
    }

    fn take_completion(&self) -> ClaimCompletion {
        self.callback.take_completion()
    }

    const fn mark_acquired(&mut self) {
        self.owns_claim = true;
    }
}

impl Drop for ClaimOperation {
    fn drop(&mut self) {
        if self.owns_claim {
            // SAFETY: `disk` is live, and unclaim is invoked on the same worker
            // thread that scheduled and drove the session. The successful
            // completion callback proved that this operation owns the claim;
            // pending and rejected operations are deliberately never
            // unclaimed because they do not own it.
            unsafe { self.disk.unclaim() };
        }
        // Normal paths have already consumed the one-shot completion callback.
        // No arbitrary number of run-loop turns can prove quiescence. We simply
        // unschedule the exact session now; on any exceptional pending path the
        // raw Arc strong reference remains valid (and may intentionally leak).
        self.session.unschedule();
    }
}

struct CallbackState {
    completion: Mutex<ClaimCompletion>,
}

impl CallbackState {
    const fn new() -> Self {
        Self {
            completion: Mutex::new(ClaimCompletion::Pending),
        }
    }

    fn lock_completion(&self) -> MutexGuard<'_, ClaimCompletion> {
        self.completion
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn take_completion(&self) -> ClaimCompletion {
        std::mem::replace(&mut *self.lock_completion(), ClaimCompletion::Pending)
    }

    fn complete(&self, completion: ClaimCompletion) {
        *self.lock_completion() = completion;
    }
}

enum ClaimCompletion {
    Pending,
    Acquired,
    Rejected(i32),
}

unsafe extern "C-unwind" fn claim_completion_callback(
    _disk: NonNull<DADisk>,
    dissenter: *const DADissenter,
    context: *mut c_void,
) {
    let Some(context) = NonNull::new(context.cast::<CallbackState>()) else {
        return;
    };

    // SAFETY: `context` came from Arc::into_raw on a dedicated +1 strong
    // reference immediately before DADiskClaim. That API invokes this
    // completion callback exactly once, so this Arc::from_raw balances exactly
    // that one reference. The worker retains a separate Arc throughout.
    let state = unsafe { Arc::from_raw(context.as_ptr()) };

    let completion = if dissenter.is_null() {
        ClaimCompletion::Acquired
    } else {
        // SAFETY: A non-null dissenter passed to this callback is valid for the
        // callback's duration according to DADiskClaim's contract.
        let status = unsafe { (*dissenter).status() };
        ClaimCompletion::Rejected(status)
    };
    state.complete(completion);
}

unsafe extern "C-unwind" fn deny_release_callback(
    _disk: NonNull<DADisk>,
    _context: *mut c_void,
) -> *const DADissenter {
    // SAFETY: `None` selects the default allocator and omits an optional reason
    // string. The returned `CFRetained` has +1 ownership. `into_raw` transfers
    // that +1 to Disk Arbitration exactly as DADiskClaimReleaseCallback
    // requires; Disk Arbitration is responsible for the matching CFRelease.
    // The generated constructor treats an impossible NULL return as a panic.
    // Never unwind that panic across a system callback frame: aborting is the
    // only fail-closed outcome because returning NULL would approve release.
    let dissenter =
        std::panic::catch_unwind(|| unsafe { DADissenter::new(None, kDAReturnBusy, None) })
            .unwrap_or_else(|_| std::process::abort());
    CFRetained::into_raw(dissenter).as_ptr().cast_const()
}
