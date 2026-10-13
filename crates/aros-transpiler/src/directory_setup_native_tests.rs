use super::collect_native_directory_setups;
use crate::dirs::DirVars;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, TargetContext};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const MAKE_CFG: &str = "AROS_DEVELOPER := $(TARGETDIR)/SYS/Developer\nGENINCDIR := $(GENDIR)/include\nAROS_INCLUDES := $(AROS_DEVELOPER)/include\n";

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    relative: PathBuf,
}

impl Fixture {
    fn new(common: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        let relative = PathBuf::from("compiler/startup");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join(&relative)).unwrap();
        fs::write(root.join("config/make.cfg.in"), MAKE_CFG).unwrap();
        fs::write(
            root.join("config/aros.cfg"),
            "# replaced by the explicit native configuration binding\n",
        )
        .unwrap();
        fs::write(root.join("config/common.mk"), common).unwrap();
        Self {
            _temporary: temporary,
            root,
            relative,
        }
    }

    fn target(&self) -> TargetContext {
        let mut native_configuration_inputs = BTreeMap::new();
        for relative in ["config/make.cfg.in", "config/common.mk"] {
            let bytes = fs::read(self.root.join(relative)).unwrap();
            native_configuration_inputs.insert(
                relative.to_owned(),
                aros_common::sha256_bytes(&bytes).to_string(),
            );
        }
        TargetContext {
            cpu32: Some(String::new()),
            make_include_bindings: BTreeMap::from([(
                "config/aros.cfg".into(),
                "config/common.mk".into(),
            )]),
            native_configuration_inputs,
            ..TargetContext::default()
        }
    }

    fn collect(
        &self,
        physical: &str,
        fallback_text: &str,
        roles: &[aros_common::SdkCompanionRole],
    ) -> (
        Vec<super::DirectorySetupDecl>,
        Vec<super::DirectorySetupRejection>,
    ) {
        fs::write(
            self.root.join(&self.relative).join("mmakefile.src"),
            physical,
        )
        .unwrap();
        let target = self.target();
        let fallback_joined = join_continuations(fallback_text);
        let (scope, line_states) = collect_vars_impl(&fallback_joined, Some(&target));
        let mut dirs = DirVars::load(&self.root);
        dirs.bind_sdk_companion_roles(roles);
        collect_native_directory_setups(
            physical,
            fallback_text,
            &self.relative,
            &scope,
            &dirs,
            &self.root,
            Some(&line_states),
            Some(&target),
        )
    }
}

fn companion_role() -> aros_common::SdkCompanionRole {
    serde_json::from_value(serde_json::json!({
        "selector_cpu": "i386",
        "target": {
            "target_triple": "i386-unknown-aros",
            "abi": {
                "source_cpu": "i386",
                "pointer_width": 32,
                "elf_class": 1,
                "elf_machine": 3,
                "endian": "little"
            }
        },
        "compiler_options": ["--target=i386-unknown-aros"],
        "library_dir": "lib32",
        "required_runtime_library": "clang_rt.builtins-i386"
    }))
    .unwrap()
}

fn physical_rule() -> &'static str {
    concat!(
        "include $(SRCDIR)/config/aros.cfg\n",
        "STARTUP_DIRS := $(AROS_LIB32)\n",
        "%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=linklibs-startup-setup\n",
    )
}

#[test]
fn physical_rule_uses_sealed_common_configuration_and_explicit_companion_root() {
    let common = concat!(
        "AROS_LIB32 := $(AROS_DEVELOPER)/lib32\n",
        "STARTUP_FROM_COMMON := $(AROS_LIB32)\n",
    );
    let fixture = Fixture::new(common);
    let (declarations, rejections) =
        fixture.collect(physical_rule(), physical_rule(), &[companion_role()]);

    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(declarations.len(), 1, "{declarations:#?}");
    assert_eq!(declarations[0].owner, "linklibs-startup-setup");
    assert_eq!(
        declarations[0].directories,
        ["${AROS_BUILD_DIR}/SYS/Developer/lib32"]
    );
}

#[test]
fn developer_lib32_is_not_admitted_without_an_explicit_companion_role() {
    let fixture = Fixture::new("AROS_LIB32 := $(AROS_DEVELOPER)/lib32\n");
    let (declarations, rejections) = fixture.collect(physical_rule(), physical_rule(), &[]);

    assert!(declarations.is_empty(), "{declarations:#?}");
    assert!(
        rejections
            .iter()
            .any(|rejection| rejection.owner == "linklibs-startup-setup"),
        "physical candidate must be reported when its Developer/lib32 root has no declared role: {rejections:#?}"
    );
}

#[test]
fn included_rule_makedirs_cannot_create_a_physical_directory_setup_owner() {
    let common = concat!(
        "AROS_LIB32 := $(AROS_DEVELOPER)/lib32\n",
        "%rule_makedirs dirs=$(AROS_LIB32) setuptarget=injected-setup\n",
    );
    let fixture = Fixture::new(common);
    let physical = "include $(SRCDIR)/config/aros.cfg\n";
    // This represents the parser's include-expanded fallback view. The common
    // fragment has a valid rule-shaped line, but it is not part of the physical
    // mmakefile.src and must not acquire ownership merely by being included.
    let fallback = format!("{physical}{common}");
    let (declarations, _) = fixture.collect(physical, &fallback, &[companion_role()]);

    assert!(
        declarations.is_empty(),
        "an included fragment must not become the producer of a physical setup target: {declarations:#?}"
    );
}

#[test]
fn unmatched_true_conditional_in_included_config_rejects_physical_candidate() {
    let common = concat!("ifeq (1,1)\n", "AROS_LIB32 := $(AROS_DEVELOPER)/lib32\n",);
    let fixture = Fixture::new(common);
    let physical = physical_rule();
    let fallback = format!("{physical}{common}");
    let (declarations, rejections) = fixture.collect(physical, &fallback, &[companion_role()]);

    assert!(
        declarations.is_empty(),
        "the malformed included conditional must not certify the physical rule: {declarations:#?}"
    );
    assert!(
        rejections
            .iter()
            .any(|rejection| rejection.owner == "linklibs-startup-setup"),
        "the physical candidate must fail closed when an included true conditional is unmatched: {rejections:#?}"
    );
}
