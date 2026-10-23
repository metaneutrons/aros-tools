use super::MetaMakeLauncherConfiguration;
use crate::metamake_project::ConfiguredProject;
use aros_common::sha256_bytes;

const PROJECT: &str = "selected";
const SOURCE_WORD: &str = "Source";
const LAUNCHER: &str =
    "/selected/observer --observe-make-launch-collection /private/control deadbeef --";
const ORIGINAL_COMMAND: &str = "Source  $(TOP)/Makefile -f extra.mk # retain  spacing  ";
const RETAINED_AROS_CONFIGURATION: &str = "[AROS]\nmaketool $(HOST_MAKE) $(MKARGS) TOP=$(TOP) SRCDIR=$(SRCDIR) CURDIR=$(CURDIR) TARGET=$(TARGET)\n";
const RETAINED_AROS_TAIL: &str =
    " $(MKARGS) TOP=$(TOP) SRCDIR=$(SRCDIR) CURDIR=$(CURDIR) TARGET=$(TARGET)";
const RETAINED_CAPTURE_DIGEST: &str =
    "110d139a22783389c558c7f9dce03288269206b4fc6219922b02be587b87059f";

fn retained_aros_launcher(root: &str) -> String {
    format!("{root}/o --observe-make-launch-collection {root}/c {RETAINED_CAPTURE_DIGEST} --")
}

fn derive_retained_aros(launcher: &str) -> Result<MetaMakeLauncherConfiguration, String> {
    let original = RETAINED_AROS_CONFIGURATION.as_bytes();
    super::MetaMakeLauncherConfiguration::derive_confined_project(
        original,
        &sha256_bytes(original),
        "AROS",
        "$(HOST_MAKE)",
        launcher,
    )
}

#[test]
fn keeps_deferred_source_command_arguments_unexpanded() {
    let original = "[probe]\nmaketool $(HOST_MAKE) $(MKARGS) TOP=$(TOP) SRCDIR=$(SRCDIR) CURDIR=$(CURDIR) TARGET=$(TARGET)\n";
    let result = MetaMakeLauncherConfiguration::derive(
        original.as_bytes(),
        &sha256_bytes(original.as_bytes()),
        "probe",
        "$(HOST_MAKE)",
        "/selected/observer --capture --",
    )
    .unwrap();
    assert_eq!(result.bytes(), b"[probe]\nmaketool /selected/observer --capture -- $(MKARGS) TOP=$(TOP) SRCDIR=$(SRCDIR) CURDIR=$(CURDIR) TARGET=$(TARGET)\n");
}

fn explicit_configuration(command: &str) -> String {
    format!(
        "# bytes before the selected section stay untouched\n\
         [unselected]\n\
         # this other project is not rewritten\n\
         maketool OtherTool --keep-this\n\
         [selected]\n\
         # selected-project comment stays untouched\n\
         defaultmakefilename\tAlternate.make\n\
         defaulttarget\tall\n\
         top\t/explicit/project\n\
         ignoredir\tgenerated\n\
         add\textra/Makefile\n\
         globalvarfile\tconfig/globals\n\
         genmakefiledeps\tsrc/main.c include/main.h\n\
         genmakefilescript\ttools/generate-makefile\n\
         genglobalvarfile\ttools/generate-globals\n\
         customvalue\tkeep this value\n\
         \tmaketool\t  {command}\n\
         # comment after the selected directive\n\
         [sibling]\n\
         # sibling bytes are also preserved\n\
         defaulttarget\tother-target\n"
    )
}

fn derive(
    original: &str,
    source_word: &str,
    launcher: &str,
) -> Result<MetaMakeLauncherConfiguration, String> {
    MetaMakeLauncherConfiguration::derive(
        original.as_bytes(),
        &sha256_bytes(original.as_bytes()),
        PROJECT,
        source_word,
        launcher,
    )
}

