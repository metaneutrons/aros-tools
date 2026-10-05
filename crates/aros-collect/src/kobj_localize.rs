//! Reproduce the KOBJ symbol-localization pass from the source make rules.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use aros_common::{AtomicFilePolicy, CancellationToken, DurableFileSet, FileIdentity};

const MAX_ELF_BYTES: u64 = 256 * 1024 * 1024;
const MAX_NM_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_LOCALIZATION_ARGUMENT_BYTES: usize = 128 * 1024;
const FIXED_BASES: [&str; 9] = [
    "DOSBase",
    "IntuitionBase",
    "LayersBase",
    "GfxBase",
    "OOPBase",
    "UtilityBase",
    "ExpansionBase",
    "KeymapBase",
    "KernelBase",
];

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Localize the source make rule's selected symbols in one KOBJ object.
///
/// Both the source and objcopy result are limited to 256 MiB. NM output is
/// captured up to 16 MiB, and the exact localization argument list is limited
/// to 128 KiB. Object files must be ELF relocatables with a portable 0644 or
/// 0755 mode. The original is only replaced through a no-follow, journalled
/// identity-and-content checked publication after objcopy has succeeded.
pub fn localize(object: &Path, nm: &Path, objcopy: &Path) -> Result<()> {
    require_regular_non_symlink(nm, "nm")?;
    require_regular_non_symlink(objcopy, "objcopy")?;

    let Some((original_identity, original_bytes)) =
        aros_common::measure_regular_file_bounded(object, MAX_ELF_BYTES)
            .with_context(|| format!("cannot safely read KOBJ {}", object.display()))?
    else {
        bail!("KOBJ {} does not exist", object.display());
    };
    let original_elf = read_relocatable_elf(&original_bytes, object)?;
    let original_metadata = fs::symlink_metadata(object)
        .with_context(|| format!("cannot inspect KOBJ {}", object.display()))?;
    ensure!(
        original_metadata.file_type().is_file()
            && metadata_matches_identity(&original_metadata, original_identity),
        "KOBJ {} changed during preflight",
        object.display()
    );
    let original_mode = portable_mode(&original_metadata, object)?;

    let mut temporary = TemporaryObject::create(object, &original_bytes, original_mode)?;
    let nm_output = run_nm(nm, temporary.path())?;
    verify_temporary_snapshot(&temporary, &original_bytes)?;
    let symbols = localization_symbols(&nm_output)?;
    verify_temporary_snapshot(&temporary, &original_bytes)?;
    run_objcopy(objcopy, temporary.path(), &symbols)?;
    let (temporary_identity, localized_bytes) =
        aros_common::measure_regular_file_bounded(temporary.path(), MAX_ELF_BYTES)
            .with_context(|| {
                format!(
                    "cannot safely read objcopy result {}",
                    temporary.path().display()
                )
            })?
            .context("objcopy removed its temporary KOBJ input")?;
    temporary.set_snapshot(temporary_identity, &localized_bytes);
    let localized_elf = read_relocatable_elf(&localized_bytes, temporary.path())?;
    ensure!(
        localized_elf.class == original_elf.class
            && localized_elf.machine == original_elf.machine
            && localized_elf.flags == original_elf.flags,
        "objcopy changed the KOBJ ELF class, machine, or flags"
    );
    ensure!(
        localized_elf.os_abi == original_elf.os_abi
            && localized_elf.abi_version == original_elf.abi_version,
        "objcopy changed the KOBJ ELF ABI identity"
    );

    publish_localized(
        object,
        original_identity,
        &original_bytes,
        &localized_bytes,
        original_mode,
    )?;
    Ok(())
}

fn read_relocatable_elf(bytes: &[u8], path: &Path) -> Result<aros_common::elf::Object> {
    let object = aros_common::elf::read(bytes)
        .with_context(|| format!("cannot parse ELF object {}", path.display()))?;
    ensure!(
        object.kind == 1,
        "{} is not an ELF relocatable object (e_type {})",
        path.display(),
        object.kind
    );
    Ok(object)
}

