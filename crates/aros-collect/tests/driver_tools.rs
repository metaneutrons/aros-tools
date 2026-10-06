#![cfg(unix)]

use std::fmt::Write as FmtWrite;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

// A child created by another test inherits this process's writable executable
// descriptors until exec closes them. Even after the copying thread closes its
// descriptor, that inherited writer can make Linux reject exec with ETXTBSY.
// Protect executable publication and spawning with the same lock; unique paths
// and O_CLOEXEC alone do not prevent this cross-test descriptor inheritance.
static EXECUTABLE_PUBLICATION: Mutex<()> = Mutex::new(());

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn elf64_fixture() -> Vec<u8> {
    let mut names = b"\0.shstrtab\0".to_vec();
    let section_name_offset = u32::try_from(names.len()).unwrap();
    names.extend_from_slice(b".text");
    names.push(0);

    let names_offset = 0x40;
    let section_table_offset = 0x80;
    let mut bytes = vec![0_u8; section_table_offset + 3 * 0x40];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    put_u32(&mut bytes, 0x14, 1);
    put_u16(&mut bytes, 0x34, 64);
    put_u64(&mut bytes, 0x28, section_table_offset as u64);
    put_u16(&mut bytes, 0x3a, 0x40);
    put_u16(&mut bytes, 0x3c, 3);
    put_u16(&mut bytes, 0x3e, 1);
    bytes[names_offset..names_offset + names.len()].copy_from_slice(&names);

    let names_header = section_table_offset + 0x40;
    put_u32(&mut bytes, names_header, 1);
    put_u32(&mut bytes, names_header + 4, 3);
    put_u64(&mut bytes, names_header + 0x18, names_offset as u64);
    put_u64(&mut bytes, names_header + 0x20, names.len() as u64);
    put_u64(&mut bytes, names_header + 0x30, 1);

    let section_header = section_table_offset + 2 * 0x40;
    put_u32(&mut bytes, section_header, section_name_offset);
    put_u32(&mut bytes, section_header + 4, 1);
    put_u64(&mut bytes, section_header + 0x30, 1);
    bytes
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

fn make_executable(path: &Path, body: &str) {
    let _publication = EXECUTABLE_PUBLICATION.lock().unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn copy_driver(directory: &Path, filename: &str) -> PathBuf {
    let _publication = EXECUTABLE_PUBLICATION.lock().unwrap();
    let destination = directory.join(filename);
    fs::copy(env!("CARGO_BIN_EXE_aros-collect"), &destination).unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
    destination
}

fn logger_script(log: &Path, output_fixture: Option<&Path>) -> String {
    let mut script = format!(
        "#!/bin/sh\nprintf 'TOOL=%s\\n' \"$0\" >> {}\nfor arg do printf 'ARG=%s\\n' \"$arg\" >> {}; done\nprintf 'END\\n' >> {}\n",
        quote(log),
        quote(log),
        quote(log)
    );
    if let Some(fixture) = output_fixture {
        script.push_str(
            "out=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then shift; out=$1; break; fi\n  shift\ndone\n[ -n \"$out\" ] || exit 91\ncase \"$out\" in -*) out=./$out;; esac\n",
        );
        writeln!(script, "/bin/cp {} \"$out\"", quote(fixture)).unwrap();
    }
    script
}

fn manifest(invocation: &str, linker: &str, strip: &str, emulation: Option<&str>) -> String {
    let emulation_field = emulation
        .map(|value| format!(",\"emulation\":\"{value}\""))
        .unwrap_or_default();
    format!(
        "{{\"schema\":\"aros-collector-tools-v1\",\"family\":\"gnu\",\"invocation\":\"{invocation}\",\"linker\":\"{linker}\",\"strip\":\"{strip}\"{emulation_field}}}"
    )
}

fn gnu_manifest_with_driver_emulation(
    invocation: &str,
    linker: &str,
    strip: &str,
    emulation: &str,
    driver_emulation: &str,
) -> String {
    format!(
        "{{\"schema\":\"aros-collector-tools-v1\",\"family\":\"gnu\",\"invocation\":\"{invocation}\",\"linker\":\"{linker}\",\"strip\":\"{strip}\",\"emulation\":\"{emulation}\",\"driver_emulation\":\"{driver_emulation}\"}}"
    )
}

fn run(driver: &Path, path: &Path, output: &Path) -> Output {
    run_args(
        driver,
        path,
        &["-o", output.to_str().unwrap(), "-s", "input.o"],
        None,
    )
}

fn run_args(driver: &Path, path: &Path, args: &[&str], current_dir: Option<&Path>) -> Output {
    let _publication = EXECUTABLE_PUBLICATION.lock().unwrap();
    let mut command = Command::new(driver);
    command.args(args).env_clear().env("PATH", path);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    command.output().unwrap()
}

#[test]
fn configured_gcc_alias_uses_gnu_tools_and_normalizes_driver_emulation() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("selected.log");
    let fallback_log = directory.path().join("fallback.log");
    let linker_name = "ld";
    let strip_name = "strip";
    make_executable(
        &directory.path().join(linker_name),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(
        &directory.path().join(strip_name),
        &logger_script(&log, None),
    );
    make_executable(
        &directory.path().join("ld.lld"),
        &logger_script(&fallback_log, None),
    );
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        gnu_manifest_with_driver_emulation(
            "collect-aros",
            linker_name,
            strip_name,
            "riscvelf_aros",
            "elf32lriscv",
        ),
    )
    .unwrap();

    let output = directory.path().join("result.o");
    let result = run_args(
        &driver,
        &path_bin,
        &[
            "-melf32lriscv",
            "-o",
            output.to_str().unwrap(),
            "-s",
            "input.o",
        ],
        None,
    );

    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let log_contents = fs::read_to_string(&log).unwrap();
    let expected_linker = format!(
        "TOOL={}",
        fs::canonicalize(directory.path().join(linker_name))
            .unwrap()
            .display()
    );
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == expected_linker.as_str())
            .count(),
        2
    );
    assert!(log_contents.contains(&format!(
        "TOOL={}\n",
        fs::canonicalize(directory.path().join(strip_name))
            .unwrap()
            .display()
    )));
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == "ARG=-m")
            .count(),
        2
    );
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == "ARG=riscvelf_aros")
            .count(),
        2
    );
    assert!(!log_contents.contains("ARG=elf32lriscv\n"));
    assert!(!fallback_log.exists());
    assert!(output.is_file());
}

