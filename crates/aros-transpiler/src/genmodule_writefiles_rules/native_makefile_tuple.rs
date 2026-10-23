use crate::genmodule_header_rules::{safe_basename, GenmoduleType};
use crate::make_shell_data::LiteralRecipeCommand;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Checks the literal arguments for one Source `GENMODULE writemakefile` call.
///
/// The command word is intentionally opaque. The caller must separately bind
/// it to the captured host `GENMODULE` program. This function checks only the
/// bounded argv shape, configuration paths, destination, and Source-derived
/// output name. It does not inspect configuration contents, execute a command,
/// or grant authority to run a recipe.
///
/// Returns the argv indices of the base configuration and optional override
/// configuration, in that order, for the existing artifact binder.
///
/// # Errors
/// Returns an error when the literal command is outside the supported
/// `writemakefile` interface or any path/name differs from Source semantics.
pub fn validate_native_makefile_tuple(
    command: &LiteralRecipeCommand,
    source_root: &Path,
    include_path: &Path,
) -> Result<Vec<usize>, String> {
    if !command.redirections().is_empty() {
        return Err("native GENMODULE writemakefile command has redirections".into());
    }

    let source_root = normalized_absolute_path(source_root, "source root")?;
    let include_path = normalized_absolute_path(include_path, "include path")?;
    let destination = include_path
        .parent()
        .ok_or("native GENMODULE include path has no parent directory")?;

    let words = command.words();
    if words.len() < 4 {
        return Err("native GENMODULE writemakefile argv is incomplete".into());
    }

    // `initconfig()` requires exactly three positional tail words after getopt:
    // operation, module name, and module type. Keeping this tail fixed also
    // leaves the first command word opaque for the caller's program binding.
    let tail_index = words.len() - 3;
    let operation = words[tail_index].as_str();
    let module = words[tail_index + 1].as_str();
    let module_type = words[tail_index + 2].as_str();
    if operation != "writemakefile" {
        return Err("native GENMODULE operation is not writemakefile".into());
    }
    if !safe_basename(module, "") {
        return Err("native GENMODULE module name is not a safe basename".into());
    }
    if GenmoduleType::parse(module_type).is_none() {
        return Err("native GENMODULE module type is not supported by Source".into());
    }

    let mut base_configuration = None;
    let mut override_configuration = None;
    let mut destination_seen = false;
    let mut flavour = None;
    let mut suffix_seen = false;
    let mut cursor = 1;
    while cursor < tail_index {
        let option = words[cursor].as_str();
        let value_index = cursor + 1;
        if value_index >= tail_index {
            return Err(format!(
                "native GENMODULE option `{option}` has no literal argument"
            ));
        }
        let value = words[value_index].as_str();
        if value.is_empty() {
            return Err(format!(
                "native GENMODULE option `{option}` has an empty argument"
            ));
        }

        match option {
            "-c" => {
                if base_configuration.is_some() {
                    return Err("native GENMODULE base configuration option is repeated".into());
                }
                validate_source_configuration(value, &source_root, "base configuration")?;
                base_configuration = Some(value_index);
            }
            "-o" => {
                if override_configuration.is_some() {
                    return Err("native GENMODULE override configuration option is repeated".into());
                }
                validate_source_configuration(value, &source_root, "override configuration")?;
                override_configuration = Some(value_index);
            }
            "-d" => {
                if destination_seen {
                    return Err("native GENMODULE destination option is repeated".into());
                }
                if !destination_argument_matches(value, destination)? {
                    return Err(
                        "native GENMODULE destination is not the include path's parent directory"
                            .into(),
                    );
                }
                destination_seen = true;
            }
            "-f" => {
                if flavour.is_some() {
                    return Err("native GENMODULE flavour option is repeated".into());
                }
                if !safe_basename(value, "") {
                    return Err("native GENMODULE flavour is not a safe basename".into());
                }
                flavour = Some(value);
            }
            "-s" => {
                if suffix_seen {
                    return Err("native GENMODULE suffix option is repeated".into());
                }
                // Source accepts `-s` as a custom generated-file suffix. The
                // `writemakefile()` output path does not use cfg->suffix, so
                // it is deliberately not folded into the predicted basename.
                if value.chars().any(char::is_control) {
                    return Err("native GENMODULE suffix contains a control character".into());
                }
                suffix_seen = true;
            }
            _ => {
                return Err(format!(
                    "native GENMODULE option `{option}` is outside the supported interface"
                ));
            }
        }
        cursor += 2;
    }

    let base_configuration = base_configuration
        .ok_or("native GENMODULE writemakefile command requires `-c` configuration")?;
    if !destination_seen {
        return Err("native GENMODULE writemakefile command requires `-d` destination".into());
    }

    // `writemakefile.c` constructs moduleversname from modulename and the
    // optional `-f` flavour, then writes `${gendir}/Makefile.${moduleversname}${modtypestr}`.
    // `readconfig()` may change cfg->basename and cfg->suffix, but neither is
    // used in that output path. `config.c` preserves the literal modtypestr,
    // including `hook`, even though it maps to the HANDLER enum internally.
    let mut expected_name = format!("Makefile.{module}");
    if let Some(flavour) = flavour {
        expected_name.push('_');
        expected_name.push_str(flavour);
    }
    expected_name.push_str(module_type);
    if include_path.file_name() != Some(OsStr::new(&expected_name)) {
        return Err(format!(
            "native GENMODULE output name does not match Source basename `{expected_name}`"
        ));
    }

    Ok(override_configuration.map_or_else(
        || vec![base_configuration],
        |override_configuration| vec![base_configuration, override_configuration],
    ))
}