fn require_regular_non_symlink(path: &Path, role: &str) -> Result<()> {
    let file = aros_common::open_regular_file_nofollow(path)
        .with_context(|| format!("cannot safely open selected {role} tool {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("cannot inspect selected {role} tool {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "selected {role} tool {} is not a regular non-symlink file",
        path.display()
    );
    let named_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("cannot recheck selected {role} tool {}", path.display()))?;
    ensure!(
        named_metadata.file_type().is_file()
            && local_identity(&file)
                .is_ok_and(|identity| path_identity_matches(&named_metadata, identity)),
        "selected {role} tool {} changed during preflight",
        path.display()
    );
    Ok(())
}

#[cfg(unix)]
fn metadata_matches_identity(metadata: &fs::Metadata, identity: FileIdentity) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.dev() == identity.device() && metadata.ino() == identity.inode()
}

#[cfg(not(unix))]
fn metadata_matches_identity(_metadata: &fs::Metadata, _identity: FileIdentity) -> bool {
    false
}

#[cfg(unix)]
fn portable_mode(metadata: &fs::Metadata, path: &Path) -> Result<u16> {
    use std::os::unix::fs::MetadataExt;

    let mode = (metadata.mode() & 0o7777) as u16;
    ensure!(
        matches!(mode, 0o644 | 0o755),
        "KOBJ {} has non-portable mode {mode:#o}; expected 0644 or 0755",
        path.display()
    );
    Ok(mode)
}

#[cfg(not(unix))]
fn portable_mode(_metadata: &fs::Metadata, _path: &Path) -> Result<u16> {
    bail!("KOBJ localization requires Unix no-follow publication support")
}

fn run_nm(nm: &Path, object: &Path) -> Result<Vec<u8>> {
    require_regular_non_symlink(nm, "nm")?;
    let mut command = nm_command(nm, object);
    let output = aros_common::run_output_with_control(
        &mut command,
        MAX_NM_CAPTURE_BYTES,
        Duration::from_secs(60),
        &CancellationToken::default(),
    )
    .with_context(|| format!("cannot execute selected nm tool {}", nm.display()))?;
    ensure!(
        !output.timed_out && !output.cancelled,
        "selected nm tool {} timed out or was cancelled",
        nm.display()
    );
    ensure!(
        !output.stdout.is_truncated(),
        "selected nm output exceeded the {MAX_NM_CAPTURE_BYTES}-byte capture limit"
    );
    ensure!(
        output.status.success(),
        "selected nm tool {} failed: {}",
        nm.display(),
        output.stderr.rendered_lossy().trim()
    );
    Ok(output
        .stdout
        .exact_bytes()
        .context("selected nm output was truncated")?
        .to_vec())
}

fn nm_command(nm: &Path, object: &Path) -> Command {
    let mut command = Command::new(nm);
    command.arg("--").arg(object).env("LC_ALL", "C");
    command
}

fn run_objcopy(objcopy: &Path, object: &Path, symbols: &[String]) -> Result<()> {
    require_regular_non_symlink(objcopy, "objcopy")?;
    let mut command = objcopy_command(objcopy, object, symbols);
    let output = aros_common::run_output_with_control(
        &mut command,
        aros_common::DEFAULT_CAPTURE_LIMIT,
        Duration::from_secs(60),
        &CancellationToken::default(),
    )
    .with_context(|| format!("cannot execute selected objcopy tool {}", objcopy.display()))?;
    ensure!(
        !output.timed_out && !output.cancelled,
        "selected objcopy tool {} timed out or was cancelled",
        objcopy.display()
    );
    ensure!(
        output.status.success(),
        "selected objcopy tool {} failed: {}",
        objcopy.display(),
        output.stderr.rendered_lossy().trim()
    );
    Ok(())
}

fn objcopy_command(objcopy: &Path, object: &Path, symbols: &[String]) -> Command {
    let mut command = Command::new(objcopy);
    command.arg(object);
    for symbol in symbols {
        command.arg("-L").arg(symbol);
    }
    command
}

