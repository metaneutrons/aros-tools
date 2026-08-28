//! Native Rust execution engine for the closed AHI capability.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, Output};
use std::time::SystemTime;

use aros_common::{run_output, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};
use sha2::{Digest, Sha256};

use crate::contract::{Contract, Mode, ProductKind};
use crate::observability::{LogLevel, Logger};
use crate::{AhiFailure, AhiResult};

const SCRIPT_NAMES: &[&str] = &["configure", "config.guess", "config.sub", "install-sh"];
const DEPENDENCY_ALIASES: &[&str] = &["libamiga.a", "libm.a", "libmui.a"];

/// Execute one already parsed and filesystem-validated AHI contract.
///
/// # Errors
///
/// Returns a structured failure when staging, generation, compilation,
/// installation, product validation, or logging fails.
pub fn run(contract: &Contract, logger: &mut Logger) -> AhiResult<()> {
    let context = context(contract);
    logger.event(
        LogLevel::Info,
        "staging.start",
        "AHI private staging started",
        &context,
    )?;
    stage(contract)?;
    logger.event(
        LogLevel::Info,
        "staging.complete",
        "AHI private staging completed",
        &context,
    )?;

    check_sfdc(contract)?;
    logger.event(
        LogLevel::Info,
        "configure.start",
        "AHI configure started",
        &context,
    )?;
    configure(contract)?;
    validate_configuration(contract)?;
    logger.event(
        LogLevel::Info,
        "configure.complete",
        "AHI configure completed",
        &context,
    )?;

    logger.event(
        LogLevel::Info,
        "build.start",
        "AHI include preparation and install started",
        &context,
    )?;
    build(contract)?;
    logger.event(
        LogLevel::Info,
        "build.complete",
        "AHI include preparation and install completed",
        &context,
    )?;

    validate_products(contract)?;
    audit_sources(contract)?;
    logger.event(
        LogLevel::Info,
        "products.validated",
        "AHI products and source immutability validated",
        &context,
    )?;
    Ok(())
}

fn stage(contract: &Contract) -> AhiResult<()> {
    for path in [
        &contract.stage_source,
        &contract.stage_build,
        &contract.stage_linklibs,
    ] {
        reject_symlink(path, contract, DiagnosticStage::AhiStaging)?;
        if path.exists() {
            fs::remove_dir_all(path).map_err(|error| {
                stage_failure(
                    contract,
                    format!("cannot remove private stage {}: {error}", path.display()),
                )
            })?;
        }
    }
    for path in [
        &contract.stage_source,
        &contract.stage_build,
        &contract.stage_linklibs,
    ] {
        fs::create_dir_all(path).map_err(|error| {
            stage_failure(
                contract,
                format!("cannot create private stage {}: {error}", path.display()),
            )
        })?;
        reject_symlink(path, contract, DiagnosticStage::AhiStaging)?;
    }

    for relative in &contract.input_relative {
        let source = contract.source_dir.join(relative);
        let staged = contract.stage_source.join(relative);
        let parent = staged.parent().ok_or_else(|| {
            stage_failure(
                contract,
                format!("staged source has no parent: {}", staged.display()),
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            stage_failure(
                contract,
                format!("cannot create staged source parent: {error}"),
            )
        })?;
        reject_symlink(&staged, contract, DiagnosticStage::AhiStaging)?;
        fs::copy(&source, &staged).map_err(|error| {
            stage_failure(
                contract,
                format!("cannot stage source {}: {error}", relative.display()),
            )
        })?;
    }
    for name in SCRIPT_NAMES {
        let script = contract.stage_source.join(name);
        if script.exists() {
            let mut permissions = fs::metadata(&script)
                .map_err(|error| {
                    stage_failure(contract, format!("cannot inspect staged script: {error}"))
                })?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).map_err(|error| {
                stage_failure(
                    contract,
                    format!("cannot make staged script executable: {error}"),
                )
            })?;
        }
    }
    for name in ["configure", "config.h.in"] {
        let path = contract.stage_source.join(name);
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|error| stage_failure(contract, format!("cannot touch {name}: {error}")))?;
        let times = fs::FileTimes::new().set_modified(SystemTime::now());
        file.set_times(times)
            .map_err(|error| stage_failure(contract, format!("cannot touch {name}: {error}")))?;
    }
    for (dependency, alias) in contract.dependency_products.iter().zip(DEPENDENCY_ALIASES) {
        fs::copy(dependency, contract.stage_linklibs.join(alias)).map_err(|error| {
            stage_failure(
                contract,
                format!("cannot stage dependency {alias}: {error}"),
            )
        })?;
    }
    fs::write(
        contract.stage_build.join("config.cache"),
        config_cache(contract.mode),
    )
    .map_err(|error| stage_failure(contract, format!("cannot write config.cache: {error}")))?;
    Ok(())
}

