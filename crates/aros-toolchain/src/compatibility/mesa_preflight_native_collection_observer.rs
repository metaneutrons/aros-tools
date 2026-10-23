//! Private native Make collection setup and readback.
//!
//! This module prepares the already selected observer frontend and reads its
//! retained packets after the supervising process tree has been reaped. It
//! never launches Make. Observed slot names and stream hashes are live data,
//! not an independently supplied call roster or admission decision. The data
//! does not prove Source ownership, process authentication, successful ordinary
//! Make execution, or completion of a native preflight.

use super::failure;
use crate::ContractError;
use aros_common::{
    directory_entry_names_nofollow_bounded,
    make_child_command::LiteralMakeChildCommand,
    make_initial_values::{ValidatedMakeInitialValues, ValidatedMakeRecipeControls},
    make_namespace::ValidatedMakeNamespace,
    make_native_collection::{read_native_make_collection, NativeMakeCallSelection},
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    measure_regular_file_bounded, measure_regular_file_digest_bounded, sha256_bytes,
    CancellationToken, Sha256Digest,
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Instant,
};

const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_INVOCATION_BYTES: usize = 2 * 1024 * 1024;
const MAX_INITIAL_STDOUT_BYTES: usize = 256 * 1024;
const MAX_OBSERVATION_BYTES: usize = 4 * 1024 * 1024;

/// Explicit selected inputs, not inferred observer/runtime authority.
pub(super) struct CollectionObserverSelection<'a> {
    pub(super) observer: &'a Path,
    pub(super) observer_sha256: &'a Sha256Digest,
    pub(super) program: &'a Path,
    pub(super) program_sha256: &'a Sha256Digest,
    pub(super) argument_zero: &'a str,
    pub(super) maximum_calls: usize,
}

/// A held private observer collection. This carries no execution authority.
pub(super) struct CollectionObserver {
    root: PrivateNativeCaptureDirectory,
    collection: PrivateNativeCaptureDirectory,
    alias: PathBuf,
    observer: PathBuf,
    observer_sha256: Sha256Digest,
    observer_mode: u32,
    program: PathBuf,
    program_sha256: Sha256Digest,
    program_mode: u32,
    argument_zero: String,
    maximum_calls: usize,
    context_sha256: Sha256Digest,
    control_sha256: Sha256Digest,
    recipe_controls: bool,
}

struct CallReadback {
    directory: PrivateNativeCaptureDirectory,
    invocation: Vec<u8>,
    initial_stdout: Vec<u8>,
    packet_sha256: Sha256Digest,
    binding: MakeObservationBinding,
}

impl CollectionObserver {
    /// Check only configured-project syntax/capacity before expensive Source
    /// projection. A fixed-width stand-in checks layout, never authentication
    /// or execution authority. Actual setup must still bind the real control
    /// digest and repeat derivation with the original live owner.
    pub(super) fn check_project_layout(
        output: &Path,
        original: &[u8],
        expected_sha256: &Sha256Digest,
        project: &str,
        source_word: &str,
    ) -> Result<(), ContractError> {
        let launcher = collection_launcher(output, &sha256_bytes(b"layout-only"));
        aros_transpiler::metamake_project::MetaMakeLauncherConfiguration::derive_confined_project(
            original, expected_sha256, project, source_word, &launcher,
        ).map(|_| ()).map_err(|_| failure(
            "native observer project layout is unsupported; instrumented physical lines must fit native MetaMake's 255-byte limit (shorten the private evidence root if necessary)",
        ))
    }

    /// Prepare an observer collection under a caller-created exact-0700 output.
    ///
    /// The short `o` symlink points to the selected canonical observer. The
    /// caller's physical-configuration parser remains responsible for its
    /// launcher's 255-byte limit. This method does not run the observer or Make.
    ///
    /// # Errors
    /// Refuses changed or unsafe selections, malformed bounds, pre-existing
    /// namespace entries, or failure to retain the private control documents.
    pub(super) fn prepare(
        output: &Path,
        selected: &CollectionObserverSelection<'_>,
        source_context: &serde_json::Value,
    ) -> Result<Self, ContractError> {
        Self::prepare_mode(output, selected, source_context, false)
    }