fn localization_symbols(nm_output: &[u8]) -> Result<Vec<String>> {
    let mut symbols: Vec<String> = FIXED_BASES.iter().map(|base| (*base).to_owned()).collect();
    let mut argument_bytes = symbols.iter().map(|symbol| symbol.len() + 2).sum::<usize>();

    for (line_index, line) in nm_output.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields = awk_fields(line);
        // Undefined symbols (including undefined LIST/END names) have only
        // the type and symbol fields in default nm output. The source awk rule
        // reads $3, so they are intentionally not selected here.
        if fields.len() == 2 {
            ensure!(
                fields[0].len() == 1 && fields[0][0].is_ascii_graphic(),
                "malformed undefined-symbol nm row at line {}",
                line_index + 1
            );
            continue;
        }
        ensure!(
            fields.len() == 3,
            "malformed default nm output at line {}: expected three fields",
            line_index + 1
        );
        ensure!(
            !fields[0].is_empty() && fields[0].iter().all(u8::is_ascii_hexdigit),
            "malformed default nm address at line {}",
            line_index + 1
        );
        ensure!(
            fields[1].len() == 1 && fields[1][0].is_ascii_graphic(),
            "malformed default nm type at line {}",
            line_index + 1
        );

        let name = fields[2];
        if !is_dynamic_localization_name(name) {
            continue;
        }
        validate_dynamic_name(name)
            .with_context(|| format!("unsafe dynamic nm symbol at line {}", line_index + 1))?;
        let name = std::str::from_utf8(name)
            .context("dynamic nm symbol is not valid UTF-8")?
            .to_owned();
        argument_bytes = argument_bytes
            .checked_add(name.len() + 2)
            .context("KOBJ localization argument size overflow")?;
        ensure!(
            argument_bytes <= MAX_LOCALIZATION_ARGUMENT_BYTES,
            "KOBJ localization symbol arguments exceed the {MAX_LOCALIZATION_ARGUMENT_BYTES}-byte limit"
        );
        symbols.push(name);
    }
    Ok(symbols)
}

/// Match the default-`nm` third field against the make rule's two patterns.
fn is_dynamic_localization_name(raw_name: &[u8]) -> bool {
    let name = raw_name.strip_suffix(b"\r").unwrap_or(raw_name);
    if name.starts_with(b"__aros_lib") {
        return true;
    }
    let Some(rest) = name.strip_prefix(b"__") else {
        return false;
    };
    rest.ends_with(b"_LIST__") || rest.ends_with(b"_END__")
}

fn validate_dynamic_name(raw_name: &[u8]) -> Result<()> {
    let name = raw_name.strip_suffix(b"\r").unwrap_or(raw_name);
    ensure!(!name.is_empty(), "dynamic symbol name is empty");
    ensure!(
        name.len() <= 1024,
        "dynamic symbol name exceeds the 1024-byte limit"
    );
    ensure!(
        name.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.' | b'$' | b'@' | b'-' | b'+')
        }),
        "dynamic symbol name contains a character outside the safe symbol-name alphabet"
    );
    Ok(())
}

/// AWK's default `FS = " "` separates on horizontal spaces and tabs. A final
/// carriage return remains in `$3`, matching the source regex's optional `\r`.
fn awk_fields(line: &[u8]) -> Vec<&[u8]> {
    line.split(|byte| matches!(*byte, b' ' | b'\t'))
        .filter(|field| !field.is_empty())
        .collect()
}

fn publish_localized(
    object: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
    localized_bytes: &[u8],
    mode: u16,
) -> Result<()> {
    let journal = aros_common::publication_journal_path(object, "kobj-localize")?;
    let mut transaction = DurableFileSet::new(journal).with_context(|| {
        format!(
            "cannot open KOBJ publication transaction for {}",
            object.display()
        )
    })?;
    ensure_original_unchanged(object, expected_identity, expected_bytes)?;

    transaction
        .stage_write_mode(object, localized_bytes, mode)
        .with_context(|| format!("cannot stage localized KOBJ {}", object.display()))?;

    // Check the original snapshot once more after staging. DurableFileSet then
    // rechecks the staged identity and digest immediately before replacement.
    ensure_original_unchanged(object, expected_identity, expected_bytes)?;
    transaction.commit().with_context(|| {
        format!(
            "cannot atomically publish localized KOBJ {}",
            object.display()
        )
    })?;
    Ok(())
}

fn ensure_original_unchanged(
    object: &Path,
    expected_identity: FileIdentity,
    expected_bytes: &[u8],
) -> Result<()> {
    let Some((current_identity, current_bytes)) =
        aros_common::measure_regular_file_bounded(object, MAX_ELF_BYTES).with_context(|| {
            format!(
                "cannot recheck KOBJ {} before publication",
                object.display()
            )
        })?
    else {
        bail!("KOBJ {} disappeared before publication", object.display());
    };
    ensure!(
        current_identity == expected_identity
            && aros_common::sha256_bytes(&current_bytes)
                == aros_common::sha256_bytes(expected_bytes),
        "KOBJ {} changed before localized output could be published",
        object.display()
    );
    Ok(())
}

