#![cfg(unix)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aros_common::{
    elf::{self, riscv::TargetContract, AROS_ABI_VERSION, OS_ABI_AROS},
    sha256_bytes, ArosCompilerIdentity, DiagnosticCode,
};
use aros_toolchain::compatibility::{
    verify_standalone_outputs, verify_standalone_outputs_with_compilers, StandaloneOutputRequest,
    StandaloneTargetArtifacts,
};

const RV32_TRIPLE: &str = "riscv-unknown-aros";
const RV64_TRIPLE: &str = "riscv64-unknown-aros";
const X86_64_TRIPLE: &str = "x86_64-unknown-aros";
const AARCH64_TRIPLE: &str = "aarch64-unknown-aros";
const RV32_ARCH: &str = "rv32i2p1";
const RV64_ARCH: &str = "rv64i2p1_f2p2_d2p2";
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const SHN_ABS: u16 = 0xfff1;
const SHN_COMMON: u16 = 0xfff2;
const SHN_XINDEX: u16 = 0xffff;
const C_COLLECTOR_SYMBOL: &str = "__TOOLCHAIN_LIST__";
const CXX_COLLECTOR_SYMBOL: &str = "__INIT_ARRAY_LIST__";

fn compiler_for_abi(abi: &str) -> ArosCompilerIdentity {
    let (isa, architecture) = match abi {
        "ilp32" => ("rv32i", RV32_ARCH),
        "lp64d" => ("rv64ifd", RV64_ARCH),
        other => panic!("unexpected fixture ABI: {other}"),
    };
    let contract = TargetContract::parse(
        serde_json::to_vec(&serde_json::json!({
            "schema": "aros-riscv-target-v1",
            "isa": isa,
            "abi": abi,
            "code_model": "medany",
            "architecture": architecture,
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        }))
        .unwrap()
        .as_slice(),
    )
    .unwrap();
    ArosCompilerIdentity::Gnu {
        gcc_version: "16.2.0".into(),
        binutils_version: "2.47".into(),
        target: contract,
    }
}

fn rv32_compiler() -> ArosCompilerIdentity {
    compiler_for_abi("ilp32")
}

fn rv64_compiler() -> ArosCompilerIdentity {
    compiler_for_abi("lp64d")
}

fn llvm_compiler() -> ArosCompilerIdentity {
    ArosCompilerIdentity::Llvm {
        version: "18.1.8".into(),
    }
}

fn make_elf(
    class: elf::Class,
    symbol: &str,
    architecture: &str,
    os_abi: u8,
    flags: u32,
) -> Vec<u8> {
    make_elf_with_identity(
        class,
        symbol,
        architecture,
        os_abi,
        flags,
        elf::riscv::MACHINE,
        1,
        2,
    )
}