fn config_cache(mode: Mode) -> String {
    let mut cache = concat!(
        "ac_cv_search_NewList='-lamiga'\n",
        "ac_cv_search_floor='-lm'\n",
        "ac_cv_search_IntuitionBase=no\n",
        "ac_cv_search_LayoutBase=no\n",
        "ac_cv_search_MUI_NewObject='-lmui'\n",
        "ac_cv_header_asm_io_h=yes\n",
        "ac_cv_header_libraries_openpci_h=no\n",
        "ac_cv_header_proto_oss_h=no\n",
        "ac_cv_lib_alsa_bridge_ALSA_Init=no\n",
        "ac_cv_lib_pulseaudio_bridge_PULSEA_Init=no\n",
        "ac_cv_lib_WASAPI_bridge_WASAPI_Init=no\n",
        "ac_cv_c_const=yes\n",
        "ac_cv_c_inline=yes\n",
        "ac_cv_c_bigendian=no\n"
    )
    .to_owned();
    cache.push_str(if mode == Mode::X86_64 {
        "ac_cv_header_proto_dma_h=no\n"
    } else {
        "ac_cv_header_proto_dma_h=yes\n"
    });
    cache
}

fn check_sfdc(contract: &Contract) -> AhiResult<()> {
    let mut command = closed_command(&contract.host_perl);
    command.arg("-c").arg(&contract.host_sfdc);
    let output = run_output(&mut command)
        .map_err(|error| {
            configure_failure(contract, format!("cannot start HOST_PERL: {error}"), None)
        })?
        .output;
    require_success(
        &output,
        contract,
        &contract.host_perl,
        DiagnosticCode::AhiConfigure,
        DiagnosticStage::AhiConfigure,
        "HOST_SFDC failed HOST_PERL",
    )
}

fn configure(contract: &Contract) -> AhiResult<()> {
    let mut command = closed_build_command(contract, &contract.stage_source.join("configure"));
    let os_include = contract.sdk_include.join("aros/posixc");
    command
        .current_dir(&contract.stage_build)
        .arg(format!("--build={}", contract.build_triplet))
        .arg(format!("--host={}", contract.target_triple))
        .arg(format!("--target={}", contract.target_triple))
        .arg(format!(
            "--cache-file={}",
            contract.stage_build.join("config.cache").display()
        ))
        .arg(format!("--prefix={}", contract.install_prefix.display()))
        .arg(format!("--bindir={}", contract.install_prefix.display()))
        .arg(format!("--sbindir={}", contract.install_prefix.display()))
        .arg(format!(
            "--libdir={}/Libs",
            contract.install_prefix.display()
        ))
        .arg(format!(
            "--includedir={}/Developer/include",
            contract.install_prefix.display()
        ))
        .arg(format!(
            "--oldincludedir={}/Developer/include",
            contract.install_prefix.display()
        ))
        .arg(format!("--with-os-includedir={}", os_include.display()))
        .arg(format!(
            "--with-target-cflags={}",
            contract.target_cflags.join(" ")
        ))
        .arg(format!(
            "--with-target-cppflags={}",
            contract.target_cppflags.join(" ")
        ))
        .arg(format!(
            "--with-target-asflags={}",
            contract.target_asflags.join(" ")
        ))
        .arg(format!(
            "--with-target-ldflags={}",
            contract.target_ldflags.join(" ")
        ))
        .arg("--with-target-optflags=-O2");
    let output = run_output(&mut command)
        .map_err(|error| {
            configure_failure(
                contract,
                format!("cannot start AHI configure: {error}"),
                None,
            )
        })?
        .output;
    require_success(
        &output,
        contract,
        &contract.stage_source.join("configure"),
        DiagnosticCode::AhiConfigure,
        DiagnosticStage::AhiConfigure,
        "AHI configure failed",
    )
}

