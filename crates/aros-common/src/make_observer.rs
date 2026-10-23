//! Pure GNU Make fragments for observing the initial variable namespace.
//!
//! An observation is meaningful only for the exact sealed Make invocation that
//! produced it and only when that invocation completed successfully. Pass
//! [`pre_file_fragment`] as a GNU Make `--eval` argument before the first
//! makefile is read, or use [`source_prelude_fragment`] at the separately
//! documented pre-selected-file boundary. The resulting metadata is evidence
//! about Make's state; it does not authorize any operation.
//!
//! Preserve the invocation's exact flags. In particular, `-r` disables built-in
//! rules, while `-rR` also removes built-in variables and therefore changes the
//! observed namespace. The observer's iterator variables are `foreach`-local;
//! its temporary newline variable is explicitly undefined after use.

use crate::make_initial_values::{
    validate_command_line_names, validate_environment_names, MAKE_COMMAND_LINE_VALUES_METADATA_KEY,
    MAKE_ENVIRONMENT_VALUES_METADATA_KEY, MAKE_INITIAL_VALUES_METADATA_KEY,
    MAKE_PARSER_CONTROLS_METADATA_KEY,
};
use crate::make_namespace::MAKE_NAMESPACE_METADATA_KEY;
use std::collections::{BTreeMap, BTreeSet};

/// Construct the bounded initial file/default-value observation fragment.
///
/// Values are read with `$(value ...)` and percent-escaped before being emitted.
/// Only ordinary variables with `file` or `default` origin are included; dot
/// variables and the observer's own helpers are excluded. No caller-provided
/// names or values are interpolated into the script.
#[must_use]
pub fn initial_values_fragment() -> String {
    let encoded = encoded_value("__AROS_INITIAL_VALUES_ITEM", "__AROS_INITIAL_VALUES_NL");
    let names = "$(foreach __AROS_INITIAL_VALUES_ITEM,$(filter-out .% __AROS_INITIAL_VALUES_ITEM __AROS_INITIAL_VALUES_NL,$(sort $(.VARIABLES))),$(if $(filter file default,$(origin $(__AROS_INITIAL_VALUES_ITEM))),$(__AROS_INITIAL_VALUES_ITEM)))";
    format!(
        "\
$(if $(filter undefined,$(origin __AROS_INITIAL_VALUES_ITEM)),,$(error Make initial values iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_INITIAL_VALUES_NL)),,$(error Make initial values newline collision))\n\
define __AROS_INITIAL_VALUES_NL\n\n\nendef\n\
$(info {MAKE_INITIAL_VALUES_METADATA_KEY}=v1;$(foreach __AROS_INITIAL_VALUES_ITEM,{names},$(__AROS_INITIAL_VALUES_ITEM)|$(origin $(__AROS_INITIAL_VALUES_ITEM))|$(flavor $(__AROS_INITIAL_VALUES_ITEM))|{encoded};)end)\n\
undefine __AROS_INITIAL_VALUES_NL\n"
    )
}

fn encoded_value(iterator: &str, newline: &str) -> String {
    let mut encoded = format!("$(value $({iterator}))");
    let newline = format!("$({newline})");
    for (from, to) in [
        ("%", "%25"),
        (newline.as_str(), "%0A"),
        // A literal leading TAB is consumed as function-name whitespace.
        // Expand it from a nonempty synthetic argument instead.
        ("$(subst x,,x\t)", "%09"),
        ("|", "%7C"),
        (";", "%3B"),
    ] {
        encoded = format!("$(subst {from},{to},{encoded})");
    }
    encoded
}