#[allow(clippy::too_many_arguments)] // Fixture inputs map to independent ELF header and symbol fields.
fn make_elf_with_identity(
    class: elf::Class,
    symbol: &str,
    architecture: &str,
    os_abi: u8,
    flags: u32,
    machine: u16,
    kind: u16,
    symbol_section: u16,
) -> Vec<u8> {
    let (header_size, section_size, symbol_size) = match class {
        elf::Class::Elf32 => (52_usize, 40_usize, 16_usize),
        elf::Class::Elf64 => (64_usize, 64_usize, 24_usize),
    };
    let section_count = 5_usize;
    let section_table_offset = header_size;
    let section_names = b"\0.shstrtab\0.riscv.attributes\0.strtab\0.symtab\0".to_vec();
    let shstrtab_name = section_name_offset(&section_names, ".shstrtab");
    let attributes_name = section_name_offset(&section_names, ".riscv.attributes");
    let strtab_name = section_name_offset(&section_names, ".strtab");
    let symtab_name = section_name_offset(&section_names, ".symtab");
    let attributes = encode_riscv_attributes(architecture);
    let mut symbol_names = vec![0_u8];
    symbol_names.extend_from_slice(symbol.as_bytes());
    symbol_names.push(0);
    let symbol_name_offset = 1_u32;
    let mut symbol_table = vec![0_u8; symbol_size * 2];

    let data_offset = section_table_offset + section_count * section_size;
    let shstrtab_offset = data_offset;
    let attributes_offset = shstrtab_offset + section_names.len();
    let strtab_offset = attributes_offset + attributes.len();
    let symtab_offset = strtab_offset + symbol_names.len();

    let mut bytes = vec![0_u8; symtab_offset + symbol_table.len()];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = match class {
        elf::Class::Elf32 => 1,
        elf::Class::Elf64 => 2,
    };
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[7] = os_abi;
    bytes[8] = AROS_ABI_VERSION;
    write_u16(&mut bytes, 0x10, kind);
    write_u16(&mut bytes, 0x12, machine);
    write_u32(&mut bytes, 0x14, 1);
    match class {
        elf::Class::Elf32 => {
            write_u32(&mut bytes, 0x20, section_table_offset as u32);
            write_u32(&mut bytes, 0x24, flags);
            write_u16(&mut bytes, 0x28, header_size as u16);
            write_u16(&mut bytes, 0x2e, section_size as u16);
            write_u16(&mut bytes, 0x30, section_count as u16);
            write_u16(&mut bytes, 0x32, 1);
        }
        elf::Class::Elf64 => {
            write_u64(&mut bytes, 0x28, section_table_offset as u64);
            write_u32(&mut bytes, 0x30, flags);
            write_u16(&mut bytes, 0x34, header_size as u16);
            write_u16(&mut bytes, 0x3a, section_size as u16);
            write_u16(&mut bytes, 0x3c, section_count as u16);
            write_u16(&mut bytes, 0x3e, 1);
        }
    }

    write_section(
        &mut bytes,
        class,
        section_table_offset + section_size,
        shstrtab_name,
        elf::SHT_STRTAB,
        shstrtab_offset,
        section_names.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 2 * section_size,
        attributes_name,
        elf::riscv::SHT_ATTRIBUTES,
        attributes_offset,
        attributes.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 3 * section_size,
        strtab_name,
        elf::SHT_STRTAB,
        strtab_offset,
        symbol_names.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 4 * section_size,
        symtab_name,
        elf::SHT_SYMTAB,
        symtab_offset,
        symbol_table.len(),
        3,
        1,
        if class == elf::Class::Elf32 { 4 } else { 8 },
        symbol_size,
    );

    let symbol = symbol_size;
    write_u32(&mut symbol_table, symbol, symbol_name_offset);
    match class {
        elf::Class::Elf32 => {
            symbol_table[symbol + 12] = 0x10;
            write_u16(&mut symbol_table, symbol + 14, symbol_section);
        }
        elf::Class::Elf64 => {
            symbol_table[symbol + 4] = 0x10;
            write_u16(&mut symbol_table, symbol + 6, symbol_section);
        }
    }

    bytes[shstrtab_offset..attributes_offset].copy_from_slice(&section_names);
    bytes[attributes_offset..strtab_offset].copy_from_slice(&attributes);
    bytes[strtab_offset..symtab_offset].copy_from_slice(&symbol_names);
    bytes[symtab_offset..].copy_from_slice(&symbol_table);
    bytes
}

fn section_name_offset(names: &[u8], name: &str) -> u32 {
    let mut needle = name.as_bytes().to_vec();
    needle.push(0);
    u32::try_from(
        names
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("fixture section name exists"),
    )
    .unwrap()
}

fn encode_riscv_attributes(architecture: &str) -> Vec<u8> {
    let mut tags = vec![4, 16, 5];
    tags.extend_from_slice(architecture.as_bytes());
    tags.push(0);
    let file_size = u32::try_from(5 + tags.len()).unwrap();
    let vendor_size = 10 + file_size;
    let mut bytes = vec![b'A'];
    bytes.extend_from_slice(&vendor_size.to_le_bytes());
    bytes.extend_from_slice(b"riscv\0");
    bytes.push(1);
    bytes.extend_from_slice(&file_size.to_le_bytes());
    bytes.extend_from_slice(&tags);
    bytes
}

