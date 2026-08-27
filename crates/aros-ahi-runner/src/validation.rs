//! Read-only, fail-closed audit performed before the AHI runner mutates state.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, SourceLocation};
use sha2::{Digest, Sha256};

use crate::contract::{Contract, ProductKind};
use crate::{AhiFailure, AhiResult};

impl Contract {
    pub fn validate_filesystem(&self, contract_source: &Path) -> AhiResult<()> {
        for (path, label) in [
            (&self.source_root, "source root"),
            (&self.build_root, "build root"),
            (&self.source_dir, "AHI source directory"),
            (&self.sdk_include, "SDK include directory"),
            (&self.gen_include, "generated include directory"),
        ] {
            require_directory(path, label, contract_source, self)?;
        }
        let os_include = self.sdk_include.join("aros/posixc");
        require_directory(
            &os_include,
            "SDK POSIX include directory",
            contract_source,
            self,
        )?;
        require_child(
            &self.sdk_include,
            &os_include,
            "SDK POSIX include directory",
            contract_source,
            self,
        )?;

        require_regular(
            &self.source_manifest,
            "source manifest",
            contract_source,
            self,
        )?;
        require_regular(
            &self.product_manifest,
            "product manifest",
            contract_source,
            self,
        )?;
        for (path, label) in [
            (&self.host_sfdc, "host sfdc"),
            (&self.host_perl, "host Perl"),
            (&self.host_flexcat, "host flexcat"),
            (&self.flexcat, "AHI flexcat adapter"),
            (&self.make, "GNU make"),
            (&self.cc, "AHI compiler adapter"),
            (&self.collect, "AROS collector"),
            (&self.assembler, "assembler"),
            (&self.ar, "AHI archiver adapter"),
            (&self.ranlib, "ranlib"),
            (&self.objcopy, "objcopy"),
            (&self.strip, "strip"),
            (&self.lld, "ld.lld"),
        ] {
            require_executable(path, label, contract_source, self)?;
        }

        for (path, label) in [
            (&self.source_dir, "source directory"),
            (&self.source_manifest, "source manifest"),
            (&self.product_manifest, "product manifest"),
        ] {
            require_child(&self.source_root, path, label, contract_source, self)?;
        }
        for (path, label) in [
            (&self.binary_dir, "binary directory"),
            (&self.install_prefix, "install prefix"),
            (&self.host_sfdc, "host sfdc"),
            (&self.host_flexcat, "host flexcat"),
            (&self.sdk_include, "SDK include"),
            (&self.gen_include, "generated include"),
        ] {
            require_child(&self.build_root, path, label, contract_source, self)?;
        }
        for (path, label) in [
            (&self.stage_source, "private source stage"),
            (&self.stage_build, "private build stage"),
            (&self.stage_linklibs, "private link-library stage"),
        ] {
            require_child(&self.binary_dir, path, label, contract_source, self)?;
            reject_symlink(path, label, contract_source, self)?;
        }
        reject_symlink(&self.binary_dir, "binary directory", contract_source, self)?;
        reject_symlink(
            &self.install_prefix,
            "install prefix",
            contract_source,
            self,
        )?;

        verify_digest(
            &self.source_manifest,
            &self.source_manifest_sha256,
            "source manifest",
            contract_source,
            self,
        )?;
        verify_digest(
            &self.product_manifest,
            &self.product_manifest_sha256,
            "product manifest",
            contract_source,
            self,
        )?;
        self.validate_source_manifest(contract_source)?;
        self.validate_product_manifest(contract_source)?;
        self.validate_inputs(contract_source)?;
        self.validate_feature_headers(contract_source)?;
        self.validate_dependencies(contract_source)?;
        self.validate_install_paths(contract_source)?;
        Ok(())
    }

    fn validate_source_manifest(&self, contract_source: &Path) -> AhiResult<()> {
        let text = read_utf8(
            &self.source_manifest,
            "source manifest",
            contract_source,
            self,
        )?;
        let mut paths = Vec::new();
        let mut unique = BTreeSet::new();
        for (offset, line) in text.lines().enumerate() {
            validate_relative(line, "source manifest", contract_source, offset + 1, self)?;
            if !unique.insert(line.to_owned()) {
                return Err(input_failure(
                    contract_source,
                    Some(offset + 1),
                    self,
                    format!("source manifest repeats {line:?}"),
                ));
            }
            paths.push(PathBuf::from(line));
        }
        if paths != self.input_relative {
            return Err(input_failure(
                contract_source,
                None,
                self,
                "contract input identity differs from the source manifest",
            ));
        }
        Ok(())
    }