    /// Select the versioned first-input recipe-control observation explicitly.
    /// Legacy collections remain unchanged and cannot supply missing controls.
    pub(super) fn prepare_with_recipe_controls(
        output: &Path,
        selected: &CollectionObserverSelection<'_>,
        source_context: &serde_json::Value,
    ) -> Result<Self, ContractError> {
        Self::prepare_mode(output, selected, source_context, true)
    }

    fn prepare_mode(
        output: &Path,
        selected: &CollectionObserverSelection<'_>,
        source_context: &serde_json::Value,
        recipe_controls: bool,
    ) -> Result<Self, ContractError> {
        let CollectionObserverSelection {
            observer,
            observer_sha256,
            program,
            program_sha256,
            argument_zero,
            maximum_calls,
        } = *selected;
        if !(1..=aros_common::make_native_collection::MAX_NATIVE_MAKE_CALLS)
            .contains(&maximum_calls)
            || argument_zero.is_empty()
            || argument_zero.len() > 128 * 1024
            || argument_zero.contains('\0')
            || argument_zero.chars().any(char::is_control)
        {
            return Err(failure("invalid native observer collection selection"));
        }

        let observer = canonical_selected_executable(observer, observer_sha256)?;
        let program = canonical_selected_executable(program, program_sha256)?;
        let observer_mode = executable_mode(&observer)?;
        let program_mode = executable_mode(&program)?;
        let root = PrivateNativeCaptureDirectory::open(output)
            .map_err(|_| failure("cannot hold native observer collection output"))?;
        let alias = root.path().join("o");
        validate_launcher_paths(&alias, &root.path().join("c"))?;
        std::os::unix::fs::symlink(&observer, &alias)
            .map_err(|_| failure("cannot create native observer short alias"))?;

        let collection_path = root.path().join("d");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&collection_path)
            .map_err(|_| failure("cannot create native observer collection directory"))?;
        fs::set_permissions(&collection_path, fs::Permissions::from_mode(0o700))
            .map_err(|_| failure("cannot set native observer collection directory mode"))?;
        let collection = PrivateNativeCaptureDirectory::open(&collection_path)
            .map_err(|_| failure("cannot hold native observer collection directory"))?;

        let mut context = serde_json::json!({
            "version": 1,
            "source_context": source_context,
            "phase": MakeObservationPhase::FirstInputPrelude,
            "maximum_calls": maximum_calls,
            "program": program,
            "program_sha256": program_sha256,
            "argument_zero": argument_zero,
            "observer": observer,
            "observer_sha256": observer_sha256,
            "observer_mode": observer_mode,
            "program_mode": program_mode,
        });
        if recipe_controls {
            context["version"] = 2.into();
            context["recipe_controls"] = true.into();
        }
        let context = serde_json::to_vec(&context)
            .map_err(|_| failure("cannot encode native observer context"))?;
        let context_sha256 = sha256_bytes(&context);
        root.write_new(OsStr::new("native-context.json"), &context)
            .map_err(|_| failure("cannot retain native observer context"))?;

        let control = serde_json::to_vec(&serde_json::json!({
            "version": if recipe_controls { 6 } else if maximum_calls > aros_common::make_native_collection::LEGACY_NATIVE_MAKE_CALL_LIMIT { 5 } else { 4 },
            "phase": MakeObservationPhase::FirstInputPrelude,
            "maximum_calls": maximum_calls,
            "output_root": collection_path,
            "program": program,
            "executable_sha256": program_sha256,
            "context_sha256": context_sha256,
            "argument_zero": argument_zero,
        }))
        .map_err(|_| failure("cannot encode native observer control"))?;
        let control_sha256 = sha256_bytes(&control);
        root.write_new(OsStr::new("c"), &control)
            .map_err(|_| failure("cannot retain native observer control"))?;