#[allow(clippy::too_many_arguments)]
fn write_section(
    bytes: &mut [u8],
    class: elf::Class,
    at: usize,
    name: u32,
    kind: u32,
    offset: usize,
    size: usize,
    link: u32,
    info: u32,
    align: usize,
    entry_size: usize,
) {
    write_u32(bytes, at, name);
    write_u32(bytes, at + 4, kind);
    match class {
        elf::Class::Elf32 => {
            write_u32(bytes, at + 16, offset as u32);
            write_u32(bytes, at + 20, size as u32);
            write_u32(bytes, at + 24, link);
            write_u32(bytes, at + 28, info);
            write_u32(bytes, at + 32, align as u32);
            write_u32(bytes, at + 36, entry_size as u32);
        }
        elf::Class::Elf64 => {
            write_u64(bytes, at + 24, offset as u64);
            write_u64(bytes, at + 32, size as u64);
            write_u32(bytes, at + 40, link);
            write_u32(bytes, at + 44, info);
            write_u64(bytes, at + 48, align as u64);
            write_u64(bytes, at + 56, entry_size as u64);
        }
    }
}

fn write_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn create_output_root(parent: &Path) -> PathBuf {
    let output_root = parent.join("outputs");
    fs::create_dir(&output_root).unwrap();
    output_root
}

fn write_pair(
    output_root: &Path,
    class: elf::Class,
    architecture: &str,
    flags: u32,
) -> StandaloneTargetArtifacts {
    let prefix = match class {
        elf::Class::Elf32 => "rv32",
        elf::Class::Elf64 => "rv64",
    };
    let c = output_root.join(format!("{prefix}-standalone-c.o"));
    let cxx = output_root.join(format!("{prefix}-standalone-cxx.o"));
    fs::write(
        &c,
        make_elf(class, C_COLLECTOR_SYMBOL, architecture, OS_ABI_AROS, flags),
    )
    .unwrap();
    fs::write(
        &cxx,
        make_elf(
            class,
            CXX_COLLECTOR_SYMBOL,
            architecture,
            OS_ABI_AROS,
            flags,
        ),
    )
    .unwrap();
    StandaloneTargetArtifacts { c, cxx }
}

fn write_pair_with_identity(
    output_root: &Path,
    class: elf::Class,
    architecture: &str,
    flags: u32,
    machine: u16,
    kind: u16,
    c_symbol_section: u16,
) -> StandaloneTargetArtifacts {
    let c = output_root.join("standalone-c.o");
    let cxx = output_root.join("standalone-cxx.o");
    fs::write(
        &c,
        make_elf_with_identity(
            class,
            C_COLLECTOR_SYMBOL,
            architecture,
            OS_ABI_AROS,
            flags,
            machine,
            kind,
            c_symbol_section,
        ),
    )
    .unwrap();
    fs::write(
        &cxx,
        make_elf_with_identity(
            class,
            CXX_COLLECTOR_SYMBOL,
            architecture,
            OS_ABI_AROS,
            flags,
            machine,
            kind,
            2,
        ),
    )
    .unwrap();
    StandaloneTargetArtifacts { c, cxx }
}

fn one_target_request(
    output_root: &Path,
    triple: &str,
    artifacts: StandaloneTargetArtifacts,
) -> StandaloneOutputRequest {
    StandaloneOutputRequest {
        output_root: output_root.to_owned(),
        targets: BTreeMap::from([(triple.to_owned(), artifacts)]),
    }
}