/// Observe the GNU parser control independently of ordinary defaults.
///
/// Only file/default origin is captured: no raw environment/CLI opt-in is implied.
/// This packet contains measurements, not default-value substitutions.
#[must_use]
pub fn parser_controls_fragment() -> String {
    let names = "$(foreach __AROS_PARSER_CONTROLS_ITEM,$(filter .RECIPEPREFIX,$(.VARIABLES)),$(if $(filter file default,$(origin $(__AROS_PARSER_CONTROLS_ITEM))),$(__AROS_PARSER_CONTROLS_ITEM)))";
    let encoded = encoded_value("__AROS_PARSER_CONTROLS_ITEM", "__AROS_PARSER_CONTROLS_NL");
    format!("\
$(if $(filter undefined,$(origin __AROS_PARSER_CONTROLS_ITEM)),,$(error Make parser controls iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_PARSER_CONTROLS_NL)),,$(error Make parser controls newline collision))\n\
define __AROS_PARSER_CONTROLS_NL\n\n\nendef\n\
$(info {MAKE_PARSER_CONTROLS_METADATA_KEY}=v1;$(foreach __AROS_PARSER_CONTROLS_ITEM,{names},$(__AROS_PARSER_CONTROLS_ITEM)|$(origin $(__AROS_PARSER_CONTROLS_ITEM))|$(flavor $(__AROS_PARSER_CONTROLS_ITEM))|{encoded};)end)\n\
undefine __AROS_PARSER_CONTROLS_NL\n")
}

/// Guard the opt-in recipe-control iterator before any metadata is emitted.
#[must_use]
pub const fn recipe_controls_guard() -> &'static str {
    "$(if $(filter undefined,$(origin __AROS_RECIPE_CONTROLS_ITEM)),,$(error Make recipe controls iterator collision))\n$(if $(filter undefined,$(origin __AROS_RECIPE_CONTROLS_NL)),,$(error Make recipe controls newline collision))\n"
}

/// Observe raw file/default `.SHELLFLAGS` without changing legacy packets.
///
/// Append only in an explicitly selected recipe-control observation. This is
/// measurement, never an invented `-c` fallback or shell execution authority.
#[must_use]
pub fn recipe_controls_fragment() -> String {
    let names = "$(foreach __AROS_RECIPE_CONTROLS_ITEM,$(filter .SHELLFLAGS,$(.VARIABLES)),$(if $(filter file default,$(origin $(__AROS_RECIPE_CONTROLS_ITEM))),$(__AROS_RECIPE_CONTROLS_ITEM)))";
    let encoded = encoded_value("__AROS_RECIPE_CONTROLS_ITEM", "__AROS_RECIPE_CONTROLS_NL");
    format!("{}define __AROS_RECIPE_CONTROLS_NL\n\n\nendef\n$(info {}=v1;$(foreach __AROS_RECIPE_CONTROLS_ITEM,{names},$(__AROS_RECIPE_CONTROLS_ITEM)|$(origin $(__AROS_RECIPE_CONTROLS_ITEM))|$(flavor $(__AROS_RECIPE_CONTROLS_ITEM))|{encoded};)end)\nundefine __AROS_RECIPE_CONTROLS_NL\n",
        recipe_controls_guard(), crate::make_initial_values::MAKE_RECIPE_CONTROLS_METADATA_KEY)
}

/// Separately observe GNU's automatic/simple CLI-propagation scalar.
///
/// Explicit recipe observers opt in; legacy namespace/default/recipe-control
/// memberships remain unchanged. Never reconstruct escaping from argv or log
/// this private packet. This is measured data, not an execution capability.
#[must_use]
pub fn recipe_engine_values_fragment() -> String {
    let name = crate::make_initial_values::MAKE_CLI_PROPAGATION_VARIABLE;
    let names = format!("$(foreach __AROS_RECIPE_ENGINE_ITEM,$(filter {name},$(.VARIABLES)),$(if $(filter automatic,$(origin $(__AROS_RECIPE_ENGINE_ITEM))),$(__AROS_RECIPE_ENGINE_ITEM)))");
    let encoded = encoded_value("__AROS_RECIPE_ENGINE_ITEM", "__AROS_RECIPE_ENGINE_NL");
    format!("$(if $(filter undefined,$(origin __AROS_RECIPE_ENGINE_ITEM)),,$(error Make recipe engine iterator collision))\n$(if $(filter undefined,$(origin __AROS_RECIPE_ENGINE_NL)),,$(error Make recipe engine newline collision))\ndefine __AROS_RECIPE_ENGINE_NL\n\n\nendef\n$(info {}=v1;$(foreach __AROS_RECIPE_ENGINE_ITEM,{names},$(__AROS_RECIPE_ENGINE_ITEM)|$(origin $(__AROS_RECIPE_ENGINE_ITEM))|$(flavor $(__AROS_RECIPE_ENGINE_ITEM))|{encoded};)end)\nundefine __AROS_RECIPE_ENGINE_NL\n",
        crate::make_initial_values::MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY)
}