struct TemporaryObject {
    path: PathBuf,
    identity: Option<FileIdentity>,
    cleanup_sha256: aros_common::Sha256Digest,
    cleanup_size: u64,
}

impl TemporaryObject {
    fn create(object: &Path, bytes: &[u8], mode: u16) -> Result<Self> {
        let parent = object
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let leaf = object
            .file_name()
            .context("KOBJ path has no file name for its temporary copy")?;

        for _ in 0..128 {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let mut temporary_leaf = OsString::from(".");
            temporary_leaf.push(leaf);
            temporary_leaf.push(format!(
                ".aros-localize-{}-{sequence}.tmp",
                std::process::id()
            ));
            let path = parent.join(temporary_leaf);
            ensure!(
                path.file_name().and_then(OsStr::to_str).is_some(),
                "temporary KOBJ path must have a UTF-8 file name"
            );
            match aros_common::publish_atomic_file(&path, bytes, AtomicFilePolicy::NoClobber) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot publish private KOBJ copy {}", path.display())
                    });
                }
            }
            let file = aros_common::open_regular_file_nofollow(&path).with_context(|| {
                format!("cannot safely open private KOBJ copy {}", path.display())
            })?;
            let (device, inode) = local_identity(&file)
                .with_context(|| format!("cannot identify private KOBJ copy {}", path.display()))?;
            let Some((identity, measured_bytes)) =
                aros_common::measure_regular_file_bounded(&path, MAX_ELF_BYTES).with_context(
                    || format!("cannot measure private KOBJ copy {}", path.display()),
                )?
            else {
                bail!("private KOBJ copy {} disappeared", path.display());
            };
            ensure!(
                identity.device() == device && identity.inode() == inode && measured_bytes == bytes,
                "private KOBJ copy {} changed during creation",
                path.display()
            );
            let temporary = Self {
                path,
                identity: Some(identity),
                cleanup_sha256: aros_common::sha256_bytes(bytes),
                cleanup_size: bytes.len() as u64,
            };
            verify_temporary_snapshot(&temporary, bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(u32::from(mode)))
                    .with_context(|| {
                        format!("cannot preserve KOBJ mode on {}", temporary.path.display())
                    })?;
            }
            file.sync_all().with_context(|| {
                format!(
                    "cannot flush private KOBJ copy {}",
                    temporary.path.display()
                )
            })?;
            verify_temporary_snapshot(&temporary, bytes)?;
            return Ok(temporary);
        }
        bail!(
            "cannot allocate a unique private KOBJ copy beside {}",
            object.display()
        )
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn set_snapshot(&mut self, identity: FileIdentity, bytes: &[u8]) {
        self.identity = Some(identity);
        self.cleanup_sha256 = aros_common::sha256_bytes(bytes);
        self.cleanup_size = bytes.len() as u64;
    }
}

fn verify_temporary_snapshot(temporary: &TemporaryObject, expected_bytes: &[u8]) -> Result<()> {
    let file = aros_common::open_regular_file_nofollow(temporary.path()).with_context(|| {
        format!(
            "cannot safely reopen private KOBJ copy {}",
            temporary.path().display()
        )
    })?;
    let metadata = file.metadata().with_context(|| {
        format!(
            "cannot inspect private KOBJ copy {}",
            temporary.path().display()
        )
    })?;
    ensure!(
        metadata.is_file(),
        "private KOBJ copy {} is not a regular file",
        temporary.path().display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "private KOBJ copy has unexpected hard links"
        );
    }
    ensure!(
        temporary.identity.is_some_and(|identity| {
            local_identity(&file).is_ok_and(|(device, inode)| {
                identity.device() == device && identity.inode() == inode
            })
        }),
        "private KOBJ copy {} changed identity",
        temporary.path().display()
    );
    let Some((measured_identity, measured_bytes)) =
        aros_common::measure_regular_file_bounded(temporary.path(), MAX_ELF_BYTES).with_context(
            || {
                format!(
                    "cannot measure private KOBJ copy {}",
                    temporary.path().display()
                )
            },
        )?
    else {
        bail!(
            "private KOBJ copy {} disappeared",
            temporary.path().display()
        );
    };
    ensure!(
        temporary.identity.is_some_and(|identity| {
            measured_identity.device() == identity.device()
                && measured_identity.inode() == identity.inode()
        }) && measured_bytes == expected_bytes,
        "private KOBJ copy {} no longer matches the expected snapshot",
        temporary.path().display()
    );
    Ok(())
}