#[test]
fn derives_exactly_one_leading_source_word_and_preserves_other_bytes_and_fields() {
    let original = explicit_configuration(ORIGINAL_COMMAND);
    let expected_command = format!("{LAUNCHER}  $(TOP)/Makefile -f extra.mk # retain  spacing  ");
    let expected = original.replacen(
        &format!("maketool\t  {ORIGINAL_COMMAND}"),
        &format!("maketool\t  {expected_command}"),
        1,
    );

    let configuration = derive(&original, SOURCE_WORD, LAUNCHER).unwrap();
    assert_eq!(configuration.bytes(), expected.as_bytes());
    assert_eq!(
        configuration.original_sha256(),
        &sha256_bytes(original.as_bytes())
    );
    assert_eq!(
        configuration.derived_sha256(),
        &sha256_bytes(expected.as_bytes())
    );

    let before = ConfiguredProject::parse(&original, PROJECT).unwrap();
    let after =
        ConfiguredProject::parse(std::str::from_utf8(configuration.bytes()).unwrap(), PROJECT)
            .unwrap();
    let mut expected_project = before;
    expected_project.make_command = Some(expected_command);
    assert_eq!(after, expected_project);
}

#[test]
fn rejects_changed_original_digest() {
    let original = explicit_configuration(ORIGINAL_COMMAND);
    let wrong_digest = sha256_bytes(b"different original configuration");
    assert!(MetaMakeLauncherConfiguration::derive(
        original.as_bytes(),
        &wrong_digest,
        PROJECT,
        SOURCE_WORD,
        LAUNCHER,
    )
    .is_err());
}

#[test]
fn rejects_nonleading_wrong_and_duplicate_source_words() {
    assert!(derive(&explicit_configuration(ORIGINAL_COMMAND), "Other", LAUNCHER).is_err());
    assert!(derive(
        &explicit_configuration("echo Source $(TOP)/Makefile"),
        SOURCE_WORD,
        LAUNCHER,
    )
    .is_err());
    assert!(derive(
        &explicit_configuration("Source Source $(TOP)/Makefile"),
        SOURCE_WORD,
        LAUNCHER,
    )
    .is_err());
}

#[test]
fn rejects_duplicate_selected_commands_and_builtin_without_explicit_directive() {
    let once = explicit_configuration(ORIGINAL_COMMAND);
    let directive = format!("\tmaketool\t  {ORIGINAL_COMMAND}\n");
    let duplicated = once.replacen(&directive, &format!("{directive}{directive}"), 1);
    assert!(derive(&duplicated, SOURCE_WORD, LAUNCHER).is_err());

    let builtin_only = "[selected]\ntop\t/explicit/project\n";
    assert!(derive(builtin_only, "make", LAUNCHER).is_err());
}

#[test]
fn rejects_relative_environment_and_shell_launcher_forms() {
    for unsafe_launcher in [
        "relative-observer --observe-make-launch-collection",
        "env OBSERVER_MODE=1 /selected/observer --observe-make-launch-collection",
        "/selected/observer; /bin/false",
    ] {
        assert!(derive(
            &explicit_configuration(ORIGINAL_COMMAND),
            SOURCE_WORD,
            unsafe_launcher,
        )
        .is_err());
    }
}