        let result = Self {
            root,
            collection,
            alias,
            observer,
            observer_sha256: observer_sha256.clone(),
            observer_mode,
            program,
            program_sha256: program_sha256.clone(),
            program_mode,
            argument_zero: argument_zero.to_owned(),
            maximum_calls,
            context_sha256,
            control_sha256,
            recipe_controls,
        };
        result.revalidate()?;
        Ok(result)
    }

    /// Launcher text consumed by the selected physical-configuration parser.
    #[must_use]
    pub(super) fn launcher(&self) -> String {
        collection_launcher(self.root.path(), &self.control_sha256)
    }

    /// Digest of the selected context, including Source context and capture mode.
    #[must_use]
    pub(super) const fn context_sha256(&self) -> &Sha256Digest {
        &self.context_sha256
    }

    /// Digest of the selected versioned collection control record.
    #[must_use]
    pub(super) const fn control_sha256(&self) -> &Sha256Digest {
        &self.control_sha256
    }

    /// Revalidate the selected executables, held roots, alias and controls.
    ///
    /// This binds the private collection inputs only; it does not establish
    /// source ownership or the authenticity of any observer-produced packet.
    ///
    /// # Errors
    /// Refuses a replaced output, changed symlink target, executable, or control.
    pub(super) fn revalidate(&self) -> Result<(), ContractError> {
        self.root
            .revalidate()
            .map_err(|_| failure("native observer output directory changed"))?;
        self.collection
            .revalidate()
            .map_err(|_| failure("native observer collection directory changed"))?;
        if self.collection.path() != self.root.path().join("d").as_path() {
            return Err(failure("native observer collection path changed"));
        }
        if fs::read_link(&self.alias)
            .map_or(true, |target| target.as_path() != self.observer.as_path())
        {
            return Err(failure("native observer short alias changed"));
        }
        check_selected_executable(&self.observer, &self.observer_sha256)?;
        check_selected_executable(&self.program, &self.program_sha256)?;
        if executable_mode(&self.observer)? != self.observer_mode
            || executable_mode(&self.program)? != self.program_mode
        {
            return Err(failure("native observer executable permissions changed"));
        }
        self.root
            .read_selected(
                OsStr::new("native-context.json"),
                &self.context_sha256,
                MAX_OBSERVATION_BYTES,
            )
            .map_err(|_| failure("native observer context changed"))?;
        self.root
            .read_selected(OsStr::new("c"), &self.control_sha256, 64 * 1024)
            .map_err(|_| failure("native observer control changed"))?;
        self.root
            .revalidate()
            .map_err(|_| failure("native observer output directory changed"))?;
        Ok(())
    }

    /// Read observer collection data after its supervising tree is reaped.
    ///
    /// The caller must establish that reaping before calling this method; the
    /// directory contents cannot establish it. The observed call count comes
    /// only from the names in `d`, not from an independent owner roster. Each
    /// invocation, stdout stream and observation packet is bounded and read
    /// through held private directories. Returned data proves consistency only.
    ///
    /// # Errors
    /// Refuses empty, incomplete, extra collection slots, unsafe, mismatched or interrupted data,
    /// an invocation outside the selected build root, or a PATH that does not
    /// resolve the original Source word to the sealed GNU executable.
    pub(super) fn read_after_reaped(
        &self,
        build_root: &Path,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<Vec<RetainedMakeObservation>, ContractError> {
        check_control(deadline, cancellation)?;
        self.revalidate()?;
        let build_root = canonical_directory(build_root)?;

        let names = directory_entry_names_nofollow_bounded(
            self.collection.path(),
            self.maximum_calls.saturating_mul(2),
        )
        .map_err(|_| failure("cannot enumerate native observer collection"))?;
        let slots = selected_observed_slots(names, self.maximum_calls)?;
        let mut selections = Vec::with_capacity(slots.len());
        let mut readbacks = Vec::with_capacity(slots.len());
        let mut retained_bytes = 0usize;

        for slot in slots {
            check_control(deadline, cancellation)?;
            let call_path = self.collection.path().join(format!("call-{slot:04}"));
            let directory = PrivateNativeCaptureDirectory::open(&call_path)
                .map_err(|_| failure("native observer call directory is unsafe"))?;
            let invocation_bytes =
                read_private_file(&directory, "invocation.json", MAX_INVOCATION_BYTES)?;
            charge_collection_bytes(&mut retained_bytes, invocation_bytes.len())?;
            let invocation =
                aros_common::make_native_invocation::NativeMakeInvocation::parse(&invocation_bytes)
                    .map_err(|_| failure("native observer invocation is invalid"))?;
            if invocation.argument_zero() != Some(self.argument_zero.as_str()) {
                return Err(failure("native observer argument zero differs"));
            }

            let working_directory = invocation.working_directory();
            let canonical_working_directory = fs::canonicalize(working_directory)
                .map_err(|_| failure("native observer working directory is unavailable"))?;
            let working_metadata = fs::metadata(&canonical_working_directory)
                .map_err(|_| failure("native observer working directory is unavailable"))?;
            if canonical_working_directory.as_path() != working_directory
                || !canonical_working_directory.starts_with(&build_root)
                || !working_metadata.is_dir()
            {
                return Err(failure(
                    "native observer working directory is outside the selected build root",
                ));
            }

            let captured_path = invocation
                .environment()
                .get("PATH")
                .ok_or_else(|| failure("native observer invocation has no PATH"))?;
            let resolved = which::which_in(
                &self.argument_zero,
                Some(captured_path.as_str()),
                working_directory,
            )
            .map_err(|_| failure("native observer PATH does not resolve the source Make word"))?;
            let resolved = fs::canonicalize(resolved)
                .map_err(|_| failure("native observer PATH result is unavailable"))?;
            if resolved.as_path() != self.program.as_path() {
                return Err(failure(
                    "native observer PATH resolves a different GNU Make executable",
                ));
            }

            let initial_stdout =
                read_private_file(&directory, "initial.stdout", MAX_INITIAL_STDOUT_BYTES)?;
            charge_collection_bytes(&mut retained_bytes, initial_stdout.len())?;
            let namespace = ValidatedMakeNamespace::from_make_stdout(&initial_stdout)
                .map_err(|_| failure("native observer initial namespace is invalid"))?;
            let values = ValidatedMakeInitialValues::from_make_stdout(&initial_stdout, &namespace)
                .map_err(|_| failure("native observer initial values are invalid"))?;
            if self.recipe_controls {
                ValidatedMakeRecipeControls::from_make_stdout(&initial_stdout, &namespace)
                    .map_err(|_| {
                        failure("native observer selected recipe controls are absent or invalid")
                    })?;
            }
            if values
                .entries()
                .find(|entry| entry.name == "MAKE_COMMAND")
                .is_none_or(|entry| entry.value.as_str() != self.argument_zero.as_str())
            {
                return Err(failure("native observer MAKE_COMMAND differs"));
            }

            let packet = read_private_file(&directory, "observation.json", MAX_OBSERVATION_BYTES)?;
            charge_collection_bytes(&mut retained_bytes, packet.len())?;
            let packet_sha256 = sha256_bytes(&packet);
            let binding = MakeObservationBinding {
                phase: MakeObservationPhase::FirstInputPrelude,
                context_sha256: self.context_sha256.clone(),
                invocation_sha256: sha256_bytes(&invocation_bytes),
                stdout_sha256: sha256_bytes(&initial_stdout),
            };
            selections.push(NativeMakeCallSelection {
                document_sha256: packet_sha256.clone(),
                binding: binding.clone(),
            });
            readbacks.push(CallReadback {
                directory,
                invocation: invocation_bytes,
                initial_stdout,
                packet_sha256,
                binding,
            });
        }

        check_control(deadline, cancellation)?;
        self.revalidate()?;
        let retained = read_native_make_collection(
            &self.collection,
            &self.control_sha256,
            &selections,
            deadline,
            cancellation,
        )
        .map_err(|_| failure("native observer collection is incomplete or inconsistent"))?;
        if retained.len() != readbacks.len() {
            return Err(failure("native observer collection count changed"));
        }

        for (observation, readback) in retained.iter().zip(&readbacks) {
            if observation.invocation().to_bytes().ok().as_deref()
                != Some(readback.invocation.as_slice())
                || observation.initial_stdout() != readback.initial_stdout.as_slice()
                || observation.binding() != &readback.binding
            {
                return Err(failure("native observer collection differs from readback"));
            }
            if read_private_file(&readback.directory, "invocation.json", MAX_INVOCATION_BYTES)?
                != readback.invocation
                || read_private_file(
                    &readback.directory,
                    "initial.stdout",
                    MAX_INITIAL_STDOUT_BYTES,
                )? != readback.initial_stdout
                || sha256_bytes(&read_private_file(
                    &readback.directory,
                    "observation.json",
                    MAX_OBSERVATION_BYTES,
                )?) != readback.packet_sha256
            {
                return Err(failure("native observer retained call data changed"));
            }
            readback
                .directory
                .revalidate()
                .map_err(|_| failure("native observer call directory changed"))?;
        }
        self.revalidate()?;
        check_control(deadline, cancellation)?;
        Ok(retained)
    }
}