fn assert_ax0703(error: &aros_toolchain::ContractError, untrusted_path: Option<&str>) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert!(!diagnostics.is_empty());
    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic.code == DiagnosticCode::ProducerCompatibility));
    if let Some(marker) = untrusted_path {
        let rendered = format!("{error}\n{:?}", error.diagnostics());
        assert!(
            !rendered.contains(marker),
            "diagnostic disclosed an untrusted path marker: {rendered}"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntrySnapshot {
    mode: u32,
    len: u64,
    device: u64,
    inode: u64,
    modified: SystemTime,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    contents: Option<Vec<u8>>,
    link_target: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectorySnapshot {
    root_device: u64,
    root_inode: u64,
    root_modified: SystemTime,
    root_changed_seconds: i64,
    root_changed_nanoseconds: i64,
    entries: BTreeMap<OsString, EntrySnapshot>,
}

fn snapshot_directory(root: &Path) -> DirectorySnapshot {
    let root_metadata = fs::symlink_metadata(root).unwrap();
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let file_type = metadata.file_type();
        let contents = if file_type.is_file() {
            Some(fs::read(&path).unwrap())
        } else {
            None
        };
        let link_target = if file_type.is_symlink() {
            Some(fs::read_link(&path).unwrap())
        } else {
            None
        };
        entries.insert(
            entry.file_name(),
            EntrySnapshot {
                mode: metadata.mode(),
                len: metadata.len(),
                device: metadata.dev(),
                inode: metadata.ino(),
                modified: metadata.modified().unwrap(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
                contents,
                link_target,
            },
        );
    }
    DirectorySnapshot {
        root_device: root_metadata.dev(),
        root_inode: root_metadata.ino(),
        root_modified: root_metadata.modified().unwrap(),
        root_changed_seconds: root_metadata.ctime(),
        root_changed_nanoseconds: root_metadata.ctime_nsec(),
        entries,
    }
}

fn assert_identity(
    identity: &aros_toolchain::compatibility::StandaloneArtifactIdentity,
    path: &Path,
    expected_class: elf::Class,
) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(identity.sha256, sha256_bytes(&bytes));
    assert_eq!(identity.size, bytes.len() as u64);
    assert_eq!(identity.class, expected_class);
}

#[test]
fn measures_gnu_rv32_and_rv64_outputs_and_legacy_verifier_rejects_them() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = create_output_root(temporary.path());
    let rv32_artifacts = write_pair(&output_root, elf::Class::Elf32, RV32_ARCH, 0);
    let rv64_artifacts = write_pair(&output_root, elf::Class::Elf64, RV64_ARCH, 4);
    let request = StandaloneOutputRequest {
        output_root: output_root.clone(),
        targets: BTreeMap::from([
            (RV32_TRIPLE.into(), rv32_artifacts.clone()),
            (RV64_TRIPLE.into(), rv64_artifacts.clone()),
        ]),
    };
    let compilers = BTreeMap::from([
        (RV32_TRIPLE.into(), rv32_compiler()),
        (RV64_TRIPLE.into(), rv64_compiler()),
    ]);
    for (triple, artifacts) in &request.targets {
        let ArosCompilerIdentity::Gnu { target, .. } = &compilers[triple] else {
            unreachable!("fixture compilers are GNU");
        };
        for path in [&artifacts.c, &artifacts.cxx] {
            let bytes = fs::read(path).unwrap();
            let object = elf::read(&bytes).unwrap();
            assert_eq!(object.os_abi, OS_ABI_AROS);
            assert_eq!(object.abi_version, AROS_ABI_VERSION);
            let expected_class = if target.abi().starts_with("ilp32") {
                elf::Class::Elf32
            } else {
                elf::Class::Elf64
            };
            assert_eq!(object.class, expected_class, "{triple}: {}", target.abi());
            target
                .verify(&bytes, elf::riscv::ArtifactRole::ArosRelocatable)
                .unwrap_or_else(|error| panic!("{triple}: {}: {error}", target.abi()));
        }
    }

    let before = snapshot_directory(&output_root);
    let report = verify_standalone_outputs_with_compilers(&request, &compilers).unwrap();
    assert_eq!(report.targets.len(), 2);
    let rv32 = &report.targets[RV32_TRIPLE];
    assert_identity(&rv32.c, &rv32_artifacts.c, elf::Class::Elf32);
    assert_identity(&rv32.cxx, &rv32_artifacts.cxx, elf::Class::Elf32);
    let rv64 = &report.targets[RV64_TRIPLE];
    assert_identity(&rv64.c, &rv64_artifacts.c, elf::Class::Elf64);
    assert_identity(&rv64.cxx, &rv64_artifacts.cxx, elf::Class::Elf64);
    assert_eq!(snapshot_directory(&output_root), before);

    let legacy_error = verify_standalone_outputs(&one_target_request(
        &output_root,
        RV32_TRIPLE,
        rv32_artifacts,
    ))
    .unwrap_err();
    assert_ax0703(&legacy_error, None);
    assert_eq!(snapshot_directory(&output_root), before);
}

#[test]
fn rejects_empty_or_mismatched_compiler_maps_before_touching_outputs() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = create_output_root(temporary.path());
    let artifacts = write_pair(&output_root, elf::Class::Elf32, RV32_ARCH, 0);
    let request = one_target_request(&output_root, RV32_TRIPLE, artifacts);
    let before = snapshot_directory(&output_root);

    let cases = [
        ("missing", BTreeMap::new()),
        (
            "extra",
            BTreeMap::from([
                (RV32_TRIPLE.into(), rv32_compiler()),
                (RV64_TRIPLE.into(), rv64_compiler()),
            ]),
        ),
        (
            "llvm-for-riscv",
            BTreeMap::from([(
                RV32_TRIPLE.into(),
                ArosCompilerIdentity::Llvm {
                    version: "18.1.8".into(),
                },
            )]),
        ),
        (
            "wrong-width",
            BTreeMap::from([(RV32_TRIPLE.into(), rv64_compiler())]),
        ),
    ];
    for (label, compilers) in cases {
        let error = verify_standalone_outputs_with_compilers(&request, &compilers)
            .err()
            .unwrap_or_else(|| panic!("{label} compiler map unexpectedly passed"));
        assert_ax0703(&error, None);
        assert_eq!(snapshot_directory(&output_root), before, "{label}");
    }

    let empty_request = StandaloneOutputRequest {
        output_root: output_root.clone(),
        targets: BTreeMap::new(),
    };
    let empty_error =
        verify_standalone_outputs_with_compilers(&empty_request, &BTreeMap::new()).unwrap_err();
    assert_ax0703(&empty_error, None);
    assert_eq!(snapshot_directory(&output_root), before);
}

