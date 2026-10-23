//! Exact one-word MetaMake launcher instrumentation; inert derivation only.

use super::{physical_line, ConfiguredProject};
use aros_common::{make_child_command::LiteralMakeChildCommand, sha256_bytes, Sha256Digest};
use std::path::Path;

/// Checked configuration derivation, not authority to replace or execute a file.
///
/// The selected command prefix is replaced once in one explicit project's
/// source directive. All other bytes, including unresolved Source variables,
/// are preserved. The live owner must bind these original bytes and the actual
/// observer executable/control before writing or launching anything.
pub struct MetaMakeLauncherConfiguration {
    original_sha256: Sha256Digest,
    derived_sha256: Sha256Digest,
    bytes: Vec<u8>,
}

impl MetaMakeLauncherConfiguration {
    /// Instrument the sole explicit project and fail closed for the implicit
    /// default project. MetaMake processes the initial `maketool false` in its
    /// default-project context before constructing explicit projects. The sole
    /// selected project then receives its explicit instrumented command.
    ///
    /// This deliberately changes the unselected default project's behavior;
    /// POSIX shell `false` cannot launch the uninstrumented default GNU Make.
    /// This is not a proof of Root project/owner selection or a shell sandbox.
    ///
    /// # Errors
    /// Refuses multiple projects and all ordinary instrumentation failures.
    pub fn derive_confined_project(
        original: &[u8],
        expected_sha256: &Sha256Digest,
        project: &str,
        source_word: &str,
        launcher: &str,
    ) -> Result<Self, String> {
        let text = std::str::from_utf8(original)
            .map_err(|_| "MetaMake original configuration is not UTF-8")?;
        ConfiguredProject::parse_only_project(text, project)?;
        let mut result = Self::derive(original, expected_sha256, project, source_word, launcher)?;
        let mut bytes = b"maketool false\n".to_vec();
        bytes.extend_from_slice(&result.bytes);
        // Refuse rather than exceed the same configured-project byte budget.
        if bytes.len() > super::MAX_CONFIG_BYTES {
            return Err("MetaMake confined configuration exceeds byte limit".into());
        }
        result.derived_sha256 = sha256_bytes(&bytes);
        result.bytes = bytes;
        Ok(result)
    }

    /// Derive observer instrumentation without a second project interpreter.
    ///
    /// Uses the existing configured-project parser and its reference line
    /// bounds. The replacement must be an absolute literal command without
    /// environment assignments; neither its executable nor its arguments are
    /// authenticated by this data operation.
    ///
    /// # Errors
    /// Refuses changed original bytes, ambiguous/missing directives, a changed
    /// leading Source word, unsafe replacement syntax, or a reference buffer
    /// overflow. No file or process is touched.
    pub fn derive(
        original: &[u8],
        expected_sha256: &Sha256Digest,
        project: &str,
        source_word: &str,
        launcher: &str,
    ) -> Result<Self, String> {
        if sha256_bytes(original) != *expected_sha256 {
            return Err("MetaMake launcher original configuration changed".into());
        }
        if source_word.is_empty() || source_word.chars().any(char::is_whitespace) {
            return Err("MetaMake launcher Source word is not a single token".into());
        }
        let command = LiteralMakeChildCommand::parse(launcher)
            .map_err(|_| "MetaMake replacement launcher is not literal")?;
        if !Path::new(command.program()).is_absolute()
            || !command.environment_assignments().is_empty()
        {
            return Err("MetaMake replacement launcher lacks an explicit program".into());
        }
        let text = std::str::from_utf8(original)
            .map_err(|_| "MetaMake original configuration is not UTF-8")?;
        let selected = ConfiguredProject::parse(text, project)?;
        let template = selected
            .make_command
            .as_deref()
            .ok_or("MetaMake project has no selected command")?;
        let tail = template
            .strip_prefix(source_word)
            .filter(|tail| tail.starts_with(' '))
            .ok_or("MetaMake leading Source command word changed")?;
        if template.matches(source_word).count() != 1 {
            return Err("MetaMake Source command word is ambiguous".into());
        }
        let replacement = format!("{launcher}{tail}");
        let mut section = None;
        let mut replacements = 0;
        let mut derived = String::new();
        for (index, physical) in text.split_inclusive('\n').enumerate() {
            let line = physical_line(physical, "launcher configuration", index + 1)?;
            if line.starts_with('[') {
                section = line
                    .strip_prefix('[')
                    .and_then(|value| value.strip_suffix(']'));
            }
            let directive = line.trim_start_matches(|value: char| value.is_ascii_whitespace());
            let end = directive
                .find(|value: char| value.is_ascii_whitespace())
                .unwrap_or(directive.len());
            if section == Some(project) && directive[..end].eq_ignore_ascii_case("maketool") {
                let value =
                    directive[end..].trim_start_matches(|value: char| value.is_ascii_whitespace());
                if value != template {
                    return Err("MetaMake selected command directive is ambiguous".into());
                }
                replacements += 1;
                let prefix_length = line.len() - value.len();
                derived.push_str(&line[..prefix_length]);
                derived.push_str(&replacement);
                derived.push('\n');
            } else {
                derived.push_str(physical);
            }
        }
        if replacements != 1 {
            return Err("MetaMake launcher requires exactly one explicit command directive".into());
        }
        let actual = ConfiguredProject::parse(&derived, project)?;
        let mut expected = selected;
        expected.make_command = Some(replacement);
        if actual != expected {
            return Err("MetaMake launcher changed other project semantics".into());
        }
        let bytes = derived.into_bytes();
        Ok(Self {
            original_sha256: expected_sha256.clone(),
            derived_sha256: sha256_bytes(&bytes),
            bytes,
        })
    }

    /// Exact original selected bytes, not a self-declared runtime identity.
    #[must_use]
    pub const fn original_sha256(&self) -> &Sha256Digest {
        &self.original_sha256
    }

    /// Exact derived configuration digest for the live owner's later write CAS.
    #[must_use]
    pub const fn derived_sha256(&self) -> &Sha256Digest {
        &self.derived_sha256
    }

    /// Inert configuration bytes; obtaining them grants no write/launch right.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
#[path = "metamake_launcher_tests.rs"]
mod tests;