#[test]
fn mismatching_driver_emulation_does_not_invoke_tools_or_replace_output() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("tools.log");
    make_executable(
        &directory.path().join("ld"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(&directory.path().join("strip"), &logger_script(&log, None));
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        gnu_manifest_with_driver_emulation(
            "collect-aros",
            "ld",
            "strip",
            "riscvelf_aros",
            "elf32lriscv",
        ),
    )
    .unwrap();
    let output = directory.path().join("existing.o");
    fs::write(&output, b"previous output").unwrap();

    let result = run_args(
        &driver,
        &path_bin,
        &["-melf64lriscv", "-o", output.to_str().unwrap(), "input.o"],
        None,
    );

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("conflicting emulation"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!log.exists());
}

#[test]
fn emulation_like_output_and_script_operands_are_not_normalized_as_emulations() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("tools.log");
    make_executable(
        &directory.path().join("ld"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(&directory.path().join("strip"), &logger_script(&log, None));
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        gnu_manifest_with_driver_emulation(
            "collect-aros",
            "ld",
            "strip",
            "riscvelf_aros",
            "elf32lriscv",
        ),
    )
    .unwrap();

    let result = run_args(
        &driver,
        &path_bin,
        &[
            "-o",
            "-m-output.o",
            "-T",
            "-m-script.ld",
            "-melf32lriscv",
            "input.o",
        ],
        Some(&path_bin),
    );

    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(path_bin.join("-m-output.o").is_file());
    let log_contents = fs::read_to_string(log).unwrap();
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == "ARG=riscvelf_aros")
            .count(),
        2
    );
    assert!(!log_contents.contains("ARG=-m-output.o\n"));
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == "ARG=-m-script.ld")
            .count(),
        1
    );
}

#[test]
fn duplicate_output_is_rejected_before_tools_or_output_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("tools.log");
    make_executable(
        &directory.path().join("ld"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(&directory.path().join("strip"), &logger_script(&log, None));
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        gnu_manifest_with_driver_emulation(
            "collect-aros",
            "ld",
            "strip",
            "riscvelf_aros",
            "elf32lriscv",
        ),
    )
    .unwrap();
    let output = path_bin.join("existing.o");
    fs::write(&output, b"previous output").unwrap();

    let result = run_args(
        &driver,
        &path_bin,
        &["-o", "existing.o", "--output=other.o", "input.o"],
        Some(&path_bin),
    );

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("specifies output more than once"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!log.exists());
    assert!(!path_bin.join("other.o").exists());
}