fn validate_configuration(contract: &Contract) -> AhiResult<()> {
    let config = read_stage_text(contract, "config.h")?;
    for define in ["HAVE_ASM_IO_H", "HAVE_LIBMUI"] {
        if !config.contains(&format!("#define {define} 1")) {
            return Err(configure_failure(
                contract,
                format!("configured feature {define} differs from contract"),
                None,
            ));
        }
    }
    let drivers = read_stage_text(contract, "Drivers/Makefile")?;
    if !drivers.contains("HAVE_ASMIO      = 1") {
        return Err(configure_failure(
            contract,
            "configured driver feature HAVE_ASMIO differs from contract",
            None,
        ));
    }
    let has_dma = config.contains("#define HAVE_PROTO_DMA_H 1");
    let drivers_have_dma = drivers.contains("HAVE_DMA_H      = 1");
    if (contract.mode == Mode::X86_64 && (has_dma || drivers_have_dma))
        || (contract.mode != Mode::X86_64 && (!has_dma || !drivers_have_dma))
    {
        return Err(configure_failure(
            contract,
            "configured DMA feature differs from architecture contract",
            None,
        ));
    }
    for unexpected in ["HAVE_LIBRARIES_OPENPCI_H", "HAVE_PROTO_OSS_H"] {
        if config.contains(&format!("#define {unexpected} 1")) {
            return Err(configure_failure(
                contract,
                format!("configured unexpected feature {unexpected}"),
                None,
            ));
        }
    }
    let configured = format!(
        "{}{}",
        read_stage_text(contract, "Include/Makefile")?,
        read_stage_text(contract, "Device/Makefile")?
    );
    for bound in [&contract.host_sfdc, &contract.flexcat, &contract.cc] {
        if !configured.contains(&bound.display().to_string()) {
            return Err(configure_failure(
                contract,
                format!("configured Makefiles did not bind {}", bound.display()),
                None,
            ));
        }
    }
    Ok(())
}

fn build(contract: &Contract) -> AhiResult<()> {
    let mut include = closed_build_command(contract, &contract.make);
    include
        .arg("-C")
        .arg(contract.stage_build.join("Include"))
        .arg("gcc-include");
    let output = run_output(&mut include)
        .map_err(|error| {
            build_failure(
                contract,
                format!("cannot start gcc-include preparation: {error}"),
                None,
            )
        })?
        .output;
    require_success(
        &output,
        contract,
        &contract.make,
        DiagnosticCode::AhiBuild,
        DiagnosticStage::AhiBuild,
        "AHI gcc-include preparation failed",
    )?;
    for relative in [
        "Include/gcc/devices/ahi.h",
        "Include/gcc/libraries/ahi_sub.h",
        "Include/gcc/proto/ahi.h",
    ] {
        require_regular_nonempty(
            &contract.stage_build.join(relative),
            contract,
            DiagnosticCode::AhiBuild,
            DiagnosticStage::AhiBuild,
            "generated AHI include",
        )?;
    }
    let mut install = closed_build_command(contract, &contract.make);
    install.arg("-C").arg(&contract.stage_build).arg("install");
    let output = run_output(&mut install)
        .map_err(|error| {
            build_failure(
                contract,
                format!("cannot start AHI make install: {error}"),
                None,
            )
        })?
        .output;
    require_success(
        &output,
        contract,
        &contract.make,
        DiagnosticCode::AhiBuild,
        DiagnosticStage::AhiBuild,
        "AHI make install failed",
    )
}