#[test]
fn rejects_derived_physical_line_overflow_and_unterminated_reference_line() {
    let long_tail = format!("Source {}", "x".repeat(220));
    let overflow = format!("[selected]\nmaketool {long_tail}\n");
    let long_absolute_launcher = format!("/selected/{}", "observer".repeat(12));
    assert!(derive(&overflow, SOURCE_WORD, &long_absolute_launcher).is_err());

    let unterminated = format!("[selected]\nmaketool {ORIGINAL_COMMAND}");
    assert!(derive(&unterminated, SOURCE_WORD, LAUNCHER).is_err());
}
#[test]
fn confined_launcher_has_one_explicit_project_and_fences_implicit_default() {
    let original = b"[probe]\nmaketool $(HOST_MAKE) -f mmakefile leaf\nignoredir .git\n";
    let derived = super::MetaMakeLauncherConfiguration::derive_confined_project(
        original,
        &aros_common::sha256_bytes(original),
        "probe",
        "$(HOST_MAKE)",
        "/selected/observer --control selected --",
    )
    .unwrap();
    assert!(derived.bytes().starts_with(b"maketool false\n[probe]\n"));
    let selected_bytes = derived.bytes().strip_prefix(b"maketool false\n").unwrap();
    let selected = super::ConfiguredProject::parse_only_project(
        std::str::from_utf8(selected_bytes).unwrap(),
        "probe",
    )
    .unwrap();
    assert_eq!(
        selected.make_command.as_deref(),
        Some("/selected/observer --control selected -- -f mmakefile leaf")
    );
    assert!(selected.ignored_directories.contains(".git"));
    let other = b"[probe]\nmaketool $(HOST_MAKE) leaf\n[other]\nmaketool make uninstrumented\n";
    assert!(
        super::MetaMakeLauncherConfiguration::derive_confined_project(
            other,
            &aros_common::sha256_bytes(other),
            "probe",
            "$(HOST_MAKE)",
            "/selected/observer --control selected --",
        )
        .is_err()
    );
    assert!(super::ConfiguredProject::parse_only_project(
        "[default]\nmaketool make\n[probe]\n",
        "probe"
    )
    .is_err());
}

#[test]
fn retained_original_maketool_form_refuses_long_launcher_and_preserves_short_tail() {
    let short_root = "/private/tmp/mi.12345678/c";
    let long_root = format!("{short_root}/{}", "x".repeat(18));
    let long_launcher = retained_aros_launcher(&long_root);
    let long_line = format!("maketool {long_launcher}{RETAINED_AROS_TAIL}\n");
    assert_eq!(long_line.len(), 278);
    let Err(error) = derive_retained_aros(&long_launcher) else {
        panic!("278-byte retained maketool line must be refused");
    };
    assert!(error.contains("physical line"), "{error}");

    let short_launcher = retained_aros_launcher(short_root);
    let derived = derive_retained_aros(&short_launcher).unwrap();
    let expected =
        format!("maketool false\n[AROS]\nmaketool {short_launcher}{RETAINED_AROS_TAIL}\n");
    assert_eq!(derived.bytes(), expected.as_bytes());
    assert_eq!(
        derived.original_sha256(),
        &sha256_bytes(RETAINED_AROS_CONFIGURATION.as_bytes())
    );
    assert_eq!(derived.derived_sha256(), &sha256_bytes(expected.as_bytes()));

    let selected_bytes = derived.bytes().strip_prefix(b"maketool false\n").unwrap();
    let selected = super::ConfiguredProject::parse_only_project(
        std::str::from_utf8(selected_bytes).unwrap(),
        "AROS",
    )
    .unwrap();
    let mut expected_selected =
        super::ConfiguredProject::parse(RETAINED_AROS_CONFIGURATION, "AROS").unwrap();
    expected_selected.make_command = Some(format!("{short_launcher}{RETAINED_AROS_TAIL}"));
    assert_eq!(selected, expected_selected);
}

#[test]
fn retained_original_maketool_line_accepts_255_bytes_and_refuses_256() {
    let root = "/private/tmp/mi.12345678/c";
    let base_launcher = retained_aros_launcher(root);
    let separator = " --observe-make-launch-collection ";
    let (program, arguments) = base_launcher.split_once(separator).unwrap();
    let base_line_bytes = format!("maketool {base_launcher}{RETAINED_AROS_TAIL}\n").len();

    for target_bytes in [255, 256] {
        let padding = target_bytes - base_line_bytes;
        let launcher = format!("{program}{}{separator}{arguments}", "x".repeat(padding));
        let physical_line = format!("maketool {launcher}{RETAINED_AROS_TAIL}\n");
        assert_eq!(physical_line.len(), target_bytes);

        let result = derive_retained_aros(&launcher);
        if target_bytes == 255 {
            assert!(result.is_ok());
        } else {
            let Err(error) = result else {
                panic!("256-byte retained maketool line must be refused");
            };
            assert!(error.contains("physical line"), "{error}");
        }
    }
}
