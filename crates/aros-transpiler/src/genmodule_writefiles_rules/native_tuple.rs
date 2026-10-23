use crate::make_shell_data::LiteralRecipeCommand;
use aros_common::mesa_genmodule_inventory::MesaGenmoduleProducer;
use std::path::{Component, Path, PathBuf};

/// Check that a literal command's arguments match one inventory producer's
/// closed native `GENMODULE writefiles` tuple.
///
/// The first argument is intentionally opaque. The caller must independently
/// bind it to the exact queried host `GENMODULE` program. This checks tuple
/// consistency only; it establishes no source provenance, environment,
/// program identity, scheduling, or authority to execute a recipe.
///
/// Returns the argv word indices for the base and optional override
/// configuration paths, for use by existing artifact-binding requests.
///
/// # Errors
/// Returns a fixed reason when paths or command arguments do not match the
/// closed tuple. No command is executed and no recipe authority is granted.
pub fn validate_native_writefiles_tuple(
    command: &LiteralRecipeCommand,
    producer: &MesaGenmoduleProducer,
    source_root: &Path,
    nominal_destination: &Path,
) -> Result<Vec<usize>, String> {
    if !command.redirections().is_empty() {
        return Err("native GENMODULE writefiles command has redirections".into());
    }

    let source_root = absolute_normalized_path(source_root, "source root")?;
    let source_root_path = PathBuf::from(source_root.as_str());
    let nominal_destination = absolute_normalized_path(nominal_destination, "nominal destination")?;
    let base_configuration =
        safe_source_relative_path(&producer.base_configuration, "base configuration")?;
    let override_configuration = producer
        .override_configuration
        .as_deref()
        .map(|configuration| safe_source_relative_path(configuration, "override configuration"))
        .transpose()?;

    let words = command.words();
    let expected_word_count = if override_configuration.is_some() {
        10
    } else {
        8
    };
    if words.len() != expected_word_count {
        return Err("native GENMODULE writefiles argv has an unexpected word count".into());
    }
    if words.get(1).map(String::as_str) != Some("-c") {
        return Err("native GENMODULE writefiles base configuration option mismatch".into());
    }

    let base_path = source_root_path.join(base_configuration);
    if words.get(2).map(String::as_str)
        != Some(path_argument(&base_path, "base configuration")?.as_str())
    {
        return Err("native GENMODULE writefiles base configuration path mismatch".into());
    }

    let destination_option_index = if let Some(override_configuration) = override_configuration {
        if words.get(3).map(String::as_str) != Some("-o") {
            return Err("native GENMODULE writefiles override option mismatch".into());
        }
        let override_path = source_root_path.join(override_configuration);
        if words.get(4).map(String::as_str)
            != Some(path_argument(&override_path, "override configuration")?.as_str())
        {
            return Err("native GENMODULE writefiles override configuration path mismatch".into());
        }
        5
    } else {
        3
    };

    if words.get(destination_option_index).map(String::as_str) != Some("-d") {
        return Err("native GENMODULE writefiles destination option mismatch".into());
    }
    if words.get(destination_option_index + 1).map(String::as_str)
        != Some(nominal_destination.as_str())
    {
        return Err("native GENMODULE writefiles destination path mismatch".into());
    }

    let operation_index = destination_option_index + 2;
    if words.get(operation_index).map(String::as_str) != Some("writefiles") {
        return Err("native GENMODULE operation is not writefiles".into());
    }
    if words.get(operation_index + 1).map(String::as_str) != Some(producer.module.as_str()) {
        return Err("native GENMODULE module does not match the producer".into());
    }
    if words.get(operation_index + 2).map(String::as_str) != Some(producer.module_type.as_str()) {
        return Err("native GENMODULE module type does not match the producer".into());
    }

    Ok(if override_configuration.is_some() {
        vec![2, 4]
    } else {
        vec![2]
    })
}

fn absolute_normalized_path(path: &Path, what: &str) -> Result<String, String> {
    let raw = path
        .to_str()
        .filter(|value| !value.contains('\0'))
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
    Ok(raw.to_owned())
}

fn safe_source_relative_path<'a>(value: &'a str, what: &str) -> Result<&'a Path, String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', '\0'])
        || value.chars().any(char::is_control)
        || path.is_absolute()
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "native GENMODULE {what} must be a safe source-relative path"
        ));
    }
    Ok(path)
}