#[test]
fn prefixed_driver_requires_a_manifest_and_never_searches_path() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "riscv-aros-collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fallback_log = directory.path().join("fallback.log");
    make_executable(
        &path_bin.join("ld.lld"),
        &logger_script(&fallback_log, None),
    );
    make_executable(
        &path_bin.join("llvm-strip"),
        &logger_script(&fallback_log, None),
    );
    let output = directory.path().join("existing.o");
    fs::write(&output, b"previous output").unwrap();

    let result = run(&driver, &path_bin, &output);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires an adjacent"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!fallback_log.exists());
}

#[test]
fn prefixed_symlink_to_legacy_named_binary_still_requires_a_manifest() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let canonical_driver = copy_driver(directory.path(), "collect-aros");
    let prefixed_alias = directory.path().join("riscv-aros-collect-aros");
    symlink(&canonical_driver, &prefixed_alias).unwrap();
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("tools.log");
    make_executable(
        &directory.path().join("ld.lld"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(
        &directory.path().join("llvm-strip"),
        &logger_script(&log, None),
    );
    let output = directory.path().join("existing.o");
    fs::write(&output, b"previous output").unwrap();

    let result = run(&prefixed_alias, &path_bin, &output);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires an adjacent"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!log.exists());
}

#[test]
fn configured_sibling_tools_are_not_resolved_from_path() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "riscv-aros-collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fallback_log = directory.path().join("fallback.log");
    make_executable(
        &path_bin.join("ld.gnu"),
        &logger_script(&fallback_log, None),
    );
    make_executable(
        &path_bin.join("strip.gnu"),
        &logger_script(&fallback_log, None),
    );
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        manifest(
            "riscv-aros-collect-aros",
            "ld.gnu",
            "strip.gnu",
            Some("riscv64elf_aros"),
        ),
    )
    .unwrap();
    let output = directory.path().join("existing.o");
    fs::write(&output, b"previous output").unwrap();

    let result = run(&driver, &path_bin, &output);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("sibling tool"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!fallback_log.exists());
}

#[test]
fn bad_manifest_fails_before_mutating_output_or_running_tools() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "riscv-aros-collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let log = directory.path().join("tools.log");
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    make_executable(
        &directory.path().join("ld.gnu"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(
        &directory.path().join("strip.gnu"),
        &logger_script(&log, None),
    );
    let output = directory.path().join("existing.o");
    fs::write(&output, b"previous output").unwrap();
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        manifest(
            "riscv-aros-collect-aros",
            "ld.gnu",
            "strip.gnu",
            Some("riscv64elf_aros"),
        )
        .replace('}', ",\"unknown\":true}"),
    )
    .unwrap();

    let result = run(&driver, &path_bin, &output);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid JSON"));
    assert_eq!(fs::read(&output).unwrap(), b"previous output");
    assert!(!log.exists());
}

#[test]
fn legacy_alias_without_manifest_keeps_llvm_sibling_tools() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("legacy.log");
    let linker = directory.path().join("ld.lld");
    let strip = directory.path().join("llvm-strip");
    make_executable(&linker, &logger_script(&log, Some(&fixture)));
    make_executable(&strip, &logger_script(&log, None));
    let output = directory.path().join("legacy.o");

    let result = run(&driver, &path_bin, &output);

    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let log_contents = fs::read_to_string(&log).unwrap();
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| {
                *line == format!("TOOL={}", fs::canonicalize(&linker).unwrap().display()).as_str()
            })
            .count(),
        2
    );
    assert!(log_contents.contains(&format!(
        "TOOL={}\n",
        fs::canonicalize(&strip).unwrap().display()
    )));
    assert!(output.is_file());
}

#[test]
fn legacy_symlink_alias_to_canonical_executable_keeps_llvm_fallback() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let canonical_driver = copy_driver(directory.path(), "aros-collect");
    let alias = directory.path().join("collect-aros");
    symlink(&canonical_driver, &alias).unwrap();
    let path_bin = directory.path().join("path-bin");
    fs::create_dir(&path_bin).unwrap();
    let fixture = directory.path().join("fixture.o");
    fs::write(&fixture, elf64_fixture()).unwrap();
    let log = directory.path().join("legacy-symlink.log");
    let linker = directory.path().join("ld.lld");
    let strip = directory.path().join("llvm-strip");
    make_executable(&linker, &logger_script(&log, Some(&fixture)));
    make_executable(&strip, &logger_script(&log, None));
    let output = directory.path().join("legacy-symlink.o");

    let result = run(&alias, &path_bin, &output);

    assert!(
        result.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let log_contents = fs::read_to_string(&log).unwrap();
    let expected_linker = format!("TOOL={}", fs::canonicalize(&linker).unwrap().display());
    let expected_strip = format!("TOOL={}", fs::canonicalize(&strip).unwrap().display());
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == expected_linker.as_str())
            .count(),
        2
    );
    assert_eq!(
        log_contents
            .lines()
            .filter(|line| *line == expected_strip.as_str())
            .count(),
        1
    );
    assert!(output.is_file());
}