fn normalized_absolute_path(path: &Path, what: &str) -> Result<PathBuf, String> {
    path.to_str()
        .filter(|value| !value.contains('\0') && !value.chars().any(char::is_control))
        .ok_or_else(|| {
            format!("native GENMODULE {what} must be an absolute normalized UTF-8 path")
        })?;
    if !path.is_absolute() {
        return Err(format!(
            "native GENMODULE {what} must be an absolute normalized UTF-8 path"
        ));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
            Component::CurDir | Component::ParentDir => {
                return Err(format!(
                    "native GENMODULE {what} must be an absolute normalized UTF-8 path"
                ));
            }
        }
    }
    if normalized.as_os_str() != path.as_os_str() {
        return Err(format!(
            "native GENMODULE {what} must be an absolute normalized UTF-8 path"
        ));
    }
    Ok(normalized)
}

fn validate_source_configuration(
    value: &str,
    source_root: &Path,
    what: &str,
) -> Result<(), String> {
    let path = normalized_absolute_path(Path::new(value), what)?;
    let relative = path
        .strip_prefix(source_root)
        .map_err(|_| format!("native GENMODULE {what} is outside the Source root"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "native GENMODULE {what} must be a file below the Source root"
        ));
    }
    Ok(())
}

fn destination_argument_matches(value: &str, destination: &Path) -> Result<bool, String> {
    // Source initconfig removes exactly one trailing slash from -d. Preserve
    // argv itself; normalize only this comparison, not configuration paths.
    let value = value.strip_suffix('/').unwrap_or(value);
    let path = normalized_absolute_path(Path::new(value), "destination")?;
    Ok(path == destination)
}

#[cfg(test)]
mod tests {
    use super::validate_native_makefile_tuple;
    use crate::make_shell_data::{
        parse_literal_recipe, LiteralRecipeCommand, LiteralRecipeStatement,
    };
    use std::path::{Path, PathBuf};

    const SOURCE_ROOT: &str = "/checkout source";
    const DESTINATION: &str = "/generated output";
    const MODULE: &str = "soundcard";
    const MODULE_TYPE: &str = "library";
    const CONFIG: &str = "/checkout source/modules/soundcard/module.conf";
    const OVERRIDE: &str = "/checkout source/config/board.conf";

    fn include_path(name: &str) -> PathBuf {
        Path::new(DESTINATION).join(name)
    }

    fn words() -> Vec<String> {
        vec![
            "/opaque captured GENMODULE".into(),
            "-c".into(),
            CONFIG.into(),
            "-d".into(),
            DESTINATION.into(),
            "writemakefile".into(),
            MODULE.into(),
            MODULE_TYPE.into(),
        ]
    }

    fn shell_quote(word: &str) -> String {
        format!("'{}'", word.replace('\'', "'\\''"))
    }

    fn parse_words(words: &[String], suffix: &str) -> LiteralRecipeCommand {
        let recipe = words
            .iter()
            .map(|word| shell_quote(word))
            .collect::<Vec<_>>()
            .join(" ");
        let mut statements = parse_literal_recipe(&format!("{recipe}{suffix}")).unwrap();
        assert_eq!(statements.len(), 1, "expected one literal recipe command");
        match statements.pop().unwrap() {
            LiteralRecipeStatement::Command(command) => command,
            _ => panic!("expected one simple command"),
        }
    }