impl Drop for TemporaryObject {
    fn drop(&mut self) {
        let Some(expected_identity) = self.identity else {
            return;
        };
        let _ = aros_common::remove_regular_file_from_snapshot_nofollow(
            &self.path,
            expected_identity,
            &self.cleanup_sha256,
            self.cleanup_size,
            MAX_ELF_BYTES,
        );
    }
}

#[cfg(unix)]
fn local_identity(file: &File) -> std::io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn local_identity(_file: &File) -> std::io::Result<(u64, u64)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "KOBJ localization requires Unix file identity support",
    ))
}

#[cfg(unix)]
fn path_identity_matches(metadata: &fs::Metadata, expected: (u64, u64)) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.dev() == expected.0 && metadata.ino() == expected.1
}

#[cfg(not(unix))]
fn path_identity_matches(_metadata: &fs::Metadata, _expected: (u64, u64)) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_nm_third_field_matches_source_rules_in_order() {
        let output = b"\
00000000 T __first_LIST__\n\
         U __undefined_END__\n\
00000008 t __local_END__\n\
00000010 D __aros_libreq_DOSBase.50\n\
00000018 T __not_LIST___extra\n\
00000020 T ___LIST__\n\
00000020 T __LIST__\n\
00000020 T __END__\n\
00000028 T __aros_lib\n\
00000030 T __aros_libfoo\n\
00000038 D __crlf_END__\r\n\
00000040 a module.c\n\
         U __aros_lib_undefined\n";

        let symbols = localization_symbols(output).unwrap();
        assert_eq!(
            symbols,
            [
                "DOSBase",
                "IntuitionBase",
                "LayersBase",
                "GfxBase",
                "OOPBase",
                "UtilityBase",
                "ExpansionBase",
                "KeymapBase",
                "KernelBase",
                "__first_LIST__",
                "__local_END__",
                "__aros_libreq_DOSBase.50",
                "___LIST__",
                "__aros_lib",
                "__aros_libfoo",
                "__crlf_END__\r",
            ]
        );
    }

    #[test]
    fn default_nm_undefined_rows_have_no_third_field() {
        let symbols =
            localization_symbols(b"         U __MISSING_LIST__\n w __aros_libmissing\n").unwrap();
        assert_eq!(symbols, FIXED_BASES);
    }

    #[test]
    fn malformed_and_unsafe_dynamic_nm_rows_are_rejected() {
        assert!(localization_symbols(b"malformed\n").is_err());
        assert!(localization_symbols(b"00000000 T __bad!_LIST__\n").is_err());
        assert!(localization_symbols(b"00000000 T __ok_LIST__ extra\n").is_err());
    }

    #[test]
    fn localization_argument_budget_refuses_an_incomplete_recipe() {
        let row = b"00000000 T __aros_lib_fixture\n";
        let excessive = row.repeat(MAX_LOCALIZATION_ARGUMENT_BYTES / 10);
        assert!(localization_symbols(&excessive)
            .unwrap_err()
            .to_string()
            .contains("argument"));
    }

    #[test]
    fn command_arguments_match_the_plain_nm_and_objcopy_recipes() {
        let nm_path = Path::new("/tools/riscv-aros-nm");
        let input = Path::new("/tmp/.kernel.KOBJ.tmp");
        let command = nm_command(nm_path, input);
        assert_eq!(command.get_program(), OsStr::new("/tools/riscv-aros-nm"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("--"), OsStr::new("/tmp/.kernel.KOBJ.tmp")]
        );
        assert!(command
            .get_envs()
            .any(|(key, value)| key == OsStr::new("LC_ALL") && value == Some(OsStr::new("C"))));

        let symbols = vec!["DOSBase".to_owned(), "__foo_LIST__".to_owned()];
        let command = objcopy_command(Path::new("/tools/riscv-aros-objcopy"), input, &symbols);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("/tmp/.kernel.KOBJ.tmp"),
                OsStr::new("-L"),
                OsStr::new("DOSBase"),
                OsStr::new("-L"),
                OsStr::new("__foo_LIST__"),
            ]
        );
    }
}