fn collection_launcher(output: &Path, control_sha256: &Sha256Digest) -> String {
    format!(
        "{} --observe-make-launch-collection {} {} --",
        output.join("o").display(),
        output.join("c").display(),
        control_sha256
    )
}

fn charge_collection_bytes(total: &mut usize, additional: usize) -> Result<(), ContractError> {
    *total = total
        .checked_add(additional)
        .filter(|value| {
            *value <= aros_common::make_native_collection::MAX_NATIVE_MAKE_COLLECTION_BYTES
        })
        .ok_or_else(|| failure("native observer aggregate retained bytes exceed bound"))?;
    Ok(())
}

fn canonical_selected_executable(
    path: &Path,
    expected: &Sha256Digest,
) -> Result<PathBuf, ContractError> {
    let canonical = fs::canonicalize(path)
        .map_err(|_| failure("selected native observer executable is unavailable"))?;
    if !canonical.is_absolute() || canonical.to_str().is_none() {
        return Err(failure(
            "selected native observer executable path is invalid",
        ));
    }
    check_selected_executable(&canonical, expected)?;
    Ok(canonical)
}

fn check_selected_executable(path: &Path, expected: &Sha256Digest) -> Result<(), ContractError> {
    if fs::canonicalize(path).map_or(true, |canonical| canonical.as_path() != path) {
        return Err(failure("selected native observer executable path changed"));
    }
    let (_, measured) = measure_regular_file_digest_bounded(path, MAX_EXECUTABLE_BYTES)
        .map_err(|_| failure("cannot measure selected native observer executable"))?;
    if measured.digest != *expected {
        return Err(failure("selected native observer executable changed"));
    }
    executable_mode(path)?;
    Ok(())
}