/// Explicitly opt in to observing every expected command-line raw value/flavor.
///
/// Append to the selected phase's normal namespace/default-values fragment.
/// Neither the legacy observer nor this fragment discovers environment values.
/// The complete expected CLI name set must be independently selected from the
/// closed invocation, never the operator's ambient process. Missing/extra CLI
/// names and helper collisions fail before any CLI value is read or emitted.
/// Values may be sensitive: retain output privately and never log the packet.
/// This does not authenticate invocation, namespace completeness or producer.
/// It is not an argv-effect fence: immediate command-line assignments and GNU
/// startup options can run effects before this fragment. The caller must admit
/// those inputs separately before starting the closed native observation.
///
/// # Errors
/// Rejects oversized sets/fragments or interpolation-unsafe variable names.
/// Non-alphanumeric names other than `_` and `-` remain explicitly unsupported.
pub fn command_line_values_fragment(expected: &BTreeSet<String>) -> crate::Result<String> {
    validate_command_line_names(expected)?;
    let names = expected.iter().cloned().collect::<Vec<_>>().join(" ");
    let actual = "$(foreach __AROS_COMMAND_VALUES_ITEM,$(sort $(.VARIABLES)),$(if $(filter command line,$(origin $(__AROS_COMMAND_VALUES_ITEM))),$(__AROS_COMMAND_VALUES_ITEM)))";
    let encoded = encoded_value("__AROS_COMMAND_VALUES_ITEM", "__AROS_COMMAND_VALUES_NL");
    let fragment = format!(
        "\
$(if $(filter undefined,$(origin __AROS_COMMAND_VALUES_ITEM)),,$(error Make command values iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_COMMAND_VALUES_NL)),,$(error Make command values newline collision))\n\
$(if $(filter-out {names},{actual}),$(error Unexpected command-line variable membership))\n\
$(if $(filter-out {actual},{names}),$(error Missing command-line variable membership))\n\
define __AROS_COMMAND_VALUES_NL\n\n\nendef\n\
$(info {MAKE_COMMAND_LINE_VALUES_METADATA_KEY}=v1;$(foreach __AROS_COMMAND_VALUES_ITEM,{names},$(__AROS_COMMAND_VALUES_ITEM)|$(origin $(__AROS_COMMAND_VALUES_ITEM))|$(flavor $(__AROS_COMMAND_VALUES_ITEM))|{encoded};)end)\n\
undefine __AROS_COMMAND_VALUES_NL\n"
    );
    if fragment.len() > 128 * 1024 {
        return Err(crate::ArosError::Configuration {
            file: "Make command-line observer".into(),
            message: "command-line observation fragment exceeds its byte limit".into(),
        });
    }
    Ok(fragment)
}

