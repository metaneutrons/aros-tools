use super::*;

const TEMPLATES: &str = r#"%define compile_q incextra="$(TOP)/$(CURDIR)"
MESA_PROBE_QUOTE := %(incextra)
%end
%define build_linklib mmake=/A objdir="$(GENDIR)/$(CURDIR)"
%(mmake)_OBJDIR ?= %(objdir)
%compile_q
%end
%define build_module_core mmake=/A modname=/A objdir= flavour= incextra=
ifeq (%(modname),)
$(error empty module name)
endif
ifneq (%(incextra),)
%(mmake)_INCEXTRA := %(incextra)
else
%(mmake)_INCEXTRA := $(TOP)/$(CURDIR)/..
endif
ifneq (%(flavour),)
%(mmake)_DEFNAME := %(modname)_%(flavour)
else
%(mmake)_DEFNAME := %(modname)
endif
ifneq (%(objdir),)
%(mmake)_OBJDIR := %(objdir)
else
%(mmake)_OBJDIR := $(OBJDIR)/$(%(mmake)_DEFNAME)
endif
%compile_q incextra="$(%(mmake)_INCEXTRA)"
%end
%define build_module_library mmake=/A modname=/A objdir= flavour= incextra=
%build_module_core mmake="%(mmake)" modname="%(modname)" objdir="%(objdir)" flavour="%(flavour)" incextra="%(incextra)"
%end
"#;

fn probe(
    before: &str,
    invocation: &str,
    templates: &str,
) -> Result<MesaSourceCompileContext, String> {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::write(root.join("config/make.tmpl"), templates).unwrap();
    std::fs::write(
        root.join("config/make.cfg.in"),
        "OBJDIR := $(GENDIR)/$(CURDIR)\n",
    )
    .unwrap();
    let prefix = format!("{before}\n");
    let input = ContextInput {
        declaration_line: prefix.lines().count(),
        source_program: format!("{prefix}{invocation}\n"),
        source_locations: vec![None; format!("{prefix}{invocation}\n").lines().count()],
        invocation: format!("{invocation}\n"),
        selected: TargetContext {
            make_variables: std::collections::BTreeMap::from([
                ("TOP".into(), "/productive/build".into()),
                ("GENDIR".into(), "/productive/build/gen".into()),
            ]),
            ..TargetContext::default()
        },
    };
    let result = project(
        &root,
        Path::new("workbench/mesa"),
        "mesa",
        &DirVars::default(),
        &MesaMakeCoordinates {
            source_root: "/nominal/source".into(),
            build_root: "/nominal/build".into(),
            generated_root: "/nominal/build/bin/target/gen".into(),
            ports_root: "/nominal/build/bin/target/Ports".into(),
        },
        &input,
    );
    if let Ok(context) = &result {
        let expanded = expand_text(
            &input.invocation,
            &root.join("mmakefile.src"),
            &root.join("config/make.tmpl"),
            Limits::default(),
        )
        .unwrap();
        let wrapper = format!(
            concat!(
                "TOP := /nominal/build\n",
                "SRCDIR := /nominal/source\n",
                "GENDIR := /nominal/build/bin/target/gen\n",
                "CURDIR := workbench/mesa\n",
                "OBJDIR := $(GENDIR)/$(CURDIR)\n",
                "{before}\n",
                "{}\n",
                "$(info OBJECT=$(abspath $(mesa_OBJDIR)))\n",
                "$(info QUOTE=$(abspath $(MESA_PROBE_QUOTE)))\n",
                "$(if $(filter undefined,$(origin top_builddir)),,",
                "$(info PYTHON_BUILD=$(abspath $(top_builddir))))\n",
                ".PHONY: measured\n",
                "measured:\n"
            ),
            expanded.text,
            before = before,
        );
        let wrapper_path = root.join("oracle.mk");
        std::fs::write(&wrapper_path, wrapper).unwrap();
        let make = ["gmake", "make"]
            .into_iter()
            .find(|program| {
                std::process::Command::new(program)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| {
                        output.status.success() && output.stdout.starts_with(b"GNU Make")
                    })
            })
            .expect("GNU Make source-context oracle");
        let mut command = std::process::Command::new(make);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("LC_ALL", "C")
            .args([
                "--silent",
                "--no-builtin-rules",
                "--no-builtin-variables",
                "-f",
            ])
            .arg(wrapper_path)
            .arg("measured");
        let output = aros_common::run_output_with_control(
            &mut command,
            64 * 1024,
            std::time::Duration::from_secs(10),
            &aros_common::CancellationToken::default(),
        )
        .unwrap();
        assert!(
            output.status.success() && !output.timed_out && !output.cancelled,
            "{}",
            output.stderr.rendered_lossy()
        );
        let output = output.stdout.rendered_lossy();
        assert!(
            output
                .lines()
                .any(|line| line == format!("OBJECT={}", context.object_directory.display())),
            "{output}"
        );
        assert!(
            output
                .lines()
                .any(|line| line == format!("QUOTE={}", context.quote_extra.display())),
            "{output}"
        );
        if let Some(directory) = &context.python_build_directory {
            assert!(
                output
                    .lines()
                    .any(|line| line == format!("PYTHON_BUILD={}", directory.display())),
                "{output}"
            );
        } else {
            assert!(
                !output.lines().any(|line| line.starts_with("PYTHON_BUILD=")),
                "{output}"
            );
        }
    }
    result
}

