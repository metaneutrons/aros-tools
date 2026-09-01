//! Private installation and journalled publication of the complete AHI set.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, Ordering};

use aros_common::{
    bounded_output_detail, measure_regular_file, publication_journal_path, run_output, Diagnostic,
    DiagnosticCode, DiagnosticContext, DiagnosticStage, DurableFileSet, FileIdentity,
};

use crate::contract::{Contract, ProductKind};
use crate::engine::closed_build_command;
use crate::{AhiFailure, AhiResult};

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const STAGE_PREFIX: &str = ".ahi-install-stage-";

#[derive(Debug)]
struct InstallStage {
    root: PathBuf,
}

impl InstallStage {
    fn create(contract: &Contract) -> AhiResult<Self> {
        for _ in 0..128 {
            let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = contract
                .binary_dir
                .join(format!("{STAGE_PREFIX}{}-{sequence}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&root) {
                Ok(()) => {
                    let stage = Self { root };
                    fs::set_permissions(&stage.root, fs::Permissions::from_mode(0o700)).map_err(
                        |error| {
                            install_failure(
                                contract,
                                format!(
                                    "cannot restrict private install stage {}: {error}",
                                    stage.root.display()
                                ),
                                None,
                            )
                        },
                    )?;
                    return Ok(stage);
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(install_failure(
                        contract,
                        format!(
                            "cannot create private install stage below {}: {error}",
                            contract.binary_dir.display()
                        ),
                        None,
                    ));
                }
            }
        }
        Err(install_failure(
            contract,
            "cannot allocate a unique private install stage",
            None,
        ))
    }

    fn physical_prefix(&self, contract: &Contract) -> AhiResult<PathBuf> {
        let relative = contract
            .install_prefix
            .strip_prefix(Path::new("/"))
            .map_err(|_| product_failure(contract, "AHI install prefix is not absolute"))?;
        Ok(self.root.join(relative))
    }

    fn cleanup(&self, contract: &Contract) -> AhiResult<()> {
        fs::remove_dir_all(&self.root).map_err(|error| {
            install_failure(
                contract,
                format!(
                    "cannot remove validated private install stage {} before publication: {error}",
                    self.root.display()
                ),
                None,
            )
        })
    }
}

impl Drop for InstallStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileSnapshot {
    identity: FileIdentity,
    contents: Vec<u8>,
    mode: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LiveSnapshot {
    Absent,
    Complete(Vec<FileSnapshot>),
}

#[derive(Clone, Debug)]
struct PreparedProduct {
    target: PathBuf,
    contents: Vec<u8>,
    mode: u16,
}

#[derive(Debug)]
pub struct PreparedInstallation {
    baseline: LiveSnapshot,
    products: Vec<PreparedProduct>,
}

/// Install into a private physical root and capture one exact validated set.
pub fn prepare(contract: &Contract) -> AhiResult<PreparedInstallation> {
    let baseline = measure_live_set(contract)?;
    let stage = InstallStage::create(contract)?;
    let physical_prefix = stage.physical_prefix(contract)?;
    run_private_install(contract, &stage.root, &physical_prefix)?;
    let products = validate_private_install(contract, &stage.root, &physical_prefix)?;
    // All exact bytes and modes are now owned in memory. Remove the transient
    // install tree before crossing the live publication boundary so cleanup
    // failure can never be reported after a committed set.
    stage.cleanup(contract)?;
    Ok(PreparedInstallation { baseline, products })
}

/// Publish the prepared products as one lock-protected, journalled set.
pub fn publish(contract: &Contract, prepared: &PreparedInstallation) -> AhiResult<()> {
    publish_products(contract, &prepared.baseline, &prepared.products)
}

fn run_private_install(
    contract: &Contract,
    stage_root: &Path,
    physical_prefix: &Path,
) -> AhiResult<()> {
    let mut command = closed_build_command(contract, &contract.make);
    command
        .env("DESTDIR", stage_root)
        .arg("-C")
        .arg(&contract.stage_build)
        .arg(format!("DESTDIR={}", stage_root.display()))
        // The audited AHI makefiles predate DESTDIR and write directly through
        // these variables. Configure still used the logical live prefix; only
        // the physical install invocation is redirected here.
        .arg(make_assignment("PREFIX", physical_prefix))
        .arg(make_assignment("prefix", physical_prefix))
        .arg(make_assignment("bindir", physical_prefix))
        .arg(make_assignment("sbindir", physical_prefix))
        .arg(make_assignment("libdir", &physical_prefix.join("Libs")))
        .arg(make_assignment(
            "includedir",
            &physical_prefix.join("Developer/include"),
        ))
        .arg(make_assignment(
            "oldincludedir",
            &physical_prefix.join("Developer/include"),
        ))
        .arg("install");
    let output = run_output(&mut command).map_err(|error| {
        install_failure(
            contract,
            format!("cannot start private AHI make install: {error}"),
            None,
        )
    })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = bounded_output_detail(&output.stdout, &output.stderr);
    let message = if detail.is_empty() {
        "private AHI make install failed".to_owned()
    } else {
        format!("private AHI make install failed: {detail}")
    };
    Err(install_failure(
        contract,
        message,
        Some((&contract.make, output.status)),
    ))
}

fn make_assignment(name: &str, value: &Path) -> OsString {
    let mut assignment = OsString::from(name);
    assignment.push("=");
    assignment.push(value);
    assignment
}

fn validate_private_install(
    contract: &Contract,
    stage_root: &Path,
    physical_prefix: &Path,
) -> AhiResult<Vec<PreparedProduct>> {
    let actual = inventory_regular_files(contract, stage_root)?;
    let expected: BTreeSet<_> = contract
        .product_relative
        .iter()
        .map(|relative| physical_prefix.join(relative))
        .collect();
    if actual != expected {
        let missing = expected.difference(&actual).next();
        let unexpected = actual.difference(&expected).next();
        return Err(product_failure(
            contract,
            format!(
                "private AHI install inventory differs from the contract{}{}",
                missing.map_or_else(String::new, |path| format!("; missing {}", path.display())),
                unexpected.map_or_else(String::new, |path| format!(
                    "; unexpected {}",
                    path.display()
                ))
            ),
        ));
    }

    let mut products = Vec::with_capacity(contract.product_relative.len());
    for ((relative, kind), target) in contract
        .product_relative
        .iter()
        .zip(&contract.product_kinds)
        .zip(&contract.install_products)
    {
        let staged = physical_prefix.join(relative);
        if target != &contract.install_prefix.join(relative) {
            return Err(product_failure(
                contract,
                format!("installed product path differs for {}", relative.display()),
            ));
        }
        let (_, contents, mode) = measure_file_with_mode(&staged).map_err(|error| {
            product_failure(
                contract,
                format!(
                    "cannot capture staged product {}: {error}",
                    staged.display()
                ),
            )
        })?;
        if !matches!(mode, 0o644 | 0o755) {
            return Err(product_failure(
                contract,
                format!(
                    "staged product {} has unsupported mode {mode:04o}",
                    relative.display()
                ),
            ));
        }
        if contents.is_empty() {
            return Err(product_failure(
                contract,
                format!("staged product {} is empty", relative.display()),
            ));
        }
        if contents
            .windows(stage_root.as_os_str().as_bytes().len())
            .any(|window| window == stage_root.as_os_str().as_bytes())
        {
            return Err(product_failure(
                contract,
                format!(
                    "staged product {} embeds the private physical install root",
                    relative.display()
                ),
            ));
        }
        if *kind == ProductKind::Elf {
            validate_elf(contract, relative, &contents)?;
        }
        products.push(PreparedProduct {
            target: target.clone(),
            contents,
            mode,
        });
    }
    Ok(products)
}

fn inventory_regular_files(contract: &Contract, root: &Path) -> AhiResult<BTreeSet<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = BTreeSet::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            product_failure(
                contract,
                format!(
                    "cannot enumerate private install directory {}: {error}",
                    directory.display()
                ),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                product_failure(
                    contract,
                    format!("cannot enumerate private install: {error}"),
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                product_failure(
                    contract,
                    format!(
                        "cannot inspect private install entry {}: {error}",
                        path.display()
                    ),
                )
            })?;
            let kind = metadata.file_type();
            if kind.is_symlink() {
                return Err(product_failure(
                    contract,
                    format!("private install entry {} is a symlink", path.display()),
                ));
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                files.insert(path);
            } else {
                return Err(product_failure(
                    contract,
                    format!("private install entry {} is not regular", path.display()),
                ));
            }
        }
    }
    Ok(files)
}