    fn validate_product_manifest(&self, contract_source: &Path) -> AhiResult<()> {
        let text = read_utf8(
            &self.product_manifest,
            "product manifest",
            contract_source,
            self,
        )?;
        let mut paths = Vec::new();
        let mut kinds = Vec::new();
        let mut unique = BTreeSet::new();
        for (offset, line) in text.lines().enumerate() {
            let line_number = offset + 1;
            let Some((kind, relative)) = line.split_once("  ") else {
                return Err(input_failure(
                    &self.product_manifest,
                    Some(line_number),
                    self,
                    "malformed AHI product manifest entry",
                ));
            };
            let parsed_kind = match kind {
                "elf" => ProductKind::Elf,
                "data" => ProductKind::Data,
                "mode" => ProductKind::Mode,
                _ => {
                    return Err(input_failure(
                        &self.product_manifest,
                        Some(line_number),
                        self,
                        format!("unsupported product kind {kind:?}"),
                    ));
                }
            };
            validate_relative(
                relative,
                "product manifest",
                &self.product_manifest,
                line_number,
                self,
            )?;
            if !unique.insert(relative.to_owned()) {
                return Err(input_failure(
                    &self.product_manifest,
                    Some(line_number),
                    self,
                    format!("product manifest repeats {relative:?}"),
                ));
            }
            paths.push(PathBuf::from(relative));
            kinds.push(parsed_kind);
        }
        if paths != self.product_relative || kinds != self.product_kinds {
            return Err(input_failure(
                contract_source,
                None,
                self,
                "contract product identity differs from the product manifest",
            ));
        }
        Ok(())
    }

    fn validate_inputs(&self, contract_source: &Path) -> AhiResult<()> {
        for (relative, expected_hash) in self.input_relative.iter().zip(&self.input_sha256) {
            let input = self.source_dir.join(relative);
            require_regular(&input, "AHI source input", contract_source, self)?;
            require_child(
                &self.source_dir,
                &input,
                "AHI source input",
                contract_source,
                self,
            )?;
            verify_digest(
                &input,
                expected_hash,
                "AHI source input",
                contract_source,
                self,
            )?;
        }
        Ok(())
    }

    fn validate_feature_headers(&self, contract_source: &Path) -> AhiResult<()> {
        for header in &self.feature_headers {
            require_regular(header, "staged feature header", contract_source, self)?;
            require_nonempty(header, "staged feature header", contract_source, self)?;
            require_child(
                &self.build_root,
                header,
                "staged feature header",
                contract_source,
                self,
            )?;
        }
        Ok(())
    }

    fn validate_dependencies(&self, contract_source: &Path) -> AhiResult<()> {
        for dependency in &self.dependency_products {
            require_regular(dependency, "link-library dependency", contract_source, self)?;
            require_nonempty(dependency, "link-library dependency", contract_source, self)?;
            require_child(
                &self.build_root,
                dependency,
                "link-library dependency",
                contract_source,
                self,
            )?;
        }
        Ok(())
    }

    fn validate_install_paths(&self, contract_source: &Path) -> AhiResult<()> {
        for (relative, declared) in self.product_relative.iter().zip(&self.install_products) {
            let expected = self.install_prefix.join(relative);
            if declared != &expected {
                return Err(path_failure(
                    contract_source,
                    None,
                    self,
                    format!(
                        "install product {} differs from its manifest-derived path",
                        declared.display()
                    ),
                ));
            }
            let parent = declared.parent().ok_or_else(|| {
                path_failure(
                    contract_source,
                    None,
                    self,
                    "install product has no parent directory",
                )
            })?;
            if require_child(
                &self.install_prefix,
                parent,
                "install product parent",
                contract_source,
                self,
            )
            .is_err()
            {
                return Err(path_failure(
                    contract_source,
                    None,
                    self,
                    "install product escaped through a symlink",
                ));
            }
            reject_symlink(declared, "install product", contract_source, self)?;
        }
        Ok(())
    }
}

