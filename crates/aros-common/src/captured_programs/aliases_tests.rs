use super::*;
use crate::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    measure_tree_content_cas, sha256_bytes,
};
use std::{
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
    },
    path::{Path, PathBuf},
};

const PROGRAM: &[u8] = b"synthetic regular-file compiler stub; never executed";

struct Fixture {
    _temporary: tempfile::TempDir,
    roots: Vec<PathBuf>,
    compiler: PathBuf,
    linker: PathBuf,
    third_tool: Option<PathBuf>,
}

fn executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn new_fixture(with_third_root: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let build_root = temporary.path().join("buildRoot0");
    let compiler_root = temporary.path().join("compilerRoot1");
    fs::create_dir_all(&build_root).unwrap();
    fs::create_dir_all(&compiler_root).unwrap();
    let build_root = build_root.canonicalize().unwrap();
    let compiler_root = compiler_root.canonicalize().unwrap();

    let compiler = compiler_root.join("bin/clang");
    let linker = compiler_root.join("bin/ld");
    executable(&compiler, PROGRAM);
    executable(&linker, PROGRAM);

    let mut roots = vec![build_root, compiler_root];
    let third_tool = if with_third_root {
        let third_root = temporary.path().join("thirdRoot2");
        fs::create_dir_all(&third_root).unwrap();
        let third_root = third_root.canonicalize().unwrap();
        let tool = third_root.join("bin/ar");
        executable(&tool, PROGRAM);
        roots.push(third_root);
        Some(tool)
    } else {
        None
    };

    Fixture {
        _temporary: temporary,
        roots,
        compiler,
        linker,
        third_tool,
    }
}

fn root_selection(root: &Path) -> ProgramRootSelection {
    let tree = measure_tree_content_cas(root).unwrap();
    ProgramRootSelection {
        root: root.to_owned(),
        content_sha256: tree.payload_digest_excluding(None),
    }
}

fn program(role: &str, root_index: usize, path: &Path) -> ProgramSelection {
    ProgramSelection {
        role: role.to_owned(),
        root_index,
        invocation_path: path.to_owned(),
        executable_sha256: sha256_bytes(PROGRAM),
    }
}

fn captured(fixture: &Fixture) -> CapturedProgramSet {
    let roots = fixture
        .roots
        .iter()
        .map(|root| root_selection(root))
        .collect::<Vec<_>>();
    let mut programs = vec![
        program("compiler", 1, &fixture.compiler),
        program("linker", 1, &fixture.linker),
    ];
    if let Some(third_tool) = &fixture.third_tool {
        programs.push(program("archiver", 2, third_tool));
    }
    CapturedProgramSet::capture(&roots, &programs).unwrap()
}

fn link(path: PathBuf, target: &[u8]) -> ProgramAliasLink {
    ProgramAliasLink {
        path,
        target: target.to_vec(),
    }
}

fn alias(
    invocation_path: PathBuf,
    role: &str,
    links: Vec<ProgramAliasLink>,
) -> ProgramAliasSelection {
    ProgramAliasSelection {
        alias_root_index: 0,
        invocation_path,
        role: role.to_owned(),
        links,
    }
}

fn two_hop_alias(fixture: &Fixture) -> (PathBuf, PathBuf, Vec<u8>, Vec<ProgramAliasLink>) {
    let invocation = fixture.roots[0].join("bin/build-tool");
    let second_link = fixture.roots[0].join("bin/buildalias");
    fs::create_dir_all(invocation.parent().unwrap()).unwrap();
    symlink("buildalias", &invocation).unwrap();

    // Preserve the literal doubled separator as evidence from read_link.
    let raw_absolute = format!("{}/bin//clang", fixture.roots[1].display()).into_bytes();
    symlink(std::ffi::OsStr::from_bytes(&raw_absolute), &second_link).unwrap();
    let links = vec![
        link(invocation.clone(), b"buildalias"),
        link(second_link.clone(), &raw_absolute),
    ];
    (invocation, second_link, raw_absolute, links)
}

#[test]
fn binds_ordered_relative_then_absolute_alias_and_preserves_invocation_and_raw_target() {
    let fixture = new_fixture(false);
    let (invocation, _, raw_absolute, links) = two_hop_alias(&fixture);
    // Expected root digests are measured independently before the capture.
    let set = captured(&fixture);
    let selected = alias(invocation.clone(), "compiler", links);
    let bound = set.bind_alias(&selected).unwrap();

    assert_eq!(bound.invocation_path(), invocation.as_path());
    assert_eq!(bound.program().role(), "compiler");
    assert_eq!(bound.program().resolved_path(), fixture.compiler.as_path());
    assert_eq!(selected.links[1].target, raw_absolute);
    assert!(bound.verify().is_ok());
}

