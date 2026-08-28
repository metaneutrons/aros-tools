//! Strict parser and typed identity checks for the generated AHI contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{AhiFailure, AhiResult};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticStage, SourceLocation};

const MMAKE_ID: &str = "workbench-devs-AHI-subsystem";
const REQUIRED_FIELDS: &[&str] = &[
    "AHI_MMAKE_ID",
    "AHI_MODE",
    "AHI_SOURCE_ROOT",
    "AHI_BUILD_ROOT",
    "AHI_SOURCE_DIR",
    "AHI_SOURCE_MANIFEST",
    "AHI_SOURCE_MANIFEST_SHA256",
    "AHI_PRODUCT_MANIFEST",
    "AHI_PRODUCT_MANIFEST_SHA256",
    "AHI_BINARY_DIR",
    "AHI_STAGE_SOURCE",
    "AHI_STAGE_BUILD",
    "AHI_STAGE_LINKLIBS",
    "AHI_INSTALL_PREFIX",
    "AHI_HOST_SFDC",
    "AHI_HOST_PERL",
    "AHI_HOST_FLEXCAT",
    "AHI_FLEXCAT",
    "AHI_MAKE",
    "AHI_CC",
    "AHI_COLLECT",
    "AHI_AS",
    "AHI_AR",
    "AHI_RANLIB",
    "AHI_OBJCOPY",
    "AHI_STRIP",
    "AHI_LLD",
    "AHI_SDK_INCLUDE",
    "AHI_GEN_INCLUDE",
    "AHI_FEATURE_HEADERS",
    "AHI_BUILD_TRIPLET",
    "AHI_TARGET_TRIPLE",
    "AHI_ELF_CLASS",
    "AHI_ELF_MACHINE_HEX",
    "AHI_TARGET_CFLAGS",
    "AHI_TARGET_CPPFLAGS",
    "AHI_TARGET_ASFLAGS",
    "AHI_TARGET_LDFLAGS",
    "AHI_INPUT_RELATIVE",
    "AHI_INPUT_SHA256",
    "AHI_PRODUCT_RELATIVE",
    "AHI_PRODUCT_KINDS",
    "AHI_INSTALL_PRODUCTS",
    "AHI_DEPENDENCY_PRODUCTS",
];

const ABSOLUTE_PATH_FIELDS: &[&str] = &[
    "AHI_SOURCE_ROOT",
    "AHI_BUILD_ROOT",
    "AHI_SOURCE_DIR",
    "AHI_SOURCE_MANIFEST",
    "AHI_PRODUCT_MANIFEST",
    "AHI_BINARY_DIR",
    "AHI_STAGE_SOURCE",
    "AHI_STAGE_BUILD",
    "AHI_STAGE_LINKLIBS",
    "AHI_INSTALL_PREFIX",
    "AHI_HOST_SFDC",
    "AHI_HOST_PERL",
    "AHI_HOST_FLEXCAT",
    "AHI_FLEXCAT",
    "AHI_MAKE",
    "AHI_CC",
    "AHI_COLLECT",
    "AHI_AS",
    "AHI_AR",
    "AHI_RANLIB",
    "AHI_OBJCOPY",
    "AHI_STRIP",
    "AHI_LLD",
    "AHI_SDK_INCLUDE",
    "AHI_GEN_INCLUDE",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    X86_64,
    Arm,
    Aarch64,
}

impl Mode {
    fn parse(value: &str, source: &Path, line: usize) -> AhiResult<Self> {
        match value {
            "x86_64" => Ok(Self::X86_64),
            "arm" => Ok(Self::Arm),
            "aarch64" => Ok(Self::Aarch64),
            _ => Err(identity_failure(
                source,
                Some(line),
                format!("unsupported AHI_MODE {value:?}"),
            )),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm => "arm",
            Self::Aarch64 => "aarch64",
        }
    }