fn read_utf8(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<String> {
    fs::read_to_string(path).map_err(|error| {
        input_failure(
            contract_source,
            None,
            contract,
            format!("cannot read {label} as UTF-8: {error}"),
        )
    })
}

fn verify_digest(
    path: &Path,
    expected: &str,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    let mut file = fs::File::open(path).map_err(|error| {
        input_failure(
            contract_source,
            None,
            contract,
            format!("cannot open {label} {}: {error}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            input_failure(
                contract_source,
                None,
                contract,
                format!("cannot hash {label} {}: {error}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(input_failure(
            contract_source,
            None,
            contract,
            format!("{label} changed after configuration; rerun CMake"),
        ));
    }
    Ok(())
}

fn require_directory(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        path_failure(
            contract_source,
            None,
            contract,
            format!("cannot inspect {label} {}: {error}", path.display()),
        )
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(path_failure(
            contract_source,
            None,
            contract,
            format!("{label} must be a non-symlink directory"),
        ));
    }
    Ok(())
}

fn require_regular(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        input_failure(
            contract_source,
            None,
            contract,
            format!("cannot inspect {label} {}: {error}", path.display()),
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(input_failure(
            contract_source,
            None,
            contract,
            format!("{label} must be a regular non-symlink file"),
        ));
    }
    Ok(())
}

fn require_executable(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    let resolved = fs::canonicalize(path).map_err(|error| {
        path_failure(
            contract_source,
            None,
            contract,
            format!(
                "cannot resolve executable {label} {}: {error}",
                path.display()
            ),
        )
    })?;
    let metadata = fs::metadata(&resolved).map_err(|error| {
        path_failure(
            contract_source,
            None,
            contract,
            format!("cannot inspect executable {label}: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(path_failure(
            contract_source,
            None,
            contract,
            format!("{label} does not resolve to a regular file"),
        ));
    }
    let mode = metadata.permissions().mode();
    if mode & 0o111 == 0 {
        return Err(path_failure(
            contract_source,
            None,
            contract,
            format!("{label} is not executable"),
        ));
    }
    Ok(())
}

fn require_nonempty(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    if fs::metadata(path)
        .map_err(|error| {
            input_failure(
                contract_source,
                None,
                contract,
                format!("cannot inspect {label}: {error}"),
            )
        })?
        .len()
        == 0
    {
        return Err(input_failure(
            contract_source,
            None,
            contract,
            format!("{label} is empty"),
        ));
    }
    Ok(())
}

fn reject_symlink(
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(path_failure(
            contract_source,
            None,
            contract,
            format!("{label} escaped through a symlink"),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_failure(
            contract_source,
            None,
            contract,
            format!("cannot inspect {label}: {error}"),
        )),
    }
}

fn require_child(
    root: &Path,
    path: &Path,
    label: &str,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<()> {
    let root = resolve_existing_tail(root, contract_source, contract)?;
    let path = resolve_existing_tail(path, contract_source, contract)?;
    if path == root || path.strip_prefix(&root).is_err() {
        return Err(path_failure(
            contract_source,
            None,
            contract,
            format!("{label} escaped its owning tree"),
        ));
    }
    Ok(())
}

fn resolve_existing_tail(
    path: &Path,
    contract_source: &Path,
    contract: &Contract,
) -> AhiResult<PathBuf> {
    let mut candidate = path.to_path_buf();
    let mut tail = Vec::new();
    loop {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let mut resolved = fs::canonicalize(&candidate).map_err(|error| {
                    path_failure(
                        contract_source,
                        None,
                        contract,
                        format!("cannot resolve {}: {error}", path.display()),
                    )
                })?;
                for component in tail.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = candidate.file_name() else {
                    return Err(path_failure(
                        contract_source,
                        None,
                        contract,
                        format!("cannot resolve {}", path.display()),
                    ));
                };
                tail.push(name.to_os_string());
                if !candidate.pop() {
                    return Err(path_failure(
                        contract_source,
                        None,
                        contract,
                        format!("cannot resolve {}", path.display()),
                    ));
                }
            }
            Err(error) => {
                return Err(path_failure(
                    contract_source,
                    None,
                    contract,
                    format!("cannot inspect {}: {error}", path.display()),
                ));
            }
        }
    }
}

fn validate_relative(
    value: &str,
    label: &str,
    source: &Path,
    line: usize,
    contract: &Contract,
) -> AhiResult<()> {
    if value.is_empty()
        || Path::new(value).is_absolute()
        || value
            .split('/')
            .any(|component| component == "." || component == "..")
        || value
            .chars()
            .any(|character| matches!(character, ';' | '"' | '$' | '\\' | '\r' | '\n'))
        || value.contains("==]")
    {
        return Err(input_failure(
            source,
            Some(line),
            contract,
            format!("{label} contains unsafe relative path {value:?}"),
        ));
    }
    Ok(())
}

fn path_failure(
    source: &Path,
    line: Option<usize>,
    contract: &Contract,
    message: impl Into<String>,
) -> AhiFailure {
    failure(
        DiagnosticCode::AhiContractPath,
        DiagnosticStage::AhiContractValidation,
        source,
        line,
        contract,
        message,
    )
}

fn input_failure(
    source: &Path,
    line: Option<usize>,
    contract: &Contract,
    message: impl Into<String>,
) -> AhiFailure {
    failure(
        DiagnosticCode::AhiInputIntegrity,
        DiagnosticStage::AhiInputValidation,
        source,
        line,
        contract,
        message,
    )
}

fn failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    source: &Path,
    line: Option<usize>,
    contract: &Contract,
    message: impl Into<String>,
) -> AhiFailure {
    let mut location = SourceLocation::new(source.display().to_string());
    if let Some(line) = line {
        location = location.at(line, None);
    }
    AhiFailure::new(
        Diagnostic::error(code, stage, message)
            .with_location(location)
            .with_context(DiagnosticContext {
                mode: Some(contract.mode.as_str().into()),
                target: Some(contract.target_triple.clone()),
                ..DiagnosticContext::default()
            }),
    )
}