#[test]
fn rejects_incomplete_reordered_and_extra_link_proofs() {
    let fixture = new_fixture(false);
    let (invocation, _, raw_absolute, links) = two_hop_alias(&fixture);
    let extra = fixture.roots[0].join("unused-link");
    symlink("buildalias", &extra).unwrap();
    let set = captured(&fixture);

    let missing = alias(
        invocation.clone(),
        "compiler",
        vec![link(invocation.clone(), b"buildalias")],
    );
    assert!(set.bind_alias(&missing).is_err());

    let mut reordered = links.clone();
    reordered.reverse();
    assert!(set
        .bind_alias(&alias(invocation.clone(), "compiler", reordered))
        .is_err());

    let mut extra_link = links;
    extra_link.push(link(extra, b"buildalias"));
    assert!(set
        .bind_alias(&alias(invocation, "compiler", extra_link))
        .is_err());

    // Keep the target bytes in scope to make the raw spelling part of this fixture explicit.
    assert!(raw_absolute.ends_with(b"/bin//clang"));
}

#[test]
fn rejects_link_cycles_and_the_declared_link_limit() {
    let fixture = new_fixture(false);
    let invocation = fixture.roots[0].join("bin/cycle-start");
    let first = fixture.roots[0].join("bin/cycle-a");
    let second = fixture.roots[0].join("bin/cycle-b");
    fs::create_dir_all(invocation.parent().unwrap()).unwrap();
    symlink("cycle-a", &invocation).unwrap();
    symlink("cycle-b", &first).unwrap();
    symlink("cycle-a", &second).unwrap();

    let set = captured(&fixture);
    let cycle = alias(
        invocation.clone(),
        "compiler",
        vec![
            link(invocation, b"cycle-a"),
            link(first, b"cycle-b"),
            link(second, b"cycle-a"),
        ],
    );
    assert!(set.bind_alias(&cycle).is_err());

    let many_fixture = new_fixture(false);
    let invocation = many_fixture.roots[0].join("bin/limit-start");
    fs::create_dir_all(invocation.parent().unwrap()).unwrap();
    let mut links = Vec::new();
    for index in 0..33 {
        let path = many_fixture.roots[0].join(format!("bin/limit-{index}"));
        let target = if index == 32 {
            format!("{}/bin/clang", many_fixture.roots[1].display())
        } else {
            format!("limit-{}", index + 1)
        };
        if index == 0 {
            symlink("limit-0", &invocation).unwrap();
            links.push(link(invocation.clone(), b"limit-0"));
        }
        symlink(&target, &path).unwrap();
        links.push(link(path, target.as_bytes()));
    }
    let set = captured(&many_fixture);
    assert!(set
        .bind_alias(&alias(invocation, "compiler", links))
        .is_err());
}

#[test]
fn rejects_relative_escape_third_root_outside_reentry_and_legacy_cross_root_capture() {
    let fixture = new_fixture(true);
    let bin = fixture.roots[0].join("bin");
    fs::create_dir_all(&bin).unwrap();
    let invocation = bin.join("escape");
    symlink("../../../compilerRoot1/bin/clang", &invocation).unwrap();
    let escape = alias(
        invocation.clone(),
        "compiler",
        vec![link(invocation, b"../../../compilerRoot1/bin/clang")],
    );

    let third_invocation = bin.join("third-root");
    let third_target = fixture.third_tool.as_ref().unwrap();
    let third_raw = third_target.as_os_str().as_bytes();
    symlink(third_target, &third_invocation).unwrap();
    let third = alias(
        third_invocation.clone(),
        "compiler",
        vec![link(third_invocation, third_raw)],
    );

    let external = tempfile::tempdir().unwrap();
    let external_return = external.path().join("return-to-selected-compiler");
    symlink(&fixture.compiler, &external_return).unwrap();
    let outside_invocation = bin.join("outside-reentry");
    symlink(&external_return, &outside_invocation).unwrap();
    let outside_raw = external_return.as_os_str().as_bytes();
    let outside = alias(
        outside_invocation.clone(),
        "compiler",
        vec![link(outside_invocation, outside_raw)],
    );

    // A route that crosses into the compiler root and then re-enters the build root is refused.
    let reentry_start = bin.join("reentry-start");
    let compiler_redirect = fixture.roots[1].join("bin/redirect");
    let build_reentry = fixture.roots[0].join("bin/reentry");
    let build_finish = fixture.roots[0].join("bin/finish");
    symlink(&compiler_redirect, &reentry_start).unwrap();
    symlink(&build_reentry, &compiler_redirect).unwrap();
    symlink(&build_finish, &build_reentry).unwrap();
    symlink(fixture.compiler.as_os_str(), &build_finish).unwrap();
    let reentry = alias(
        reentry_start.clone(),
        "compiler",
        vec![
            link(reentry_start, compiler_redirect.as_os_str().as_bytes()),
            link(
                compiler_redirect.clone(),
                build_reentry.as_os_str().as_bytes(),
            ),
            link(build_reentry, build_finish.as_os_str().as_bytes()),
            link(
                build_finish.clone(),
                fixture.compiler.as_os_str().as_bytes(),
            ),
        ],
    );

    // Ordinary capture remains confined to the selected root.
    let direct = fixture.roots[0].join("bin/direct-cross-root");
    symlink(fixture.compiler.as_os_str(), &direct).unwrap();
    let set = captured(&fixture);
    assert!(set.bind_alias(&escape).is_err());
    assert!(set.bind_alias(&third).is_err());
    assert!(set.bind_alias(&outside).is_err());
    assert!(set.bind_alias(&reentry).is_err());

    let roots = fixture
        .roots
        .iter()
        .map(|root| root_selection(root))
        .collect::<Vec<_>>();
    assert!(CapturedProgramSet::capture(&roots, &[program("legacy", 0, &direct)],).is_err());
}