    const fn identity(self) -> (&'static str, &'static str, &'static str, usize, usize) {
        match self {
            Self::X86_64 => ("x86_64-unknown-aros", "02", "3e00", 73, 2),
            Self::Arm => ("arm-unknown-aros", "01", "2800", 85, 4),
            Self::Aarch64 => ("aarch64-unknown-aros", "02", "b700", 85, 4),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductKind {
    Elf,
    Data,
    Mode,
}

impl ProductKind {
    fn parse(value: &str, source: &Path, line: usize) -> AhiResult<Self> {
        match value {
            "elf" => Ok(Self::Elf),
            "data" => Ok(Self::Data),
            "mode" => Ok(Self::Mode),
            _ => Err(identity_failure(
                source,
                Some(line),
                format!("unsupported AHI product kind {value:?}"),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub mode: Mode,
    pub source_root: PathBuf,
    pub build_root: PathBuf,
    pub source_dir: PathBuf,
    pub source_manifest: PathBuf,
    pub source_manifest_sha256: String,
    pub product_manifest: PathBuf,
    pub product_manifest_sha256: String,
    pub binary_dir: PathBuf,
    pub stage_source: PathBuf,
    pub stage_build: PathBuf,
    pub stage_linklibs: PathBuf,
    pub install_prefix: PathBuf,
    pub host_sfdc: PathBuf,
    pub host_perl: PathBuf,
    pub host_flexcat: PathBuf,
    pub flexcat: PathBuf,
    pub make: PathBuf,
    pub cc: PathBuf,
    pub collect: PathBuf,
    pub assembler: PathBuf,
    pub ar: PathBuf,
    pub ranlib: PathBuf,
    pub objcopy: PathBuf,
    pub strip: PathBuf,
    pub lld: PathBuf,
    pub sdk_include: PathBuf,
    pub gen_include: PathBuf,
    pub feature_headers: Vec<PathBuf>,
    pub build_triplet: String,
    pub target_triple: String,
    pub elf_class: String,
    pub elf_machine_hex: String,
    pub target_cflags: Vec<String>,
    pub target_cppflags: Vec<String>,
    pub target_asflags: Vec<String>,
    pub target_ldflags: Vec<String>,
    pub input_relative: Vec<PathBuf>,
    pub input_sha256: Vec<String>,
    pub product_relative: Vec<PathBuf>,
    pub product_kinds: Vec<ProductKind>,
    pub install_products: Vec<PathBuf>,
    pub dependency_products: Vec<PathBuf>,
}

#[derive(Debug)]
struct RawValue {
    value: String,
    line: usize,
}

impl Contract {
    /// Load a strict AHI execution contract from a regular UTF-8 file.
    ///
    /// # Errors
    ///
    /// Returns a structured contract error for unsafe paths, I/O failures, or
    /// invalid contract syntax and values.
    pub fn load(path: &Path) -> AhiResult<Self> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            path_failure(path, None, format!("cannot inspect AHI contract: {error}"))
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(path_failure(
                path,
                None,
                "AHI contract must be a regular non-symlink file",
            ));
        }
        let text = fs::read_to_string(path).map_err(|error| {
            syntax_failure(
                path,
                None,
                format!("cannot read AHI contract as UTF-8: {error}"),
            )
        })?;
        Self::parse(&text, path)
    }

    /// Parse and validate one AHI execution contract.
    ///
    /// # Errors
    ///
    /// Returns a structured contract error for missing, unknown, duplicate,
    /// malformed, or internally inconsistent fields.
    pub fn parse(text: &str, source: &Path) -> AhiResult<Self> {
        let mut values = parse_assignments(text, source)?;
        let expected: BTreeSet<&str> = REQUIRED_FIELDS.iter().copied().collect();
        if let Some((unknown, raw)) = values
            .iter()
            .find(|(name, _)| !expected.contains(name.as_str()))
        {
            return Err(syntax_failure(
                source,
                Some(raw.line),
                format!("AHI contract contains unknown field {unknown}"),
            ));
        }
        for field in REQUIRED_FIELDS {
            if !values.contains_key(*field) {
                return Err(syntax_failure(
                    source,
                    None,
                    format!("AHI contract omits {field}"),
                ));
            }
        }

        validate_absolute_fields(&values, source)?;
        let mmake = take_scalar(&mut values, "AHI_MMAKE_ID", source)?;
        if mmake.value != MMAKE_ID {
            return Err(identity_failure(
                source,
                Some(mmake.line),
                "AHI contract differs from audited mmake identity",
            ));
        }
        let mode_raw = take_scalar(&mut values, "AHI_MODE", source)?;
        let mode = Mode::parse(&mode_raw.value, source, mode_raw.line)?;
        let (expected_triple, expected_class, expected_machine, product_count, feature_count) =
            mode.identity();

        let build_triplet = take_scalar(&mut values, "AHI_BUILD_TRIPLET", source)?;
        if !build_triplet.value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '+' | '-')
        }) {
            return Err(identity_failure(
                source,
                Some(build_triplet.line),
                "AHI_BUILD_TRIPLET contains unsupported characters",
            ));
        }
        let target_triple = take_scalar(&mut values, "AHI_TARGET_TRIPLE", source)?;
        let elf_class = take_scalar(&mut values, "AHI_ELF_CLASS", source)?;
        let elf_machine = take_scalar(&mut values, "AHI_ELF_MACHINE_HEX", source)?;
        if target_triple.value != expected_triple
            || elf_class.value != expected_class
            || elf_machine.value != expected_machine
        {
            return Err(identity_failure(
                source,
                Some(target_triple.line),
                "AHI contract differs from audited architecture identity",
            ));
        }

        let source_hash = take_scalar(&mut values, "AHI_SOURCE_MANIFEST_SHA256", source)?;
        validate_sha256(&source_hash, "AHI_SOURCE_MANIFEST_SHA256", source)?;
        let product_hash = take_scalar(&mut values, "AHI_PRODUCT_MANIFEST_SHA256", source)?;
        validate_sha256(&product_hash, "AHI_PRODUCT_MANIFEST_SHA256", source)?;

        let feature_headers = take_path_list(&mut values, "AHI_FEATURE_HEADERS", source, false)?;
        if feature_headers.len() != feature_count {
            return Err(identity_failure(
                source,
                None,
                format!(
                    "{} requires exactly {feature_count} feature headers",
                    mode.as_str()
                ),
            ));
        }
        let input_relative = take_path_list(&mut values, "AHI_INPUT_RELATIVE", source, true)?;
        let input_hashes = take_list(&mut values, "AHI_INPUT_SHA256", source)?;
        for hash in &input_hashes {
            validate_sha256(hash, "AHI_INPUT_SHA256", source)?;
        }
        if input_relative.len() != input_hashes.len() {
            return Err(identity_failure(
                source,
                None,
                "AHI input paths and hashes have different lengths",
            ));
        }
        let product_relative = take_path_list(&mut values, "AHI_PRODUCT_RELATIVE", source, true)?;
        let kinds_raw = take_list(&mut values, "AHI_PRODUCT_KINDS", source)?;
        let mut product_kinds = Vec::with_capacity(kinds_raw.len());
        for kind in kinds_raw {
            product_kinds.push(ProductKind::parse(&kind.value, source, kind.line)?);
        }
        let install_products = take_path_list(&mut values, "AHI_INSTALL_PRODUCTS", source, false)?;
        if product_relative.len() != product_count
            || product_kinds.len() != product_count
            || install_products.len() != product_count
        {
            return Err(identity_failure(
                source,
                None,
                format!(
                    "{} requires exactly {product_count} product paths, kinds and outputs",
                    mode.as_str()
                ),
            ));
        }
        let dependency_products =
            take_path_list(&mut values, "AHI_DEPENDENCY_PRODUCTS", source, false)?;
        if dependency_products.len() != 3 {
            return Err(identity_failure(
                source,
                None,
                "AHI contract requires exactly three link-library dependencies",
            ));
        }

        let contract = Self {
            mode,
            source_root: take_path(&mut values, "AHI_SOURCE_ROOT", source)?,
            build_root: take_path(&mut values, "AHI_BUILD_ROOT", source)?,
            source_dir: take_path(&mut values, "AHI_SOURCE_DIR", source)?,
            source_manifest: take_path(&mut values, "AHI_SOURCE_MANIFEST", source)?,
            source_manifest_sha256: source_hash.value,
            product_manifest: take_path(&mut values, "AHI_PRODUCT_MANIFEST", source)?,
            product_manifest_sha256: product_hash.value,
            binary_dir: take_path(&mut values, "AHI_BINARY_DIR", source)?,
            stage_source: take_path(&mut values, "AHI_STAGE_SOURCE", source)?,
            stage_build: take_path(&mut values, "AHI_STAGE_BUILD", source)?,
            stage_linklibs: take_path(&mut values, "AHI_STAGE_LINKLIBS", source)?,
            install_prefix: take_path(&mut values, "AHI_INSTALL_PREFIX", source)?,
            host_sfdc: take_path(&mut values, "AHI_HOST_SFDC", source)?,
            host_perl: take_path(&mut values, "AHI_HOST_PERL", source)?,
            host_flexcat: take_path(&mut values, "AHI_HOST_FLEXCAT", source)?,
            flexcat: take_path(&mut values, "AHI_FLEXCAT", source)?,
            make: take_path(&mut values, "AHI_MAKE", source)?,
            cc: take_path(&mut values, "AHI_CC", source)?,
            collect: take_path(&mut values, "AHI_COLLECT", source)?,
            assembler: take_path(&mut values, "AHI_AS", source)?,
            ar: take_path(&mut values, "AHI_AR", source)?,
            ranlib: take_path(&mut values, "AHI_RANLIB", source)?,
            objcopy: take_path(&mut values, "AHI_OBJCOPY", source)?,
            strip: take_path(&mut values, "AHI_STRIP", source)?,
            lld: take_path(&mut values, "AHI_LLD", source)?,
            sdk_include: take_path(&mut values, "AHI_SDK_INCLUDE", source)?,
            gen_include: take_path(&mut values, "AHI_GEN_INCLUDE", source)?,
            feature_headers,
            build_triplet: build_triplet.value,
            target_triple: target_triple.value,
            elf_class: elf_class.value,
            elf_machine_hex: elf_machine.value,
            target_cflags: take_string_list(&mut values, "AHI_TARGET_CFLAGS", source)?,
            target_cppflags: take_string_list(&mut values, "AHI_TARGET_CPPFLAGS", source)?,
            target_asflags: take_string_list(&mut values, "AHI_TARGET_ASFLAGS", source)?,
            target_ldflags: take_string_list(&mut values, "AHI_TARGET_LDFLAGS", source)?,
            input_relative,
            input_sha256: input_hashes.into_iter().map(|value| value.value).collect(),
            product_relative,
            product_kinds,
            install_products,
            dependency_products,
        };
        if !values.is_empty() {
            return Err(AhiFailure::new(Diagnostic::error(
                DiagnosticCode::AhiInternal,
                DiagnosticStage::Internal,
                "typed AHI parser did not consume every required field",
            )));
        }
        contract.validate_derived_identity(source)?;
        Ok(contract)
    }

    fn validate_derived_identity(&self, source: &Path) -> AhiResult<()> {
        let expected_source = self.source_root.join("workbench/devs/AHI");
        let expected_source_manifest = expected_source.join("ahi-build.inputs");
        let expected_product_manifest = self.source_root.join(format!(
            "cmake/manifests/ahi-{}.install",
            self.mode.as_str()
        ));
        let expected_binary = self.build_root.join(format!(
            "gen/configure/workbench/devs/AHI/{}",
            self.mode.as_str()
        ));
        let expected_feature_headers = match self.mode {
            Mode::X86_64 => vec![
                self.gen_include.join("libraries/mui.h"),
                self.sdk_include.join("asm/io.h"),
            ],
            Mode::Arm | Mode::Aarch64 => vec![
                self.gen_include.join("libraries/mui.h"),
                self.sdk_include.join("asm/io.h"),
                self.sdk_include.join("proto/dma.h"),
                self.sdk_include.join("proto/mbox.h"),
            ],
        };
        let identities = [
            (&self.source_dir, expected_source, "AHI_SOURCE_DIR"),
            (
                &self.source_manifest,
                expected_source_manifest,
                "AHI_SOURCE_MANIFEST",
            ),
            (
                &self.product_manifest,
                expected_product_manifest,
                "AHI_PRODUCT_MANIFEST",
            ),
            (&self.binary_dir, expected_binary, "AHI_BINARY_DIR"),
            (
                &self.stage_source,
                self.binary_dir.join("source"),
                "AHI_STAGE_SOURCE",
            ),
            (
                &self.stage_build,
                self.binary_dir.join("build"),
                "AHI_STAGE_BUILD",
            ),
            (
                &self.stage_linklibs,
                self.binary_dir.join("linklibs"),
                "AHI_STAGE_LINKLIBS",
            ),
            (
                &self.install_prefix,
                self.build_root.join("SYS"),
                "AHI_INSTALL_PREFIX",
            ),
            (
                &self.host_sfdc,
                self.build_root.join("hosttools/sfdc"),
                "AHI_HOST_SFDC",
            ),
            (
                &self.host_flexcat,
                self.build_root.join("hosttools/flexcat"),
                "AHI_HOST_FLEXCAT",
            ),
            (
                &self.flexcat,
                self.binary_dir.join("ahi-flexcat"),
                "AHI_FLEXCAT",
            ),
            (&self.cc, self.binary_dir.join("ahi-cc"), "AHI_CC"),
            (&self.ar, self.binary_dir.join("ahi-ar"), "AHI_AR"),
            (
                &self.sdk_include,
                self.build_root.join("SDK/include"),
                "AHI_SDK_INCLUDE",
            ),
            (
                &self.gen_include,
                self.build_root.join("GENINCDIR"),
                "AHI_GEN_INCLUDE",
            ),
        ];
        for (actual, expected, name) in identities {
            if actual != &expected {
                return Err(identity_failure(
                    source,
                    None,
                    format!("{name} differs from its audited derived path"),
                ));
            }
        }
        if self.feature_headers != expected_feature_headers {
            return Err(identity_failure(
                source,
                None,
                "AHI_FEATURE_HEADERS differ from the audited architecture profile",
            ));
        }
        if self.lld.file_name().and_then(|name| name.to_str()) != Some("ld.lld") {
            return Err(identity_failure(
                source,
                None,
                "AHI_LLD must invoke ld.lld by that exact name",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn product_count(&self) -> usize {
        self.product_relative.len()
    }

    #[must_use]
    pub const fn input_count(&self) -> usize {
        self.input_relative.len()
    }
}

fn parse_assignments(text: &str, source: &Path) -> AhiResult<BTreeMap<String, RawValue>> {
    let mut values = BTreeMap::new();
    if text.is_empty() {
        return Err(syntax_failure(source, None, "AHI contract is empty"));
    }
    for (offset, line) in text.lines().enumerate() {
        let line_number = offset + 1;
        let Some(body) = line.strip_prefix("set(") else {
            return Err(syntax_failure(
                source,
                Some(line_number),
                "AHI contract line is not an exact set assignment",
            ));
        };
        let Some(body) = body.strip_suffix("]==])") else {
            return Err(syntax_failure(
                source,
                Some(line_number),
                "AHI contract assignment has an unsupported delimiter",
            ));
        };
        let Some((name, value)) = body.split_once(" [==[") else {
            return Err(syntax_failure(
                source,
                Some(line_number),
                "AHI contract assignment has an unsupported shape",
            ));
        };
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(syntax_failure(
                source,
                Some(line_number),
                format!("AHI contract has unsafe field name {name:?}"),
            ));
        }
        if value.is_empty() {
            return Err(syntax_failure(
                source,
                Some(line_number),
                format!("AHI contract field {name} is empty"),
            ));
        }
        if values
            .insert(
                name.to_owned(),
                RawValue {
                    value: value.to_owned(),
                    line: line_number,
                },
            )
            .is_some()
        {
            return Err(syntax_failure(
                source,
                Some(line_number),
                format!("AHI contract repeats field {name}"),
            ));
        }
    }
    Ok(values)
}

fn validate_absolute_fields(values: &BTreeMap<String, RawValue>, source: &Path) -> AhiResult<()> {
    for field in ABSOLUTE_PATH_FIELDS {
        let raw = values
            .get(*field)
            .ok_or_else(|| syntax_failure(source, None, format!("AHI contract omits {field}")))?;
        validate_absolute_path(&raw.value, field, source, raw.line)?;
    }
    Ok(())
}

fn validate_absolute_path(value: &str, field: &str, source: &Path, line: usize) -> AhiResult<()> {
    if !Path::new(value).is_absolute()
        || value.chars().any(|character| {
            matches!(character, ';' | '"' | '$' | '\\' | '\r' | '\n')
                || character.is_ascii_whitespace()
        })
        || value.contains("==]")
    {
        return Err(path_failure(
            source,
            Some(line),
            format!("{field} is not a safe absolute configure/Make path"),
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str, field: &str, source: &Path, line: usize) -> AhiResult<()> {
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
        return Err(path_failure(
            source,
            Some(line),
            format!("{field} contains unsafe relative path {value:?}"),
        ));
    }
    Ok(())
}

fn validate_sha256(value: &RawValue, field: &str, source: &Path) -> AhiResult<()> {
    if value.value.len() != 64
        || !value
            .value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(identity_failure(
            source,
            Some(value.line),
            format!("{field} is not a lowercase SHA-256 digest"),
        ));
    }
    Ok(())
}

fn take_scalar(
    values: &mut BTreeMap<String, RawValue>,
    field: &str,
    source: &Path,
) -> AhiResult<RawValue> {
    values
        .remove(field)
        .ok_or_else(|| syntax_failure(source, None, format!("AHI contract omits {field}")))
}

fn take_path(
    values: &mut BTreeMap<String, RawValue>,
    field: &str,
    source: &Path,
) -> AhiResult<PathBuf> {
    Ok(PathBuf::from(take_scalar(values, field, source)?.value))
}

fn take_list(
    values: &mut BTreeMap<String, RawValue>,
    field: &str,
    source: &Path,
) -> AhiResult<Vec<RawValue>> {
    let raw = take_scalar(values, field, source)?;
    let line = raw.line;
    let members: Vec<_> = raw
        .value
        .split(';')
        .map(|value| RawValue {
            value: value.to_owned(),
            line,
        })
        .collect();
    if members.iter().any(|member| member.value.is_empty()) {
        return Err(syntax_failure(
            source,
            Some(line),
            format!("{field} contains an empty list member"),
        ));
    }
    Ok(members)
}

fn take_string_list(
    values: &mut BTreeMap<String, RawValue>,
    field: &str,
    source: &Path,
) -> AhiResult<Vec<String>> {
    Ok(take_list(values, field, source)?
        .into_iter()
        .map(|value| value.value)
        .collect())
}

fn take_path_list(
    values: &mut BTreeMap<String, RawValue>,
    field: &str,
    source: &Path,
    relative: bool,
) -> AhiResult<Vec<PathBuf>> {
    let members = take_list(values, field, source)?;
    let mut unique = BTreeSet::new();
    let mut paths = Vec::with_capacity(members.len());
    for member in members {
        if relative {
            validate_relative_path(&member.value, field, source, member.line)?;
        } else {
            validate_absolute_path(&member.value, field, source, member.line)?;
        }
        if !unique.insert(member.value.clone()) {
            return Err(path_failure(
                source,
                Some(member.line),
                format!("{field} contains duplicate path {:?}", member.value),
            ));
        }
        paths.push(PathBuf::from(member.value));
    }
    Ok(paths)
}

fn syntax_failure(source: &Path, line: Option<usize>, message: impl Into<String>) -> AhiFailure {
    failure(
        DiagnosticCode::AhiContractSyntax,
        DiagnosticStage::AhiContractParsing,
        source,
        line,
        message,
    )
}

fn identity_failure(source: &Path, line: Option<usize>, message: impl Into<String>) -> AhiFailure {
    failure(
        DiagnosticCode::AhiContractIdentity,
        DiagnosticStage::AhiContractValidation,
        source,
        line,
        message,
    )
}

fn path_failure(source: &Path, line: Option<usize>, message: impl Into<String>) -> AhiFailure {
    failure(
        DiagnosticCode::AhiContractPath,
        DiagnosticStage::AhiContractValidation,
        source,
        line,
        message,
    )
}

fn failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    source: &Path,
    line: Option<usize>,
    message: impl Into<String>,
) -> AhiFailure {
    let mut location = SourceLocation::new(source.display().to_string());
    if let Some(line) = line {
        location = location.at(line, None);
    }
    AhiFailure::new(Diagnostic::error(code, stage, message).with_location(location))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assignment(name: &str, value: &str) -> String {
        format!("set({name} [==[{value}]==])\n")
    }

    fn valid_x86_64_contract() -> String {
        let products: Vec<_> = (0..73).map(|index| format!("product-{index}")).collect();
        let install: Vec<_> = products
            .iter()
            .map(|product| format!("/build/SYS/{product}"))
            .collect();
        let kinds = vec!["data"; products.len()];
        let fields = [
            ("AHI_MMAKE_ID", MMAKE_ID.to_owned()),
            ("AHI_MODE", "x86_64".to_owned()),
            ("AHI_SOURCE_ROOT", "/source".to_owned()),
            ("AHI_BUILD_ROOT", "/build".to_owned()),
            ("AHI_SOURCE_DIR", "/source/workbench/devs/AHI".to_owned()),
            (
                "AHI_SOURCE_MANIFEST",
                "/source/workbench/devs/AHI/ahi-build.inputs".to_owned(),
            ),
            ("AHI_SOURCE_MANIFEST_SHA256", "0".repeat(64)),
            (
                "AHI_PRODUCT_MANIFEST",
                "/source/cmake/manifests/ahi-x86_64.install".to_owned(),
            ),
            ("AHI_PRODUCT_MANIFEST_SHA256", "1".repeat(64)),
            (
                "AHI_BINARY_DIR",
                "/build/gen/configure/workbench/devs/AHI/x86_64".to_owned(),
            ),
            (
                "AHI_STAGE_SOURCE",
                "/build/gen/configure/workbench/devs/AHI/x86_64/source".to_owned(),
            ),
            (
                "AHI_STAGE_BUILD",
                "/build/gen/configure/workbench/devs/AHI/x86_64/build".to_owned(),
            ),
            (
                "AHI_STAGE_LINKLIBS",
                "/build/gen/configure/workbench/devs/AHI/x86_64/linklibs".to_owned(),
            ),
            ("AHI_INSTALL_PREFIX", "/build/SYS".to_owned()),
            ("AHI_HOST_SFDC", "/build/hosttools/sfdc".to_owned()),
            ("AHI_HOST_PERL", "/usr/bin/perl".to_owned()),
            ("AHI_HOST_FLEXCAT", "/build/hosttools/flexcat".to_owned()),
            (
                "AHI_FLEXCAT",
                "/build/gen/configure/workbench/devs/AHI/x86_64/ahi-flexcat".to_owned(),
            ),
            ("AHI_MAKE", "/usr/bin/make".to_owned()),
            (
                "AHI_CC",
                "/build/gen/configure/workbench/devs/AHI/x86_64/ahi-cc".to_owned(),
            ),
            ("AHI_COLLECT", "/tools/aros-collect".to_owned()),
            ("AHI_AS", "/tools/clang".to_owned()),
            (
                "AHI_AR",
                "/build/gen/configure/workbench/devs/AHI/x86_64/ahi-ar".to_owned(),
            ),
            ("AHI_RANLIB", "/tools/llvm-ranlib".to_owned()),
            ("AHI_OBJCOPY", "/tools/llvm-objcopy".to_owned()),
            ("AHI_STRIP", "/tools/llvm-strip".to_owned()),
            ("AHI_LLD", "/tools/ld.lld".to_owned()),
            ("AHI_SDK_INCLUDE", "/build/SDK/include".to_owned()),
            ("AHI_GEN_INCLUDE", "/build/GENINCDIR".to_owned()),
            (
                "AHI_FEATURE_HEADERS",
                "/build/GENINCDIR/libraries/mui.h;/build/SDK/include/asm/io.h".to_owned(),
            ),
            ("AHI_BUILD_TRIPLET", "x86_64-unknown-linux".to_owned()),
            ("AHI_TARGET_TRIPLE", "x86_64-unknown-aros".to_owned()),
            ("AHI_ELF_CLASS", "02".to_owned()),
            ("AHI_ELF_MACHINE_HEX", "3e00".to_owned()),
            (
                "AHI_TARGET_CFLAGS",
                "--target=x86_64-unknown-elf".to_owned(),
            ),
            ("AHI_TARGET_CPPFLAGS", "-I/build/SDK/include".to_owned()),
            (
                "AHI_TARGET_ASFLAGS",
                "--target=x86_64-unknown-elf".to_owned(),
            ),
            ("AHI_TARGET_LDFLAGS", "-Wl,-r".to_owned()),
            ("AHI_INPUT_RELATIVE", "configure".to_owned()),
            ("AHI_INPUT_SHA256", "2".repeat(64)),
            ("AHI_PRODUCT_RELATIVE", products.join(";")),
            ("AHI_PRODUCT_KINDS", kinds.join(";")),
            ("AHI_INSTALL_PRODUCTS", install.join(";")),
            (
                "AHI_DEPENDENCY_PRODUCTS",
                "/build/libamiga.a;/build/libm.a;/build/libmui.a".to_owned(),
            ),
        ];
        fields
            .into_iter()
            .map(|(name, value)| assignment(name, &value))
            .collect()
    }

    #[test]
    fn exact_typed_x86_64_contract_is_accepted() {
        let contract =
            Contract::parse(&valid_x86_64_contract(), Path::new("contract.cmake")).unwrap();
        assert_eq!(contract.mode, Mode::X86_64);
        assert_eq!(contract.product_count(), 73);
        assert_eq!(contract.input_count(), 1);
    }

    #[test]
    fn unknown_field_is_rejected_even_when_it_is_valid_cmake() {
        let text = valid_x86_64_contract() + &assignment("AHI_FUTURE_ESCAPE", "enabled");
        let error = Contract::parse(&text, Path::new("contract.cmake")).unwrap_err();
        assert_eq!(error.diagnostic().code, DiagnosticCode::AhiContractSyntax);
        assert!(error.diagnostic().message.contains("unknown field"));
    }

    #[test]
    fn duplicate_field_is_a_stable_syntax_error() {
        let error = Contract::parse(
            "set(AHI_MODE [==[x86_64]==])\nset(AHI_MODE [==[x86_64]==])\n",
            Path::new("contract.cmake"),
        )
        .unwrap_err();
        assert_eq!(error.diagnostic().code, DiagnosticCode::AhiContractSyntax);
        assert_eq!(error.diagnostic().location.as_ref().unwrap().line, Some(2));
    }

    #[test]
    fn arbitrary_cmake_is_rejected_instead_of_executed() {
        let error = Contract::parse(
            "message(FATAL_ERROR injected)\n",
            Path::new("contract.cmake"),
        )
        .unwrap_err();
        assert_eq!(error.diagnostic().code, DiagnosticCode::AhiContractSyntax);
    }
}