/// Explicitly capture a complete selected set of native environment-origin values.
///
/// This is separate from the default observer: values may be sensitive and the
/// caller must explicitly opt in, retain output privately, and bind an exact
/// environment-cleared native invocation. Supply the complete expected name set
/// from an independently bound namespace observation at the same phase/context;
/// do not discover the operator's ambient environment or guess engine names.
/// The set includes native-created environment-origin bindings; original process
/// membership before CLI overrides remains a separate required input. This
/// fragment does not authenticate either observation or authorize execution.
///
/// # Errors
/// Rejects unsafe/oversized name sets and bounded-fragment overflow. Native
/// helper collisions and different membership refuse before raw values emit.
pub fn environment_values_fragment(expected: &BTreeSet<String>) -> crate::Result<String> {
    validate_environment_names(expected)?;
    let names = expected.iter().cloned().collect::<Vec<_>>().join(" ");
    let actual = "$(foreach __AROS_ENVIRONMENT_VALUES_ITEM,$(sort $(.VARIABLES)),$(if $(filter-out environment,$(origin $(__AROS_ENVIRONMENT_VALUES_ITEM))),,$(__AROS_ENVIRONMENT_VALUES_ITEM)))";
    let encoded = encoded_value(
        "__AROS_ENVIRONMENT_VALUES_ITEM",
        "__AROS_ENVIRONMENT_VALUES_NL",
    );
    let fragment = format!(
        "\
$(if $(filter undefined,$(origin __AROS_ENVIRONMENT_VALUES_ITEM)),,$(error Make environment values iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_ENVIRONMENT_VALUES_NL)),,$(error Make environment values newline collision))\n\
$(if $(strip $(foreach __AROS_ENVIRONMENT_VALUES_ITEM,$(sort $(.VARIABLES)),$(if $(filter environment,$(origin $(__AROS_ENVIRONMENT_VALUES_ITEM))),$(if $(filter override,$(origin $(__AROS_ENVIRONMENT_VALUES_ITEM))),blocked)))),$(error Environment-override origin is unsupported))\n\
$(if $(filter-out {names},{actual}),$(error Unexpected environment variable membership))\n\
$(if $(filter-out {actual},{names}),$(error Missing environment variable membership))\n\
define __AROS_ENVIRONMENT_VALUES_NL\n\n\nendef\n\
$(info {MAKE_ENVIRONMENT_VALUES_METADATA_KEY}=v1;$(foreach __AROS_ENVIRONMENT_VALUES_ITEM,{names},$(__AROS_ENVIRONMENT_VALUES_ITEM)|$(origin $(__AROS_ENVIRONMENT_VALUES_ITEM))|$(flavor $(__AROS_ENVIRONMENT_VALUES_ITEM))|{encoded};)end)\n\
undefine __AROS_ENVIRONMENT_VALUES_NL\n"
    );
    if fragment.len() > 128 * 1024 {
        return Err(crate::ArosError::Configuration {
            file: "Make environment observer".into(),
            message: "environment observation fragment exceeds its byte limit".into(),
        });
    }
    Ok(fragment)
}

/// Construct the pre-first-file Make observation intended for one `--eval`
/// argument of the exact sealed invocation being measured.
///
/// All helper origins are checked before either metadata line is emitted. The
/// namespace record contains only sorted variable names and origins. It is
/// followed by [`initial_values_fragment`] and the separate
/// [`parser_controls_fragment`], which report eligible raw values using the
/// established bounded encoding without changing legacy default membership.
#[must_use]
pub fn pre_file_fragment() -> String {
    format!(
        "\
$(if $(filter undefined,$(origin __AROS_NAMESPACE_ITEM)),,$(error Make namespace iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_INITIAL_VALUES_ITEM)),,$(error Make initial values iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_INITIAL_VALUES_NL)),,$(error Make initial values newline collision))\n\
$(if $(filter undefined,$(origin __AROS_PARSER_CONTROLS_ITEM)),,$(error Make parser controls iterator collision))\n\
$(if $(filter undefined,$(origin __AROS_PARSER_CONTROLS_NL)),,$(error Make parser controls newline collision))\n\
$(info {MAKE_NAMESPACE_METADATA_KEY}=v1:$(words $(sort $(.VARIABLES)));$(foreach __AROS_NAMESPACE_ITEM,$(sort $(.VARIABLES)),$(__AROS_NAMESPACE_ITEM)|$(origin $(__AROS_NAMESPACE_ITEM));)end)\n\
{}{}",
        initial_values_fragment(), parser_controls_fragment()
    )
}