#[derive(Clone, Copy)]
enum InvalidOutput {
    Machine,
    Type,
    OsAbi,
    FloatingPointAbi,
    IsaAttributes,
    MissingCollectorSymbol,
    UnknownFlags,
}

#[test]
fn rejects_riscv_outputs_that_disagree_with_the_target_contract() {
    let cases = [
        ("wrong-machine", InvalidOutput::Machine),
        ("wrong-type", InvalidOutput::Type),
        ("wrong-osabi", InvalidOutput::OsAbi),
        ("wrong-fp-abi", InvalidOutput::FloatingPointAbi),
        ("wrong-isa-attributes", InvalidOutput::IsaAttributes),
        ("missing-symbol", InvalidOutput::MissingCollectorSymbol),
        ("unknown-flags", InvalidOutput::UnknownFlags),
    ];

    for (label, invalid) in cases {
        let temporary = tempfile::tempdir().unwrap();
        let output_root = create_output_root(temporary.path());
        let actual_architecture = if matches!(invalid, InvalidOutput::IsaAttributes) {
            "rv64i2p1_f2p2"
        } else {
            RV64_ARCH
        };
        let symbol = if matches!(invalid, InvalidOutput::MissingCollectorSymbol) {
            "__WRONG_COLLECTOR_SYMBOL__"
        } else {
            C_COLLECTOR_SYMBOL
        };
        let mut c_bytes = make_elf(
            elf::Class::Elf64,
            symbol,
            actual_architecture,
            OS_ABI_AROS,
            4,
        );
        match invalid {
            InvalidOutput::Machine => write_u16(&mut c_bytes, 0x12, 62),
            InvalidOutput::Type => write_u16(&mut c_bytes, 0x10, 2),
            InvalidOutput::OsAbi => c_bytes[7] = 0,
            InvalidOutput::FloatingPointAbi => write_u32(&mut c_bytes, 0x30, 0),
            InvalidOutput::IsaAttributes | InvalidOutput::MissingCollectorSymbol => {}
            InvalidOutput::UnknownFlags => write_u32(&mut c_bytes, 0x30, 0x80),
        }
        let c = output_root.join("standalone-c.o");
        let cxx = output_root.join("standalone-cxx.o");
        fs::write(&c, c_bytes).unwrap();
        fs::write(
            &cxx,
            make_elf(
                elf::Class::Elf64,
                CXX_COLLECTOR_SYMBOL,
                RV64_ARCH,
                OS_ABI_AROS,
                4,
            ),
        )
        .unwrap();
        let request = one_target_request(
            &output_root,
            RV64_TRIPLE,
            StandaloneTargetArtifacts { c, cxx },
        );
        let compilers = BTreeMap::from([(RV64_TRIPLE.into(), rv64_compiler())]);
        let before = snapshot_directory(&output_root);

        let error = verify_standalone_outputs_with_compilers(&request, &compilers).unwrap_err();
        assert_ax0703(&error, None);
        assert_eq!(snapshot_directory(&output_root), before, "{label}");
    }
}