fn validate_products(contract: &Contract) -> AhiResult<()> {
    reject_symlink(
        &contract.install_prefix,
        contract,
        DiagnosticStage::AhiProductValidation,
    )?;
    for ((relative, kind), product) in contract
        .product_relative
        .iter()
        .zip(&contract.product_kinds)
        .zip(&contract.install_products)
    {
        let expected = contract.install_prefix.join(relative);
        if product != &expected {
            return Err(product_failure(
                contract,
                format!("installed product path differs for {}", relative.display()),
            ));
        }
        require_regular_nonempty(
            product,
            contract,
            DiagnosticCode::AhiProductValidation,
            DiagnosticStage::AhiProductValidation,
            "installed AHI product",
        )?;
        if *kind == ProductKind::Elf {
            validate_elf(contract, product, relative)?;
        }
    }
    Ok(())
}

fn validate_elf(contract: &Contract, product: &Path, relative: &Path) -> AhiResult<()> {
    let mut file = fs::File::open(product).map_err(|error| {
        product_failure(contract, format!("cannot inspect ELF product: {error}"))
    })?;
    let mut header = [0_u8; 20];
    file.read_exact(&mut header).map_err(|error| {
        product_failure(
            contract,
            format!("ELF product {} is truncated: {error}", relative.display()),
        )
    })?;
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
    if header[0..4] != *b"\x7fELF"
        || header[4] != expected_class
        || header[18..20] != expected_machine
    {
        return Err(product_failure(
            contract,
            format!("ELF product {} has wrong format", relative.display()),
        ));
    }
    Ok(())
}

fn audit_sources(contract: &Contract) -> AhiResult<()> {
    for (relative, expected) in contract.input_relative.iter().zip(&contract.input_sha256) {
        let actual = digest(&contract.source_dir.join(relative)).map_err(|error| {
            source_failure(
                contract,
                format!("cannot re-audit source {}: {error}", relative.display()),
            )
        })?;
        if &actual != expected {
            return Err(source_failure(
                contract,
                format!("AHI runner modified source input {}", relative.display()),
            ));
        }
    }
    Ok(())
}

fn closed_command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C");
    command
}

fn closed_build_command(contract: &Contract, program: &Path) -> Command {
    let mut command = closed_command(program);
    command.envs(build_environment(contract));
    command
}

fn build_environment(contract: &Contract) -> Vec<(OsString, OsString)> {
    let values: Vec<(&str, OsString)> = vec![
        ("SHELL", "/bin/sh".into()),
        ("CC", contract.cc.as_os_str().to_owned()),
        ("AS", contract.assembler.as_os_str().to_owned()),
        ("AR", contract.ar.as_os_str().to_owned()),
        ("RANLIB", contract.ranlib.as_os_str().to_owned()),
        ("OBJCOPY", contract.objcopy.as_os_str().to_owned()),
        ("STRIP", contract.strip.as_os_str().to_owned()),
        ("MAKE", contract.make.as_os_str().to_owned()),
        ("SFDC", contract.host_sfdc.as_os_str().to_owned()),
        ("FLEXCAT", contract.flexcat.as_os_str().to_owned()),
        ("PERL", contract.host_perl.as_os_str().to_owned()),
        ("RM", "/bin/rm".into()),
        ("INSTALL", "/usr/bin/install".into()),
        ("ROBODOC", "/usr/bin/false".into()),
        ("LHA", "/usr/bin/false".into()),
        ("AHI_MODE", contract.mode.as_str().into()),
        (
            "AHI_INSTALL_PREFIX",
            contract.install_prefix.as_os_str().to_owned(),
        ),
        (
            "AHI_PRODUCT_MANIFEST",
            contract.product_manifest.as_os_str().to_owned(),
        ),
        ("MFLAGS", OsString::new()),
        ("CDPATH", OsString::new()),
        ("ENV", OsString::new()),
        ("BASH_ENV", OsString::new()),
        ("CPATH", OsString::new()),
        ("C_INCLUDE_PATH", OsString::new()),
        ("CPLUS_INCLUDE_PATH", OsString::new()),
        ("LIBRARY_PATH", OsString::new()),
        ("SDKROOT", OsString::new()),
        ("PKG_CONFIG_PATH", OsString::new()),
        ("PKG_CONFIG_LIBDIR", OsString::new()),
        ("PKG_CONFIG_SYSROOT_DIR", OsString::new()),
    ];
    values
        .into_iter()
        .map(|(name, value)| (OsString::from(name), value))
        .collect()
}