/// Observe immediately before the selected source file without using `--eval`.
///
/// Read this target-free fragment as the first explicit Make input. Its normal
/// assignment clears the observer input-list contribution but retains stronger
/// command-line priority. The selected source is the next explicit input. This
/// is a pre-selected-file transition, not a claim that no file was read: the
/// observed list has already acquired native file origin/flavor unless a
/// stronger binding prevailed. Preserve that phase distinction in the caller.
/// No target, recipe, default goal or Make flag is introduced by this fragment.
/// The caller must separately prevent remaking every independently admitted
/// input, including this observer (for example with explicit `--old-file`
/// arguments using each exact Make read identity, not merely a canonical-path
/// alias). Dry-run mode alone does not prevent Makefile-remake recipes.
/// Validate the exact explicit environment and assignments with
/// [`validate_source_prelude_inputs`] before starting Make, not after automatic
/// MAKEFILES inputs or inherited options might already have taken effect.
#[must_use]
pub fn source_prelude_fragment() -> String {
    format!("MAKEFILE_LIST :=\n{}", pre_file_fragment())
}

/// Refuse unmodeled input preloading and inherited option sources before launch.
///
/// Check the exact maps the caller will pass to an environment-cleared process.
/// Empty or absent control values are supported. In the environment only,
/// MAKEFLAGS/MFLAGS may contain one compact short-option word using r/R/n/s,
/// with an optional leading dash, optionally followed by the exact display-only
/// option `--no-print-directory`. That option is also accepted on its own.
/// These switches neither select extra inputs nor contain expressions; they
/// remain unchanged in the actual invocation.
/// All other nonempty forms need explicit option/preloaded-source modeling;
/// never silently clear or normalize them to pass this gate.
/// This does not validate argv, input ownership or a complete execution receipt.
///
/// # Errors
/// Refuses nonempty preload/unknown option forms and all nonempty command-line
/// control assignments. Only the control name enters the diagnostic.
pub fn validate_source_prelude_inputs(
    environment: &BTreeMap<String, String>,
    command_variables: &BTreeMap<String, String>,
) -> crate::Result<()> {
    for name in ["MAKEFILES", "MAKEFLAGS", "GNUMAKEFLAGS", "MFLAGS"] {
        let unsafe_environment = environment.get(name).is_some_and(|value| {
            !(value.is_empty()
                || matches!(name, "MAKEFLAGS" | "MFLAGS") && closed_inherited_flags(value))
        });
        if unsafe_environment
            || command_variables
                .get(name)
                .is_some_and(|value| !value.is_empty())
        {
            return Err(crate::ArosError::Configuration {
                file: "Make source prelude".into(),
                message: format!(
                    "nonempty {name} needs bound native option/preloaded-input modeling"
                ),
            });
        }
    }
    Ok(())
}

fn closed_inherited_flags(value: &str) -> bool {
    if value == "--no-print-directory" {
        return true;
    }
    let value = value.strip_suffix(" --no-print-directory").unwrap_or(value);
    let flags = value.strip_prefix('-').unwrap_or(value);
    !flags.is_empty()
        && flags
            .bytes()
            .all(|byte| matches!(byte, b'r' | b'R' | b'n' | b's'))
}

/// Refuse inherited or Source-exported noninteractive shell startup hooks.
///
/// Both inputs are independently held process/export maps, not ambient reads.
/// Empty or absent hooks are inactive. Do not merge first: a later empty export
/// must not silently excuse an unmodeled inherited startup configuration.
/// This does not authenticate a shell, loader, flags or complete environment.
///
/// # Errors
/// Refuses any nonempty `ENV` or `BASH_ENV` without exposing its value.
pub fn validate_recipe_startup_hooks(
    inherited: &BTreeMap<String, String>,
    source_exports: &BTreeMap<String, String>,
) -> crate::Result<()> {
    if [inherited, source_exports].into_iter().any(|values| {
        ["ENV", "BASH_ENV"]
            .into_iter()
            .any(|name| values.get(name).is_some_and(|value| !value.is_empty()))
    }) {
        return Err(crate::ArosError::Configuration {
            file: "Make recipe startup".into(),
            message: "unmodeled recipe shell startup hook".into(),
        });
    }
    Ok(())
}