#[test]
fn explicit_llvm_verification_checks_machine_and_relocatable_type() {
    let compilers = BTreeMap::from([(AARCH64_TRIPLE.into(), llvm_compiler())]);

    let valid_temporary = tempfile::tempdir().unwrap();
    let valid_root = create_output_root(valid_temporary.path());
    let valid_artifacts = write_pair_with_identity(
        &valid_root,
        elf::Class::Elf64,
        RV64_ARCH,
        0,
        EM_AARCH64,
        1,
        2,
    );
    let valid_request = one_target_request(&valid_root, AARCH64_TRIPLE, valid_artifacts.clone());
    let valid_object = elf::read(&fs::read(&valid_artifacts.c).unwrap()).unwrap();
    assert_eq!(valid_object.class, elf::Class::Elf64);
    assert_eq!(valid_object.machine, EM_AARCH64);
    assert_eq!(valid_object.kind, 1);
    assert!(valid_object.symbols.iter().any(|symbol| {
        symbol.name == C_COLLECTOR_SYMBOL && symbol.home != elf::Home::Undefined
    }));
    let before_valid = snapshot_directory(&valid_root);
    let report = verify_standalone_outputs_with_compilers(&valid_request, &compilers).unwrap();
    let valid_report = &report.targets[AARCH64_TRIPLE];
    assert_identity(&valid_report.c, &valid_artifacts.c, elf::Class::Elf64);
    assert_identity(&valid_report.cxx, &valid_artifacts.cxx, elf::Class::Elf64);
    assert_eq!(snapshot_directory(&valid_root), before_valid);

    let wrong_machine_temporary = tempfile::tempdir().unwrap();
    let wrong_machine_root = create_output_root(wrong_machine_temporary.path());
    let wrong_machine_artifacts = write_pair_with_identity(
        &wrong_machine_root,
        elf::Class::Elf64,
        RV64_ARCH,
        0,
        EM_X86_64,
        1,
        2,
    );
    let wrong_machine_request =
        one_target_request(&wrong_machine_root, AARCH64_TRIPLE, wrong_machine_artifacts);
    let before_wrong_machine = snapshot_directory(&wrong_machine_root);
    let wrong_machine_error =
        verify_standalone_outputs_with_compilers(&wrong_machine_request, &compilers).unwrap_err();
    assert_ax0703(&wrong_machine_error, None);
    assert_eq!(
        snapshot_directory(&wrong_machine_root),
        before_wrong_machine
    );

    let linked_temporary = tempfile::tempdir().unwrap();
    let linked_root = create_output_root(linked_temporary.path());
    let linked_artifacts = write_pair_with_identity(
        &linked_root,
        elf::Class::Elf64,
        RV64_ARCH,
        0,
        EM_AARCH64,
        2,
        2,
    );
    let linked_request = one_target_request(&linked_root, AARCH64_TRIPLE, linked_artifacts);
    let before_linked = snapshot_directory(&linked_root);
    let linked_error =
        verify_standalone_outputs_with_compilers(&linked_request, &compilers).unwrap_err();
    assert_ax0703(&linked_error, None);
    assert_eq!(snapshot_directory(&linked_root), before_linked);
}

#[test]
fn rejects_undefined_collector_symbols_for_explicit_gnu_and_legacy_verification() {
    let gnu_temporary = tempfile::tempdir().unwrap();
    let gnu_root = create_output_root(gnu_temporary.path());
    let gnu_artifacts = write_pair_with_identity(
        &gnu_root,
        elf::Class::Elf32,
        RV32_ARCH,
        0,
        elf::riscv::MACHINE,
        1,
        0,
    );
    let gnu_request = one_target_request(&gnu_root, RV32_TRIPLE, gnu_artifacts);
    let gnu_compilers = BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]);
    let before_gnu = snapshot_directory(&gnu_root);
    let gnu_error =
        verify_standalone_outputs_with_compilers(&gnu_request, &gnu_compilers).unwrap_err();
    assert_ax0703(&gnu_error, None);
    assert_eq!(snapshot_directory(&gnu_root), before_gnu);

    let legacy_temporary = tempfile::tempdir().unwrap();
    let legacy_root = create_output_root(legacy_temporary.path());
    let legacy_artifacts = write_pair_with_identity(
        &legacy_root,
        elf::Class::Elf64,
        RV64_ARCH,
        0,
        EM_X86_64,
        1,
        0,
    );
    let legacy_request = one_target_request(&legacy_root, X86_64_TRIPLE, legacy_artifacts);
    let before_legacy = snapshot_directory(&legacy_root);
    let legacy_error = verify_standalone_outputs(&legacy_request).unwrap_err();
    assert_ax0703(&legacy_error, None);
    assert_eq!(snapshot_directory(&legacy_root), before_legacy);
}