fn validate_elf(contract: &Contract, relative: &Path, contents: &[u8]) -> AhiResult<()> {
    if contents.len() < 20 {
        return Err(product_failure(
            contract,
            format!("ELF product {} is truncated", relative.display()),
        ));
    }
    let expected_class = u8::from_str_radix(&contract.elf_class, 16).map_err(|error| {
        product_failure(contract, format!("invalid ELF class contract: {error}"))
    })?;
    let expected_machine = [
        u8::from_str_radix(&contract.elf_machine_hex[0..2], 16).map_err(|error| {
            product_failure(contract, format!("invalid ELF machine contract: {error}"))
        })?,
        u8::from_str_radix(&contract.elf_machine_hex[2..4], 16).map_err(|error| {
            product_failure(contract, format!("invalid ELF machine contract: {error}"))
        })?,
    ];
    if contents[0..4] != *b"\x7fELF"
        || contents[4] != expected_class
        || contents[18..20] != expected_machine
    {
        return Err(product_failure(
            contract,
            format!("ELF product {} has wrong format", relative.display()),
        ));
    }
    Ok(())
}

fn measure_live_set(contract: &Contract) -> AhiResult<LiveSnapshot> {
    let mut present = Vec::with_capacity(contract.install_products.len());
    let mut absent = 0_usize;
    for target in &contract.install_products {
        match measure_optional_file_with_mode(target) {
            Ok(Some((identity, contents, mode))) if matches!(mode, 0o644 | 0o755) => {
                present.push(FileSnapshot {
                    identity,
                    contents,
                    mode,
                });
            }
            Ok(Some((_, _, mode))) => {
                return Err(product_failure(
                    contract,
                    format!(
                        "live AHI product {} has unsafe mode {mode:04o}",
                        target.display()
                    ),
                ));
            }
            Ok(None) => absent += 1,
            Err(error) => {
                return Err(product_failure(
                    contract,
                    format!(
                        "live AHI product {} is unsafe or unreadable: {error}",
                        target.display()
                    ),
                ));
            }
        }
    }
    if absent == contract.install_products.len() {
        Ok(LiveSnapshot::Absent)
    } else if absent == 0 {
        Ok(LiveSnapshot::Complete(present))
    } else {
        Err(product_failure(
            contract,
            "live AHI product set is partial; refusing a mixed replacement",
        ))
    }
}