fn read_stage_text(contract: &Contract, relative: &str) -> AhiResult<String> {
    let path = contract.stage_build.join(relative);
    fs::read_to_string(&path).map_err(|error| {
        configure_failure(
            contract,
            format!("cannot read configured {}: {error}", path.display()),
            None,
        )
    })
}

fn require_regular_nonempty(
    path: &Path,
    contract: &Contract,
    code: DiagnosticCode,
    stage: DiagnosticStage,
    label: &str,
) -> AhiResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        failure(
            contract,
            code,
            stage,
            format!("cannot inspect {label} {}: {error}", path.display()),
            None,
        )
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(failure(
            contract,
            code,
            stage,
            format!("{label} must be a regular nonempty non-symlink file"),
            None,
        ));
    }
    Ok(())
}

fn reject_symlink(path: &Path, contract: &Contract, stage: DiagnosticStage) -> AhiResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(failure(
            contract,
            DiagnosticCode::AhiStaging,
            stage,
            format!("{} escaped through a symlink", path.display()),
            None,
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(
            contract,
            DiagnosticCode::AhiStaging,
            stage,
            format!("cannot inspect {}: {error}", path.display()),
            None,
        )),
    }
}

fn require_success(
    output: &Output,
    contract: &Contract,
    tool: &Path,
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: &str,
) -> AhiResult<()> {
    if output.status.success() {
        return Ok(());
    }
    let detail = process_detail(output);
    let rendered = if detail.is_empty() {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };
    Err(failure(
        contract,
        code,
        stage,
        rendered,
        Some((tool, output)),
    ))
}

fn process_detail(output: &Output) -> String {
    const LIMIT: usize = 64 * 1024;
    fn part(bytes: &[u8]) -> String {
        let selected = if bytes.len() > LIMIT {
            &bytes[..LIMIT]
        } else {
            bytes
        };
        let mut text = String::from_utf8_lossy(selected).trim().to_owned();
        if bytes.len() > LIMIT {
            text.push_str("\n[output truncated by aros-ahi-runner]");
        }
        text
    }
    let stdout = part(&output.stdout);
    let stderr = part(&output.stderr);
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn digest(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn context(contract: &Contract) -> DiagnosticContext {
    DiagnosticContext {
        mode: Some(contract.mode.as_str().into()),
        target: Some(contract.target_triple.clone()),
        ..DiagnosticContext::default()
    }
}

fn stage_failure(contract: &Contract, message: impl Into<String>) -> AhiFailure {
    failure(
        contract,
        DiagnosticCode::AhiStaging,
        DiagnosticStage::AhiStaging,
        message,
        None,
    )
}

fn configure_failure(
    contract: &Contract,
    message: impl Into<String>,
    process: Option<(&Path, &Output)>,
) -> AhiFailure {
    failure(
        contract,
        DiagnosticCode::AhiConfigure,
        DiagnosticStage::AhiConfigure,
        message,
        process,
    )
}

fn build_failure(
    contract: &Contract,
    message: impl Into<String>,
    process: Option<(&Path, &Output)>,
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

fn source_failure(contract: &Contract, message: impl Into<String>) -> AhiFailure {
    failure(
        contract,
        DiagnosticCode::AhiSourceAudit,
        DiagnosticStage::AhiSourceAudit,
        message,
        None,
    )
}

fn failure(
    contract: &Contract,
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
    process: Option<(&Path, &Output)>,
) -> AhiFailure {
    let mut diagnostic_context = context(contract);
    if let Some((tool, output)) = process {
        diagnostic_context.tool = Some(tool.display().to_string());
        diagnostic_context.exit_code = output.status.code();
        diagnostic_context.signal = output.status.signal();
    }
    AhiFailure::new(Diagnostic::error(code, stage, message).with_context(diagnostic_context))
}