fn executable_mode(path: &Path) -> Result<u32, ContractError> {
    let file = aros_common::open_regular_file_nofollow(path)
        .map_err(|_| failure("cannot open selected native executable permissions"))?;
    let mode = file
        .metadata()
        .map_err(|_| failure("cannot measure selected native executable permissions"))?
        .permissions()
        .mode()
        & 0o7777;
    if mode & 0o111 == 0 {
        return Err(failure(
            "selected native observer program is not executable",
        ));
    }
    Ok(mode)
}

// A Source configuration later passes this text through its own shell launcher.
// Refuse unsupported path spelling before writing anything, rather than silently
// splitting paths or interpreting shell syntax as part of the chosen command.
fn validate_launcher_paths(alias: &Path, control: &Path) -> Result<(), ContractError> {
    let alias = alias
        .to_str()
        .ok_or_else(|| failure("native observer alias is not UTF-8"))?;
    let control = control
        .to_str()
        .ok_or_else(|| failure("native observer control path is not UTF-8"))?;
    let digest = sha256_bytes(b"launcher path validation").to_string();
    let expected = [
        "--observe-make-launch-collection",
        control,
        digest.as_str(),
        "--",
    ];
    let command = LiteralMakeChildCommand::parse(&format!("{alias} {}", expected.join(" ")))
        .map_err(|_| failure("native observer paths are not literal launcher words"))?;
    if command.program() != alias
        || command.arguments().iter().map(String::as_str).ne(expected)
        || !command.environment_assignments().is_empty()
    {
        return Err(failure(
            "native observer paths do not round-trip as launcher words",
        ));
    }
    Ok(())
}