    fn validate(words: &[String], filename: &str, suffix: &str) -> Result<Vec<usize>, String> {
        let command = parse_words(words, suffix);
        validate_native_makefile_tuple(&command, Path::new(SOURCE_ROOT), &include_path(filename))
    }

    #[test]
    fn accepts_base_configuration_and_uses_exact_source_output_name() {
        let words = words();
        assert_eq!(
            validate(&words, "Makefile.soundcardlibrary", "").unwrap(),
            vec![2]
        );
    }

    #[test]
    fn destination_matches_source_single_trailing_slash_handling() {
        let mut words = words();
        words[4] = format!("{DESTINATION}/");
        assert_eq!(
            validate(&words, "Makefile.soundcardlibrary", "").unwrap(),
            vec![2]
        );
        words[4] = format!("{DESTINATION}//");
        assert!(validate(&words, "Makefile.soundcardlibrary", "").is_err());
        words[4] = format!("{DESTINATION}/../other/");
        assert!(validate(&words, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn accepts_base_and_override_configurations_and_returns_both_word_indices() {
        let mut words = words();
        words.splice(3..3, ["-o".into(), OVERRIDE.into()]);

        assert_eq!(
            validate(&words, "Makefile.soundcardlibrary", "").unwrap(),
            vec![2, 4]
        );
    }

    #[test]
    fn accepts_reordered_source_supported_options_without_reordering_argv() {
        let words = vec![
            "/opaque captured GENMODULE".into(),
            "-f".into(),
            "debug".into(),
            "-d".into(),
            DESTINATION.into(),
            "-s".into(),
            "custom".into(),
            "-o".into(),
            OVERRIDE.into(),
            "-c".into(),
            CONFIG.into(),
            "writemakefile".into(),
            MODULE.into(),
            MODULE_TYPE.into(),
        ];
        let command = parse_words(&words, "");

        assert_eq!(command.words(), words.as_slice());
        assert_eq!(
            validate(&words, "Makefile.soundcard_debuglibrary", "").unwrap(),
            vec![10, 8]
        );
    }

    #[test]
    fn flavour_contributes_underscore_and_value_to_output_basename() {
        let mut words = words();
        words.splice(3..3, ["-f".into(), "debug".into()]);

        assert_eq!(
            validate(&words, "Makefile.soundcard_debuglibrary", "").unwrap(),
            vec![2]
        );
        assert!(validate(&words, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn rejects_wrong_operation() {
        let mut words = words();
        words[5] = "writefiles".into();

        assert!(validate(&words, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn rejects_destination_that_is_not_include_parent() {
        let mut words = words();
        words[4] = "/generated output/other".into();

        assert!(validate(&words, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn rejects_output_name_that_differs_from_source_semantics() {
        let words = words();

        assert!(validate(&words, "Makefile.soundcard.device", "").is_err());
    }

    #[test]
    fn rejects_unknown_and_repeated_options() {
        let mut unknown = words();
        unknown.splice(3..3, ["-l".into(), DESTINATION.into()]);
        assert!(validate(&unknown, "Makefile.soundcardlibrary", "").is_err());

        let mut repeated = words();
        repeated.splice(3..3, ["-c".into(), OVERRIDE.into()]);
        assert!(validate(&repeated, "Makefile.soundcardlibrary", "").is_err());

        let mut repeated_destination = words();
        repeated_destination.splice(3..3, ["-d".into(), DESTINATION.into()]);
        assert!(validate(&repeated_destination, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn rejects_configuration_paths_with_traversal_or_outside_source() {
        let mut traversing = words();
        traversing[2] = "/checkout source/../outside/module.conf".into();
        assert!(validate(&traversing, "Makefile.soundcardlibrary", "").is_err());

        let mut outside = words();
        outside[2] = "/another checkout/module.conf".into();
        assert!(validate(&outside, "Makefile.soundcardlibrary", "").is_err());
    }

    #[test]
    fn rejects_recipe_redirections() {
        let words = words();

        assert!(validate(&words, "Makefile.soundcardlibrary", " > /tmp/genmodule.log").is_err());
    }

    #[test]
    fn rejects_unsafe_module_and_unsupported_type_tails() {
        let mut unsafe_module = words();
        unsafe_module[6] = "../soundcard".into();
        assert!(validate(&unsafe_module, "Makefile.soundcardlibrary", "").is_err());

        let mut unsupported_type = words();
        unsupported_type[7] = "unknown".into();
        assert!(validate(&unsupported_type, "Makefile.soundcardlibrary", "").is_err());
    }
}