#[test]
fn rejects_missing_or_reserved_collector_sections_and_accepts_absolute_symbols() {
    for (label, symbol_section) in [
        ("nonexistent-section", 7),
        ("shn-common", SHN_COMMON),
        ("shn-xindex", SHN_XINDEX),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let output_root = create_output_root(temporary.path());
        let artifacts = write_pair_with_identity(
            &output_root,
            elf::Class::Elf32,
            RV32_ARCH,
            0,
            elf::riscv::MACHINE,
            1,
            symbol_section,
        );
        let request = one_target_request(&output_root, RV32_TRIPLE, artifacts);
        let compilers = BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]);
        let before = snapshot_directory(&output_root);

        let error = verify_standalone_outputs_with_compilers(&request, &compilers).unwrap_err();
        assert_ax0703(&error, None);
        assert_eq!(snapshot_directory(&output_root), before, "{label}");
    }

    let temporary = tempfile::tempdir().unwrap();
    let output_root = create_output_root(temporary.path());
    let artifacts = write_pair_with_identity(
        &output_root,
        elf::Class::Elf32,
        RV32_ARCH,
        0,
        elf::riscv::MACHINE,
        1,
        SHN_ABS,
    );
    let request = one_target_request(&output_root, RV32_TRIPLE, artifacts.clone());
    let compilers = BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]);
    let object = elf::read(&fs::read(&artifacts.c).unwrap()).unwrap();
    assert!(object
        .symbols
        .iter()
        .any(|symbol| { symbol.name == C_COLLECTOR_SYMBOL && symbol.home == elf::Home::Absolute }));
    let before = snapshot_directory(&output_root);
    let report = verify_standalone_outputs_with_compilers(&request, &compilers).unwrap();
    assert_identity(
        &report.targets[RV32_TRIPLE].c,
        &artifacts.c,
        elf::Class::Elf32,
    );
    assert_eq!(snapshot_directory(&output_root), before);
}

#[test]
fn rejects_symlinks_duplicate_paths_and_untrusted_path_names_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = create_output_root(temporary.path());
    let artifacts = write_pair(&output_root, elf::Class::Elf32, RV32_ARCH, 0);

    let symlink_name = "untrusted-symlink-marker.o";
    let linked = output_root.join(symlink_name);
    symlink(&artifacts.c, &linked).unwrap();
    let with_symlink = one_target_request(
        &output_root,
        RV32_TRIPLE,
        StandaloneTargetArtifacts {
            c: linked,
            cxx: artifacts.cxx.clone(),
        },
    );
    let before_symlink_check = snapshot_directory(&output_root);
    let symlink_error = verify_standalone_outputs_with_compilers(
        &with_symlink,
        &BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]),
    )
    .unwrap_err();
    assert_ax0703(&symlink_error, Some(symlink_name));
    assert_eq!(snapshot_directory(&output_root), before_symlink_check);

    let duplicate = one_target_request(
        &output_root,
        RV32_TRIPLE,
        StandaloneTargetArtifacts {
            c: artifacts.c.clone(),
            cxx: artifacts.c.clone(),
        },
    );
    let before_duplicate_check = snapshot_directory(&output_root);
    let duplicate_error = verify_standalone_outputs_with_compilers(
        &duplicate,
        &BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]),
    )
    .unwrap_err();
    assert_ax0703(&duplicate_error, None);
    assert_eq!(snapshot_directory(&output_root), before_duplicate_check);

    let traversal_marker = "untrusted-traversal-marker.o";
    let traversal = output_root.join("..").join(traversal_marker);
    let outside_path = one_target_request(
        &output_root,
        RV32_TRIPLE,
        StandaloneTargetArtifacts {
            c: traversal,
            cxx: artifacts.cxx,
        },
    );
    let before_traversal_check = snapshot_directory(&output_root);
    let traversal_error = verify_standalone_outputs_with_compilers(
        &outside_path,
        &BTreeMap::from([(RV32_TRIPLE.into(), rv32_compiler())]),
    )
    .unwrap_err();
    assert_ax0703(&traversal_error, Some(traversal_marker));
    assert_eq!(snapshot_directory(&output_root), before_traversal_check);
}