fn selected_observed_slots(
    names: Vec<OsString>,
    maximum_calls: usize,
) -> Result<Vec<usize>, ContractError> {
    let entry_count = names.len();
    if entry_count == 0 || !entry_count.is_multiple_of(2) || entry_count > maximum_calls * 2 {
        return Err(failure(
            "native observer collection has an invalid slot count",
        ));
    }
    let mut observed = BTreeMap::<usize, (bool, bool)>::new();
    for name in names {
        let name = name
            .to_str()
            .ok_or_else(|| failure("native observer collection has an invalid name"))?;
        let (call_name, is_claim) = name
            .strip_suffix(".claim")
            .map_or((name, false), |call_name| (call_name, true));
        let digits = call_name
            .strip_prefix("call-")
            .ok_or_else(|| failure("native observer collection has an unexpected entry"))?;
        if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(failure(
                "native observer collection has an invalid slot name",
            ));
        }
        let slot = digits
            .parse::<usize>()
            .map_err(|_| failure("native observer collection has an invalid slot name"))?;
        if slot >= maximum_calls || format!("call-{slot:04}") != call_name {
            return Err(failure("native observer collection slot exceeds selection"));
        }
        let pair = observed.entry(slot).or_default();
        let present = if is_claim { &mut pair.1 } else { &mut pair.0 };
        if std::mem::replace(present, true) {
            return Err(failure("native observer collection repeats a slot entry"));
        }
    }
    if observed.is_empty()
        || observed.len() * 2 != entry_count
        || observed.values().any(|(call, claim)| !call || !claim)
    {
        return Err(failure(
            "native observer collection has an incomplete slot pair",
        ));
    }
    let count = observed.len();
    if !observed.keys().copied().eq(0..count) {
        return Err(failure(
            "native observer collection slots are not contiguous",
        ));
    }
    Ok((0..count).collect())
}

fn read_private_file(
    directory: &PrivateNativeCaptureDirectory,
    name: &str,
    maximum_bytes: usize,
) -> Result<Vec<u8>, ContractError> {
    let path = directory.path().join(name);
    let (_, measured) = measure_regular_file_bounded(&path, maximum_bytes as u64)
        .map_err(|_| failure("cannot measure native observer call data"))?
        .ok_or_else(|| failure("native observer call data is missing"))?;
    directory
        .read_selected(OsStr::new(name), &sha256_bytes(&measured), maximum_bytes)
        .map_err(|_| failure("native observer call data is unsafe or changed"))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ContractError> {
    let canonical =
        fs::canonicalize(path).map_err(|_| failure("selected native build root is unavailable"))?;
    if canonical != path || !canonical.is_dir() {
        return Err(failure("selected native build root is not canonical"));
    }
    Ok(canonical)
}

fn check_control(deadline: Instant, cancellation: &CancellationToken) -> Result<(), ContractError> {
    if cancellation.is_cancelled() || Instant::now() >= deadline {
        return Err(failure("native observer collection readback interrupted"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mesa_preflight_native_collection_observer_tests.rs"]
mod tests;