#[test]
fn rejects_wrong_role_wrong_file_and_raw_target_even_when_bytes_match() {
    let fixture = new_fixture(false);
    let same_bytes_other_file = fixture.roots[1].join("bin/clang-copy");
    executable(&same_bytes_other_file, PROGRAM);
    let bin = fixture.roots[0].join("bin");
    fs::create_dir_all(&bin).unwrap();

    let compiler_target = format!("{}/bin/clang", fixture.roots[1].display());
    let wrong_role_invocation = bin.join("wrong-role");
    symlink(&compiler_target, &wrong_role_invocation).unwrap();
    let wrong_role_link = vec![link(
        wrong_role_invocation.clone(),
        compiler_target.as_bytes(),
    )];

    let other_target = format!("{}/bin/clang-copy", fixture.roots[1].display());
    let wrong_file_invocation = bin.join("wrong-file");
    symlink(&other_target, &wrong_file_invocation).unwrap();
    let wrong_file_link = vec![link(wrong_file_invocation.clone(), other_target.as_bytes())];

    let doubled_target = format!("{}/bin//clang", fixture.roots[1].display());
    let raw_mismatch_invocation = bin.join("raw-mismatch");
    symlink(&doubled_target, &raw_mismatch_invocation).unwrap();

    // Capture expected root digests after fixture creation and before capture.
    let set = captured(&fixture);
    assert!(set
        .bind_alias(&alias(wrong_role_invocation, "linker", wrong_role_link,))
        .is_err());
    assert!(set
        .bind_alias(&alias(wrong_file_invocation, "compiler", wrong_file_link,))
        .is_err());
    let wrong_raw_proof = alias(
        raw_mismatch_invocation.clone(),
        "compiler",
        vec![link(raw_mismatch_invocation, compiler_target.as_bytes())],
    );
    assert!(set.bind_alias(&wrong_raw_proof).is_err());
}

#[test]
fn preserves_directory_target_for_pending_tool_and_rejects_file_target_suffixes() {
    let fixture = new_fixture(false);
    let invocation = fixture.roots[0].join("bin/toolchain/clang");
    let directory_alias = fixture.roots[0].join("bin/toolchain");
    fs::create_dir_all(directory_alias.parent().unwrap()).unwrap();
    let directory_target = format!("{}/bin/", fixture.roots[1].display());
    symlink(&directory_target, &directory_alias).unwrap();
    let set = captured(&fixture);
    let bound = set
        .bind_alias(&alias(
            invocation,
            "compiler",
            vec![link(directory_alias, directory_target.as_bytes())],
        ))
        .unwrap();
    assert_eq!(bound.program().resolved_path(), fixture.compiler.as_path());

    for suffix in ["/", "/."] {
        let fixture = new_fixture(false);
        let invocation = fixture.roots[0].join("bin/final-file");
        fs::create_dir_all(invocation.parent().unwrap()).unwrap();
        let target = format!("{}/bin/clang{suffix}", fixture.roots[1].display());
        symlink(&target, &invocation).unwrap();
        let set = captured(&fixture);
        assert!(set
            .bind_alias(&alias(
                invocation.clone(),
                "compiler",
                vec![link(invocation, target.as_bytes())],
            ))
            .is_err());
    }

    let fixture = new_fixture(false);
    let invocation = fixture.roots[0].join("bin/implementation-defined-root");
    fs::create_dir_all(invocation.parent().unwrap()).unwrap();
    let target = format!(
        "//{}",
        fixture.compiler.strip_prefix("/").unwrap().display()
    );
    symlink(&target, &invocation).unwrap();
    let set = captured(&fixture);
    assert!(set
        .bind_alias(&alias(
            invocation.clone(),
            "compiler",
            vec![link(invocation, target.as_bytes())],
        ))
        .is_err());
}

#[test]
fn bound_alias_verification_rejects_root_and_link_drift() {
    let fixture = new_fixture(false);
    let (invocation, second_link, _, links) = two_hop_alias(&fixture);
    let set = captured(&fixture);
    let bound = set
        .bind_alias(&alias(invocation, "compiler", links))
        .unwrap();
    assert!(bound.verify().is_ok());

    fs::remove_file(&second_link).unwrap();
    symlink(
        format!("{}/bin/clang", fixture.roots[1].display()),
        &second_link,
    )
    .unwrap();
    assert!(bound.verify().is_err());
}