fn path_argument(path: &Path, what: &str) -> Result<String, String> {
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .map(str::to_owned)
        .ok_or_else(|| format!("native GENMODULE {what} path is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::validate_native_writefiles_tuple;
    use crate::make_shell_data::{
        LiteralRecipeCommand, LiteralRecipeStatement, parse_literal_recipe,
    };
    use aros_common::mesa_genmodule_inventory::MesaGenmoduleProducer;
    use std::path::{Path, PathBuf};

    const SOURCE_ROOT: &str = "/checkout source";
    const DESTINATION: &str = "/generated output/mesa";

    fn producer(override_configuration: Option<&str>) -> MesaGenmoduleProducer {
        MesaGenmoduleProducer {
            owner: "mesa-driver".into(),
            declaration_directory: "src/mesa/driver".into(),
            module: "mesa_driver".into(),
            module_type: "library".into(),
            base_configuration: "src/mesa/driver/module.conf".into(),
            override_configuration: override_configuration.map(str::to_owned),
        }
    }

    fn words_for(producer: &MesaGenmoduleProducer) -> Vec<String> {
        let source_root = Path::new(SOURCE_ROOT);
        let mut words = vec![
            "/unbound host GENMODULE".to_owned(),
            "-c".to_owned(),
            source_root
                .join(&producer.base_configuration)
                .to_str()
                .unwrap()
                .to_owned(),
        ];
        if let Some(configuration) = &producer.override_configuration {
            words.push("-o".to_owned());
            words.push(source_root.join(configuration).to_str().unwrap().to_owned());
        }
        words.extend([
            "-d".to_owned(),
            DESTINATION.to_owned(),
            "writefiles".to_owned(),
            producer.module.clone(),
            producer.module_type.clone(),
        ]);
        words
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
        let recipe = format!("{recipe}{suffix}");
        let mut statements = parse_literal_recipe(&recipe).unwrap();
        assert_eq!(statements.len(), 1, "expected one literal recipe command");
        match statements.pop().unwrap() {
            LiteralRecipeStatement::Command(command) => command,
            _ => panic!("expected one simple command"),
        }
    }

    fn rejects_words(words: &[String], producer: &MesaGenmoduleProducer, expected_reason: &str) {
        let command = parse_words(words, "");
        let error = validate_native_writefiles_tuple(
            &command,
            producer,
            Path::new(SOURCE_ROOT),
            Path::new(DESTINATION),
        )
        .unwrap_err();
        assert!(
            error.contains(expected_reason),
            "expected reason `{expected_reason}`, received `{error}`"
        );
    }

    #[test]
    fn accepts_base_configuration_and_preserves_quoted_argv_words() {
        let producer = producer(None);
        let words = words_for(&producer);
        let command = parse_words(&words, "");

        assert_eq!(command.words(), words.as_slice());
        assert_eq!(command.words()[0], "/unbound host GENMODULE");
        assert_eq!(
            validate_native_writefiles_tuple(
                &command,
                &producer,
                Path::new(SOURCE_ROOT),
                Path::new(DESTINATION),
            )
            .unwrap(),
            vec![2]
        );
    }

    #[test]
    fn accepts_base_and_override_configurations() {
        let producer = producer(Some("src/mesa/driver/override.conf"));
        let command = parse_words(&words_for(&producer), "");

        assert_eq!(
            validate_native_writefiles_tuple(
                &command,
                &producer,
                Path::new(SOURCE_ROOT),
                Path::new(DESTINATION),
            )
            .unwrap(),
            vec![2, 4]
        );
    }

    #[test]
    fn rejects_operation_module_type_config_override_and_destination_mismatches() {
        let producer = producer(Some("src/mesa/driver/override.conf"));
        let valid_words = words_for(&producer);
        let cases = [
            (7, "readfiles", "operation is not writefiles"),
            (8, "other_module", "module does not match"),
            (9, "resource", "module type does not match"),
            (
                2,
                "/checkout source/other/module.conf",
                "base configuration path",
            ),
            (
                4,
                "/checkout source/other/override.conf",
                "override configuration path",
            ),
            (6, "/generated output/other", "destination path mismatch"),
        ];

        for (index, replacement, reason) in cases {
            let mut words = valid_words.clone();
            words[index] = replacement.to_owned();
            rejects_words(&words, &producer, reason);
        }
    }

    #[test]
    fn rejects_missing_arguments_and_wrong_options() {
        let producer = producer(Some("src/mesa/driver/override.conf"));
        let valid_words = words_for(&producer);
        let cases = [
            (1, "--config", "base configuration option"),
            (3, "--override", "override option"),
            (5, "--destination", "destination option"),
        ];

        for (index, replacement, reason) in cases {
            let mut words = valid_words.clone();
            words[index] = replacement.to_owned();
            rejects_words(&words, &producer, reason);
        }

        let mut missing_argument = valid_words;
        missing_argument.pop();
        rejects_words(&missing_argument, &producer, "unexpected word count");
    }

    #[test]
    fn rejects_extra_argv_and_redirections() {
        let producer = producer(None);
        let mut words = words_for(&producer);
        words.push("extra".to_owned());
        rejects_words(&words, &producer, "unexpected word count");

        let words = words_for(&producer);
        let command = parse_words(&words, " 2>captured-error");
        let error = validate_native_writefiles_tuple(
            &command,
            &producer,
            Path::new(SOURCE_ROOT),
            Path::new(DESTINATION),
        )
        .unwrap_err();
        assert!(error.contains("redirections"));
    }

    #[test]
    fn rejects_traversal_in_source_relative_configuration_paths() {
        let mut producer = producer(None);
        producer.base_configuration = "../outside/module.conf".into();
        let command = parse_words(&words_for(&producer), "");
        let error = validate_native_writefiles_tuple(
            &command,
            &producer,
            Path::new(SOURCE_ROOT),
            Path::new(DESTINATION),
        )
        .unwrap_err();
        assert!(error.contains("safe source-relative path"));

        producer.base_configuration = "src/mesa/driver/module.conf".into();
        producer.override_configuration = Some("src/mesa/../override.conf".into());
        let command = parse_words(&words_for(&producer), "");
        let error = validate_native_writefiles_tuple(
            &command,
            &producer,
            Path::new(SOURCE_ROOT),
            Path::new(DESTINATION),
        )
        .unwrap_err();
        assert!(error.contains("override configuration must be a safe source-relative path"));
    }

    #[test]
    fn rejects_unnormalized_or_relative_root_and_destination_paths() {
        let producer = producer(None);
        let command = parse_words(&words_for(&producer), "");
        for (source_root, destination) in [
            (PathBuf::from("relative/root"), PathBuf::from(DESTINATION)),
            (
                PathBuf::from(SOURCE_ROOT),
                PathBuf::from("/generated/../output"),
            ),
        ] {
            assert!(
                validate_native_writefiles_tuple(&command, &producer, &source_root, &destination,)
                    .is_err()
            );
        }
    }
}