/// Refuse unmodeled POSIX-shell startup, option, imported-function and loader
/// controls in independently held inherited and Source-export maps.
///
/// Empty ENV/BASH_ENV are inactive; unrelated data is preserved, not filtered.
/// This is environment admission, not executable/dialect authentication.
/// # Errors
/// Refuses active hooks or modeled control names without exposing values.
pub fn validate_posix_shell_environment(
    inherited: &BTreeMap<String, String>,
    source_exports: &BTreeMap<String, String>,
) -> crate::Result<()> {
    validate_recipe_startup_hooks(inherited, source_exports)?;
    if [inherited, source_exports].into_iter().any(|values| {
        values.keys().any(|key| {
            matches!(
                key.as_str(),
                "IFS"
                    | "CDPATH"
                    | "SHELLOPTS"
                    | "BASHOPTS"
                    | "GLOBIGNORE"
                    | "POSIXLY_CORRECT"
                    | "BASH_COMPAT"
                    | "BASH_XTRACEFD"
                    | "BASH_ARGV0"
            ) || key.starts_with("BASH_FUNC_")
                || key.starts_with("LD_")
                || key.starts_with("DYLD_")
        })
    }) {
        return Err(crate::ArosError::Configuration {
            file: "Make shell controls".into(),
            message: "unmodeled shell option, function or loader controls".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{initial_values_fragment, pre_file_fragment, validate_source_prelude_inputs};

    #[test]
    fn recipe_startup_hooks_require_both_maps_to_be_inactive_without_value_disclosure() {
        let empty = std::collections::BTreeMap::new();
        let inactive = std::collections::BTreeMap::from([
            ("ENV".into(), String::new()),
            ("BASH_ENV".into(), String::new()),
            ("UNRELATED".into(), "preserved".into()),
        ]);
        assert!(super::validate_recipe_startup_hooks(&empty, &inactive).is_ok());
        assert!(super::validate_recipe_startup_hooks(&inactive, &empty).is_ok());
        for hook in ["ENV", "BASH_ENV"] {
            for value in ["private-hook-path", " "] {
                let active = std::collections::BTreeMap::from([(hook.into(), value.into())]);
                for (inherited, exports) in [(&active, &inactive), (&inactive, &active)] {
                    let error =
                        super::validate_recipe_startup_hooks(inherited, exports).unwrap_err();
                    assert!(!error.to_string().contains("private-hook-path"));
                }
            }
        }
        assert_eq!(inactive["UNRELATED"], "preserved");
    }
    use std::collections::BTreeMap;

    #[test]
    fn environment_capture_is_opt_in_and_checks_membership_before_raw_output() {
        use super::{environment_values_fragment, MAKE_ENVIRONMENT_VALUES_METADATA_KEY};
        use std::collections::BTreeSet;
        let names = BTreeSet::from(["SYNTHETIC".into(), "1IMPORTED-NAME".into()]);
        let fragment = environment_values_fragment(&names).unwrap();
        let output = fragment
            .find(&format!("$(info {MAKE_ENVIRONMENT_VALUES_METADATA_KEY}="))
            .unwrap();
        for guard in [
            "iterator collision",
            "newline collision",
            "Environment-override origin",
            "Unexpected environment",
            "Missing environment",
        ] {
            assert!(fragment.find(guard).unwrap() < output);
        }
        assert!(fragment.contains("$(value $(__AROS_ENVIRONMENT_VALUES_ITEM))"));
        assert!(fragment.ends_with("undefine __AROS_ENVIRONMENT_VALUES_NL\n"));
        assert!(!pre_file_fragment().contains(MAKE_ENVIRONMENT_VALUES_METADATA_KEY));
        for name in [".BAD", "A%", "A,B", "A B", "__AROS_ENVIRONMENT_VALUES_NL"] {
            assert!(environment_values_fragment(&BTreeSet::from([name.into()])).is_err());
        }
    }

    #[test]
    fn input_control_sources_are_checked_without_disclosing_values() {
        let empty = BTreeMap::new();
        assert!(validate_source_prelude_inputs(&empty, &empty).is_ok());
        for name in ["MAKEFILES", "MAKEFLAGS", "GNUMAKEFLAGS", "MFLAGS"] {
            let inactive = BTreeMap::from([(name.into(), String::new())]);
            assert!(validate_source_prelude_inputs(&inactive, &inactive).is_ok());
            let active = BTreeMap::from([(name.into(), "private-control-value".into())]);
            for (environment, assignments) in [(&active, &empty), (&empty, &active)] {
                let error = validate_source_prelude_inputs(environment, assignments)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(name));
                assert!(!error.contains("private-control-value"));
            }
        }
    }

    #[test]
    fn inherited_no_print_directory_option_is_literal_and_preserved() {
        let empty = BTreeMap::new();
        for name in ["MAKEFLAGS", "MFLAGS"] {
            for flags in [
                "rs --no-print-directory",
                "-rs --no-print-directory",
                "--no-print-directory",
            ] {
                let environment = BTreeMap::from([(name.into(), flags.into())]);
                let before = environment.clone();
                validate_source_prelude_inputs(&environment, &empty).unwrap();
                assert_eq!(environment, before);
                assert!(validate_source_prelude_inputs(&empty, &environment).is_err());
            }
            for flags in [
                "rs --no-print-directory --eval=private",
                "rs --no-print-directory=private",
                "rs --no-print-directory --file=private",
                "--no-print-directory $(private)",
                "rs  --no-print-directory",
                "rs --no-print-directory ",
                "rs\n--no-print-directory",
            ] {
                let error = validate_source_prelude_inputs(
                    &BTreeMap::from([(name.into(), flags.into())]),
                    &empty,
                )
                .unwrap_err()
                .to_string();
                assert!(error.contains(name));
                assert!(!error.contains("private"));
            }
        }
    }

    #[test]
    fn compact_inherited_flags_are_preserved_and_other_control_forms_refuse() {
        let empty = BTreeMap::new();
        for name in ["MAKEFLAGS", "MFLAGS"] {
            for flags in ["r", "rs", "-rs", "rRns", "-rRns", "ssrr"] {
                let environment = BTreeMap::from([(name.into(), flags.into())]);
                let before = environment.clone();
                validate_source_prelude_inputs(&environment, &empty).unwrap();
                assert_eq!(environment, before);
                assert!(validate_source_prelude_inputs(&empty, &environment).is_err());
            }
            for flags in [
                "-",
                "--",
                " rs",
                "rs ",
                "r s",
                "re",
                "j3",
                "--eval=private",
                "rs --eval=private",
                "$(private)",
                "rs\n",
            ] {
                let environment = BTreeMap::from([(name.into(), flags.into())]);
                let error = validate_source_prelude_inputs(&environment, &empty)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains(name));
                assert!(!error.contains("private"));
            }
        }
        for name in ["MAKEFILES", "GNUMAKEFLAGS"] {
            assert!(validate_source_prelude_inputs(
                &BTreeMap::from([(name.into(), "rs".into())]),
                &empty
            )
            .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn admitted_inherited_switches_match_native_gnu_argument_effects() {
        use std::{
            io::Write as _,
            process::{Command, Stdio},
        };
        let run = |environment: BTreeMap<String, String>, arguments: &[&str]| {
            validate_source_prelude_inputs(&environment, &BTreeMap::new()).unwrap();
            let mut command = Command::new(if cfg!(target_os = "macos") {
                "gmake"
            } else {
                "make"
            });
            command
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .envs(&environment)
                .args(arguments)
                .args(["--no-print-directory", "-f", "/dev/stdin", "probe"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(
                    b"$(info FLAG_EFFECT=$(origin CC)|$(flavor CC))\n.PHONY: probe\nprobe:\n",
                )
                .unwrap();
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "synthetic inherited GNU option observation failed"
            );
            result.stdout
        };
        for (flags, arguments, expected) in [
            (
                "rs",
                vec!["-r", "-s"],
                b"FLAG_EFFECT=default|recursive\n".as_slice(),
            ),
            (
                "rRs",
                vec!["-r", "-R", "-s"],
                b"FLAG_EFFECT=undefined|undefined\n".as_slice(),
            ),
        ] {
            let environment = BTreeMap::from([
                ("MAKEFLAGS".into(), flags.into()),
                ("MFLAGS".into(), format!("-{flags}")),
            ]);
            let inherited = run(environment, &[]);
            assert_eq!(inherited, run(BTreeMap::new(), &arguments));
            assert_eq!(inherited, expected);
        }
    }

    #[test]
    fn pre_file_fragment_guards_helpers_before_emitting_metadata() {
        let fragment = pre_file_fragment();
        let namespace = fragment
            .find("$(info AROS_MAKE_INITIAL_NAMESPACE=")
            .expect("namespace metadata line");
        let values = fragment
            .find("$(info AROS_MAKE_INITIAL_VALUES=")
            .expect("initial values metadata line");

        for guard in [
            "$(origin __AROS_NAMESPACE_ITEM)",
            "$(origin __AROS_INITIAL_VALUES_ITEM)",
            "$(origin __AROS_INITIAL_VALUES_NL)",
        ] {
            assert!(fragment.find(guard).expect("helper guard") < namespace);
        }
        assert!(namespace < values);
        assert!(fragment.contains(&initial_values_fragment()));
        assert!(fragment.ends_with(&super::parser_controls_fragment()));
    }

    #[test]
    fn initial_values_fragment_uses_raw_values_and_excludes_observer_helpers() {
        let fragment = initial_values_fragment();

        assert!(fragment.contains("$(value $(__AROS_INITIAL_VALUES_ITEM))"));
        assert!(fragment.contains("$(filter file default,$(origin $(__AROS_INITIAL_VALUES_ITEM)))"));
        assert!(fragment
            .contains("$(filter-out .% __AROS_INITIAL_VALUES_ITEM __AROS_INITIAL_VALUES_NL,"));
        assert!(fragment.contains("undefine __AROS_INITIAL_VALUES_NL\n"));
    }

    #[test]
    fn command_line_capture_is_opt_in_bounded_and_guards_before_raw_output() {
        use super::{command_line_values_fragment, MAKE_COMMAND_LINE_VALUES_METADATA_KEY};
        use std::collections::BTreeSet;
        let expected = BTreeSet::from(["A".into(), "1CLI-B".into()]);
        let fragment = command_line_values_fragment(&expected).unwrap();
        let output = fragment
            .find(&format!("$(info {MAKE_COMMAND_LINE_VALUES_METADATA_KEY}="))
            .unwrap();
        for guard in [
            "iterator collision",
            "newline collision",
            "Unexpected command-line",
            "Missing command-line",
        ] {
            assert!(fragment.find(guard).unwrap() < output);
        }
        assert!(fragment.contains("$(value $(__AROS_COMMAND_VALUES_ITEM))"));
        assert!(fragment.ends_with("undefine __AROS_COMMAND_VALUES_NL\n"));
        assert!(!pre_file_fragment().contains(MAKE_COMMAND_LINE_VALUES_METADATA_KEY));
        for name in [
            ".HIDDEN",
            "A%",
            "A,B",
            "A B",
            "$(effect)",
            "__AROS_COMMAND_VALUES_ITEM",
        ] {
            assert!(command_line_values_fragment(&BTreeSet::from([name.into()])).is_err());
        }
        let oversized = (0..4097).map(|index| format!("NAME{index}")).collect();
        assert!(command_line_values_fragment(&oversized).is_err());
    }
}