fn measure_file_with_mode(path: &Path) -> std::io::Result<(FileIdentity, Vec<u8>, u16)> {
    measure_optional_file_with_mode(path)?.ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::NotFound,
            format!("regular file '{}' disappeared", path.display()),
        )
    })
}

fn measure_optional_file_with_mode(
    path: &Path,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>, u16)>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("'{}' is not a regular non-symlink file", path.display()),
        ));
    }
    let measured = measure_regular_file(path)?.ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::NotFound,
            format!("regular file '{}' disappeared", path.display()),
        )
    })?;
    let after = fs::symlink_metadata(path)?;
    let before_identity = (before.dev(), before.ino());
    let after_identity = (after.dev(), after.ino());
    let before_mode = before.mode() & 0o7777;
    let after_mode = after.mode() & 0o7777;
    if !after.file_type().is_file()
        || after.file_type().is_symlink()
        || before_identity != after_identity
        || before_mode != after_mode
    {
        return Err(std::io::Error::other(format!(
            "regular file '{}' changed while measured",
            path.display()
        )));
    }
    let mode = u16::try_from(after_mode).map_err(|error| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!("cannot represent mode for '{}': {error}", path.display()),
        )
    })?;
    Ok(Some((measured.0, measured.1, mode)))
}

fn publish_products(
    contract: &Contract,
    baseline: &LiveSnapshot,
    products: &[PreparedProduct],
) -> AhiResult<()> {
    let journal =
        publication_journal_path(&contract.install_prefix, "ahi-install").map_err(|error| {
            product_failure(contract, format!("cannot derive AHI journal path: {error}"))
        })?;
    let mut transaction = DurableFileSet::new(journal).map_err(|error| {
        product_failure(
            contract,
            format!("cannot open durable AHI publication: {error}"),
        )
    })?;
    require_unchanged(contract, baseline)?;
    for product in products {
        transaction
            .stage_write_mode(&product.target, &product.contents, product.mode)
            .map_err(|error| {
                product_failure(
                    contract,
                    format!(
                        "cannot stage durable AHI product {}: {error}",
                        product.target.display()
                    ),
                )
            })?;
    }
    require_unchanged(contract, baseline)?;
    transaction.commit().map_err(|error| {
        product_failure(
            contract,
            format!("cannot commit durable AHI product set: {error}"),
        )
    })?;
    Ok(())
}

fn require_unchanged(contract: &Contract, expected: &LiveSnapshot) -> AhiResult<()> {
    let current = measure_live_set(contract)?;
    if &current == expected {
        Ok(())
    } else {
        Err(product_failure(
            contract,
            "live AHI product set changed while the private install was prepared",
        ))
    }
}

fn install_failure(
    contract: &Contract,
    message: impl Into<String>,
    process: Option<(&Path, ExitStatus)>,
) -> AhiFailure {
    failure(
        contract,
        DiagnosticCode::AhiBuild,
        DiagnosticStage::AhiBuild,
        message,
        process,
    )
}

fn product_failure(contract: &Contract, message: impl Into<String>) -> AhiFailure {
    failure(
        contract,
        DiagnosticCode::AhiProductValidation,
        DiagnosticStage::AhiProductValidation,
        message,
        None,
    )
}

fn failure(
    contract: &Contract,
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
    process: Option<(&Path, ExitStatus)>,
) -> AhiFailure {
    let mut context = DiagnosticContext {
        mode: Some(contract.mode.as_str().into()),
        target: Some(contract.target_triple.clone()),
        ..DiagnosticContext::default()
    };
    if let Some((tool, status)) = process {
        context.tool = Some(tool.display().to_string());
        context.exit_code = status.code();
        context.signal = status.signal();
    }
    AhiFailure::new(Diagnostic::error(code, stage, message).with_context(context))
}

#[cfg(test)]
#[path = "installation_tests.rs"]
mod tests;