#[test]
fn gnu_implicit_output_uses_two_staged_passes_and_response_file_arguments() {
    let directory = tempfile::tempdir().unwrap();
    let driver = copy_driver(directory.path(), "collect-aros");
    let fixture = directory.path().join("fixture.o");
    let log = directory.path().join("tools.log");
    fs::write(&fixture, elf64_fixture()).unwrap();
    make_executable(
        &directory.path().join("ld"),
        &logger_script(&log, Some(&fixture)),
    );
    make_executable(&directory.path().join("strip"), &logger_script(&log, None));
    fs::write(
        directory.path().join("aros-collector-tools.json"),
        manifest("collect-aros", "ld", "strip", Some("riscv64elf_aros")),
    )
    .unwrap();
    fs::write(directory.path().join("input.rsp"), "input.o").unwrap();
    fs::write(directory.path().join("a.out"), b"previous good output").unwrap();
    let result = run_args(
        &driver,
        directory.path(),
        &["@input.rsp"],
        Some(directory.path()),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let bytes = fs::read(directory.path().join("a.out")).unwrap();
    assert_eq!(bytes[7], 15);
    let arguments = fs::read_to_string(&log).unwrap();
    assert!(arguments.contains("ARG=a.out.collect-pre\n"));
    assert!(arguments.contains("ARG=a.out.collect-final\n"));
    assert!(!arguments.contains("ARG=a.out\n"));
    assert!(!directory.path().join("a.out.collect-pre").exists());
    assert!(!directory.path().join("a.out.collect-final").exists());
}

#[test]
fn failed_gnu_default_link_preserves_existing_output_and_cleans_staging() {
    for fail_second in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let driver = copy_driver(directory.path(), "collect-aros");
        let fixture = directory.path().join("fixture.o");
        let log = directory.path().join("tools.log");
        fs::write(&fixture, elf64_fixture()).unwrap();
        let mut body = String::from("#!/bin/sh\n");
        if fail_second {
            body.push_str("for arg do case $arg in *.collect-final) exit 27;; esac; done\n");
            body.push_str(&logger_script(&log, Some(&fixture)));
        } else {
            body.push_str("exit 27\n");
        }
        make_executable(&directory.path().join("ld"), &body);
        make_executable(&directory.path().join("strip"), &logger_script(&log, None));
        fs::write(
            directory.path().join("aros-collector-tools.json"),
            manifest("collect-aros", "ld", "strip", Some("riscv64elf_aros")),
        )
        .unwrap();
        fs::write(directory.path().join("a.out"), b"previous good output").unwrap();
        let result = run_args(
            &driver,
            directory.path(),
            &["input.o"],
            Some(directory.path()),
        );
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(if fail_second {
                "AC0302"
            } else {
                "AC0301"
            })
        );
        assert_eq!(
            fs::read(directory.path().join("a.out")).unwrap(),
            b"previous good output"
        );
        assert!(!directory.path().join("a.out.collect-pre").exists());
        assert!(!directory.path().join("a.out.collect-final").exists());
    }
}

#[test]
fn llvm_manifest_and_legacy_alias_never_infer_gnu_default_output() {
    for configured in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let driver = copy_driver(directory.path(), "collect-aros");
        let log = directory.path().join("tools.log");
        make_executable(&directory.path().join("ld.lld"), &logger_script(&log, None));
        make_executable(
            &directory.path().join("llvm-strip"),
            &logger_script(&log, None),
        );
        if configured {
            fs::write(directory.path().join("aros-collector-tools.json"),
                r#"{"schema":"aros-collector-tools-v1","family":"llvm","invocation":"collect-aros","linker":"ld.lld","strip":"llvm-strip"}"#).unwrap();
        }
        fs::write(directory.path().join("a.out"), b"previous good output").unwrap();
        let result = run_args(
            &driver,
            directory.path(),
            &["input.o"],
            Some(directory.path()),
        );
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("AC0001"));
        assert!(!log.exists());
        assert_eq!(
            fs::read(directory.path().join("a.out")).unwrap(),
            b"previous good output"
        );
    }
}