#[test]
fn nested_genmf_defaults_distinguish_linklib_and_module_roots() {
    let link = probe("", "%build_linklib mmake=mesa", TEMPLATES).unwrap();
    assert_eq!(
        link.object_directory,
        Path::new("/nominal/build/bin/target/gen/workbench/mesa")
    );
    assert_eq!(link.quote_extra, Path::new("/nominal/build/workbench/mesa"));
    let module = probe(
        "",
        "%build_module_library mmake=mesa modname=Mesa",
        TEMPLATES,
    )
    .unwrap();
    assert_eq!(
        module.object_directory,
        Path::new("/nominal/build/bin/target/gen/workbench/mesa/Mesa")
    );
    assert_eq!(module.quote_extra, Path::new("/nominal/build/workbench"));
}

#[test]
fn explicit_arguments_and_prior_immediate_values_remain_source_owned() {
    for (version, name) in [("26.0.0", "Mesa26-0"), ("20.0.8", "Mesa20-0")] {
        let before = format!("VERSION := {version}\nDOTS := $(subst ., ,$(VERSION))\nMAJOR = $(word 1,$(DOTS))\nMINOR = $(word 2,$(DOTS))\nBUILD = $(MAJOR)-$(MINOR)\n");
        let context = probe(
            &before,
            "%build_module_library mmake=mesa modname=\"Mesa$(BUILD)\"",
            TEMPLATES,
        )
        .unwrap();
        assert!(context.object_directory.ends_with(name));
    }
    let result = probe("NAME := Mesa\nVERSION := 26.0.0\n", "%build_module_library mmake=mesa modname=$(NAME) objdir=\"$(OBJDIR)/$(VERSION)\" incextra=\"$(SRCDIR)/shared\"", TEMPLATES).unwrap();
    assert_eq!(
        result.object_directory,
        Path::new("/nominal/build/bin/target/gen/workbench/mesa/26.0.0")
    );
    assert_eq!(result.quote_extra, Path::new("/nominal/source/shared"));
    let flavour = probe(
        "",
        "%build_module_library mmake=mesa modname=Mesa flavour=debug",
        TEMPLATES,
    )
    .unwrap();
    assert!(flavour.object_directory.ends_with("Mesa_debug"));
    let prior = probe(
        "mesa_OBJDIR := $(GENDIR)/prior\n",
        "%build_linklib mmake=mesa objdir=\"$(GENDIR)/later\"",
        TEMPLATES,
    )
    .unwrap();
    assert_eq!(
        prior.object_directory,
        Path::new("/nominal/build/bin/target/gen/prior")
    );
}

#[test]
fn unsafe_unresolved_or_ambiguous_recipe_contexts_refuse() {
    for (args, reason) in [
        ("objdir=/outside", "beneath measured GENDIR"),
        (
            "objdir=$(UNKNOWN)",
            "cannot resolve Mesa source object directory",
        ),
        ("incextra=/outside", "leaves measured source/build roots"),
        (
            "incextra=\"$(shell touch /no-side-effects)\"",
            "cannot resolve Mesa extra quote root",
        ),
    ] {
        let error = probe(
            "",
            &format!("%build_module_library mmake=mesa modname=Mesa {args}"),
            TEMPLATES,
        )
        .unwrap_err();
        assert!(error.contains(reason), "{error}");
    }
    let no_compile = TEMPLATES.replace("%compile_q\n", "# no compile\n");
    assert!(probe("", "%build_linklib mmake=mesa", &no_compile)
        .unwrap_err()
        .contains("found 0"));
    let mixed = TEMPLATES.replace(
        "%compile_q\n",
        "%compile_q\n%compile_q incextra=\"$(TOP)/different\"\n",
    );
    assert!(probe("", "%build_linklib mmake=mesa", &mixed)
        .unwrap_err()
        .contains("found 2"));
}

#[test]
fn top_builddir_is_optional_source_owned_and_confined_to_generated_root() {
    let explicit = probe(
        "top_builddir := $(GENDIR)/workbench/mesa/26.0.0\n",
        "%build_module_library mmake=mesa modname=Mesa",
        TEMPLATES,
    )
    .unwrap();
    assert_eq!(
        explicit.python_build_directory.as_deref(),
        Some(Path::new(
            "/nominal/build/bin/target/gen/workbench/mesa/26.0.0"
        ))
    );

    let undefined = probe(
        "",
        "%build_module_library mmake=mesa modname=Mesa",
        TEMPLATES,
    )
    .unwrap();
    assert_eq!(undefined.python_build_directory, None);

    let escaping = probe(
        "top_builddir := $(GENDIR)/../outside\n",
        "%build_module_library mmake=mesa modname=Mesa",
        TEMPLATES,
    )
    .unwrap_err();
    assert!(
        escaping.contains("Python build directory must be beneath measured GENDIR"),
        "{escaping}"
    );
}
