//! Data-only binding for one bounded, caller-supplied GNU Make scope.
//!
//! This module validates retained stdout against an independently supplied
//! context. It does not authenticate the producer and does not grant authority
//! to choose an owner, working directory, inputs, or execution behavior.

use std::collections::BTreeMap;
use std::path::Path;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::digest::sha256_bytes;
use crate::make_initial_values::ValidatedMakeInitialValues;
use crate::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};
use crate::{ArosError, Result, Sha256Digest};

const MAX_OWNER_BYTES: usize = 255;
const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
const MAX_INPUTS: usize = 64;
const MAX_INPUT_NAME_BYTES: usize = 128;
const MAX_INPUT_VALUE_BYTES: usize = 4096;
const MAX_INPUTS_TOTAL_BYTES: usize = 64 * 1024;
const MAX_MAKE_STDOUT_BYTES: usize = 256 * 1024;

const SCOPE_CONTEXT_METADATA_KEY: &str = "AROS_MAKE_SCOPE_CONTEXT_SHA256";
const SCOPE_INPUTS_METADATA_KEY: &str = "AROS_MAKE_SCOPE_INPUTS";
const CONFIGURATION_FILE: &str = "Make scope snapshot";

/// Caller-supplied identity and literal command-line inputs for a Make scope.
///
/// The identity hash serializes fields in declaration order. `inputs` uses a
/// `BTreeMap`, so its keys serialize in lexical order and yield deterministic
/// bytes. This binds data to a description; it is not execution authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeScopeContext {
    pub owner: String,
    pub source_inputs_sha256: Sha256Digest,
    pub compiler_tree_sha256: Sha256Digest,
    pub configuration_stdout_sha256: Sha256Digest,
    pub working_directory: String,
    #[serde(deserialize_with = "deserialize_inputs")]
    pub inputs: BTreeMap<String, String>,
}

impl MakeScopeContext {
    /// Check the bounded, data-only scope description.
    ///
    /// # Errors
    /// Returns an error when the owner, working directory, or input map does
    /// not meet the documented bounds and syntax.
    pub fn validate(&self) -> Result<()> {
        if self.owner.is_empty()
            || self.owner.len() > MAX_OWNER_BYTES
            || self.owner == "."
            || self.owner == ".."
            || !self.owner.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
            })
        {
            return Err(scope_error("owner is invalid or exceeds its byte limit"));
        }

        if !is_normalized_absolute_path(&self.working_directory) {
            return Err(scope_error(
                "working directory is not a normalized absolute path",
            ));
        }

        if self.inputs.is_empty() || self.inputs.len() > MAX_INPUTS {
            return Err(scope_error("input count is outside its allowed range"));
        }

        let mut total_bytes = 0usize;
        for (name, value) in &self.inputs {
            if !is_valid_input_name(name) {
                return Err(scope_error(
                    "an input name is invalid or exceeds its byte limit",
                ));
            }
            if value.len() > MAX_INPUT_VALUE_BYTES
                || value.chars().any(char::is_control)
                || value.contains('$')
            {
                return Err(scope_error(
                    "an input value is invalid or exceeds its byte limit",
                ));
            }
            total_bytes = total_bytes
                .saturating_add(name.len())
                .saturating_add(value.len());
            if total_bytes > MAX_INPUTS_TOTAL_BYTES {
                return Err(scope_error("combined input data exceeds its byte limit"));
            }
        }

        Ok(())
    }

    /// Hash the deterministic JSON representation after validating the data.
    ///
    /// The resulting digest binds a snapshot to this description. It does not
    /// authenticate a Make process or authorize execution in this context.
    ///
    /// # Errors
    /// Returns an error when this context is invalid or cannot be serialized.
    pub fn identity_sha256(&self) -> Result<Sha256Digest> {
        self.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|_| scope_error("could not serialize the scope identity"))?;
        Ok(sha256_bytes(&encoded))
    }
}

/// A validated pairing of one caller context and its retained Make data.
///
/// The fields remain private so consumers receive only data that passed the
/// context, namespace, initial-values, and command-line-input checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMakeScopeSnapshot {
    context: MakeScopeContext,
    namespace: ValidatedMakeNamespace,
    values: ValidatedMakeInitialValues,
}

impl ValidatedMakeScopeSnapshot {
    /// Parse and validate scope metadata from retained Make stdout.
    ///
    /// `expected` is supplied independently by the caller; stdout cannot
    /// choose or replace it. The result is a data consistency check, not proof
    /// of producer identity, cwd, ownership, or when the snapshot was taken.
    /// The stdout must contain exactly one line for each of the four scope and
    /// initial-observation metadata keys, with no other output lines.
    ///
    /// # Errors
    /// Returns an error for oversized or invalid UTF-8 stdout, an invalid
    /// expected context, missing, duplicate, or unexpected metadata lines, or
    /// any mismatch or malformed observation.
    pub fn from_make_stdout(bytes: &[u8], expected: &MakeScopeContext) -> Result<Self> {
        if bytes.len() > MAX_MAKE_STDOUT_BYTES {
            return Err(scope_error("Make stdout exceeds its byte limit"));
        }
        expected.validate()?;

        let text =
            std::str::from_utf8(bytes).map_err(|_| scope_error("Make stdout is not UTF-8"))?;
        validate_metadata_lines(text)?;
        let context_digest = unique_metadata_value(text, SCOPE_CONTEXT_METADATA_KEY)?;
        let supplied_digest = Sha256Digest::parse(context_digest)
            .map_err(|_| scope_error("scope context digest is malformed"))?;
        if supplied_digest != expected.identity_sha256()? {
            return Err(scope_error(
                "scope context digest does not match the expected context",
            ));
        }

        let inputs_measurement = unique_metadata_value(text, SCOPE_INPUTS_METADATA_KEY)?;
        let measured_inputs = parse_scope_inputs(inputs_measurement)?;
        if measured_inputs != expected.inputs {
            return Err(scope_error(
                "scope inputs do not match the expected context",
            ));
        }

        let namespace = ValidatedMakeNamespace::from_make_stdout(bytes)?;
        if namespace.to_observation().entries.iter().any(|entry| {
            entry.origin == MakeVariableOrigin::CommandLine
                && !expected.inputs.contains_key(&entry.name)
        }) {
            return Err(scope_error(
                "an unbound command-line variable is present in the scope namespace",
            ));
        }
        for name in expected.inputs.keys() {
            if namespace.origin(name) != Some(MakeVariableOrigin::CommandLine) {
                return Err(scope_error(
                    "an expected input is not reported as a command-line variable",
                ));
            }
        }
        let values = ValidatedMakeInitialValues::from_make_stdout(bytes, &namespace)?;

        Ok(Self {
            context: expected.clone(),
            namespace,
            values,
        })
    }

    /// Return the independently supplied scope context.
    #[must_use]
    pub const fn context(&self) -> &MakeScopeContext {
        &self.context
    }

    /// Return the validated Make variable namespace.
    #[must_use]
    pub const fn namespace(&self) -> &ValidatedMakeNamespace {
        &self.namespace
    }

    /// Return the validated ordinary file/default initial values.
    #[must_use]
    pub const fn values(&self) -> &ValidatedMakeInitialValues {
        &self.values
    }
}

fn deserialize_inputs<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct InputsVisitor;

    impl<'de> Visitor<'de> for InputsVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a map of unique Make input names to literal values")
        }

        fn visit_map<A>(self, mut access: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut inputs = BTreeMap::new();
            while let Some((name, value)) = access.next_entry::<String, String>()? {
                if inputs.contains_key(&name) {
                    return Err(de::Error::custom("duplicate Make input name"));
                }
                if inputs.len() == MAX_INPUTS {
                    return Err(de::Error::custom("too many Make inputs"));
                }
                inputs.insert(name, value);
            }
            Ok(inputs)
        }
    }

    deserializer.deserialize_map(InputsVisitor)
}

fn is_normalized_absolute_path(path: &str) -> bool {
    if path.is_empty()
        || path.len() > MAX_WORKING_DIRECTORY_BYTES
        || !Path::new(path).is_absolute()
        || path.chars().any(char::is_control)
    {
        return false;
    }
    if path == "/" {
        return true;
    }
    if !path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    path[1..]
        .split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_valid_input_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_INPUT_NAME_BYTES {
        return false;
    }
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn unique_metadata_value<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    let mut selected = None;
    for line in text.lines() {
        if let Some((line_key, value)) = line.split_once('=') {
            if line_key == key && selected.replace(value).is_some() {
                return Err(scope_error("duplicate scope metadata"));
            }
        }
    }
    selected.ok_or_else(|| scope_error("required scope metadata is missing"))
}

fn validate_metadata_lines(text: &str) -> Result<()> {
    const KEYS: [&str; 4] = [
        "AROS_MAKE_INITIAL_NAMESPACE",
        "AROS_MAKE_INITIAL_VALUES",
        SCOPE_CONTEXT_METADATA_KEY,
        SCOPE_INPUTS_METADATA_KEY,
    ];

    let mut seen = [false; KEYS.len()];
    for line in text.lines() {
        let Some((key, _)) = line.split_once('=') else {
            return Err(scope_error(
                "Make stdout contains an unexpected output line",
            ));
        };
        let Some(index) = KEYS.iter().position(|expected| *expected == key) else {
            return Err(scope_error(
                "Make stdout contains an unexpected metadata line",
            ));
        };
        if seen[index] {
            return Err(scope_error("Make stdout contains duplicate metadata"));
        }
        seen[index] = true;
    }
    if seen.iter().any(|present| !present) {
        return Err(scope_error("Make stdout is missing required metadata"));
    }
    Ok(())
}

fn parse_scope_inputs(measurement: &str) -> Result<BTreeMap<String, String>> {
    if measurement.chars().any(char::is_control) {
        return Err(scope_error(
            "scope input metadata contains a control character",
        ));
    }

    let mut records = measurement.split(';');
    if records.next() != Some("v1") {
        return Err(scope_error("scope input metadata has invalid framing"));
    }

    let mut inputs = BTreeMap::new();
    loop {
        let record = records
            .next()
            .ok_or_else(|| scope_error("scope input metadata is missing its end marker"))?;
        if record == "end" {
            if records.next().is_some() {
                return Err(scope_error(
                    "scope input metadata has data after its end marker",
                ));
            }
            break;
        }
        if inputs.len() == MAX_INPUTS {
            return Err(scope_error("scope input metadata exceeds its entry limit"));
        }

        let record = record.strip_prefix(' ').unwrap_or(record);
        let mut fields = record.split('|');
        let [Some(name), Some(origin), Some(flavor), Some(encoded_value)] =
            [fields.next(), fields.next(), fields.next(), fields.next()]
        else {
            return Err(scope_error(
                "scope input metadata is missing a record field",
            ));
        };
        if fields.next().is_some()
            || !is_valid_input_name(name)
            || origin != "command line"
            || flavor != "recursive"
        {
            return Err(scope_error(
                "scope input metadata contains an invalid record",
            ));
        }

        let value = decode_scope_input(encoded_value)?;
        if value.contains('$') {
            return Err(scope_error("scope input metadata contains a refused value"));
        }
        if inputs.insert(name.to_owned(), value).is_some() {
            return Err(scope_error(
                "scope input metadata contains a duplicate name",
            ));
        }
    }

    if inputs.is_empty() {
        return Err(scope_error("scope input metadata contains no inputs"));
    }
    Ok(inputs)
}

fn decode_scope_input(encoded: &str) -> Result<String> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0usize;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let escape = bytes
                .get(offset..offset.saturating_add(3))
                .ok_or_else(|| scope_error("scope input has an incomplete escape"))?;
            decoded.push(match escape {
                b"%25" => b'%',
                b"%7C" => b'|',
                b"%3B" => b';',
                _ => return Err(scope_error("scope input has an invalid escape")),
            });
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
        if decoded.len() > MAX_INPUT_VALUE_BYTES {
            return Err(scope_error("scope input value exceeds its byte limit"));
        }
    }

    let value =
        String::from_utf8(decoded).map_err(|_| scope_error("scope input value is not UTF-8"))?;
    if value.chars().any(char::is_control) {
        return Err(scope_error(
            "scope input value contains a control character",
        ));
    }
    Ok(value)
}

fn scope_error(message: &str) -> ArosError {
    ArosError::Configuration {
        file: CONFIGURATION_FILE.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(owner: &str, cwd: &str, name: &str, value: &str) -> MakeScopeContext {
        MakeScopeContext {
            owner: owner.to_owned(),
            source_inputs_sha256: sha256_bytes(b"source inputs"),
            compiler_tree_sha256: sha256_bytes(b"compiler tree"),
            configuration_stdout_sha256: sha256_bytes(b"configuration stdout"),
            working_directory: cwd.to_owned(),
            inputs: BTreeMap::from([(name.to_owned(), value.to_owned())]),
        }
    }

    fn encode(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.bytes() {
            match byte {
                b'%' => encoded.push_str("%25"),
                b'|' => encoded.push_str("%7C"),
                b';' => encoded.push_str("%3B"),
                _ => encoded.push(char::from(byte)),
            }
        }
        encoded
    }

    fn stdout(context: &MakeScopeContext, version_origin: &str) -> String {
        let context_digest = context.identity_sha256().unwrap();
        let (input_name, input_value) = context.inputs.iter().next().unwrap();
        let namespace = format!(
            "v1:3;.VARIABLES|default;MAKE_VERSION|{version_origin};{input_name}|command line;end"
        );
        let flavor = if version_origin == "file" {
            "simple"
        } else {
            "recursive"
        };
        let values = format!("v1;MAKE_VERSION|{version_origin}|{flavor}|4.4.1;end");
        let inputs = format!(
            "v1;{input_name}|command line|recursive|{};end",
            encode(input_value)
        );
        format!(
            "AROS_MAKE_SCOPE_CONTEXT_SHA256={context_digest}\nAROS_MAKE_SCOPE_INPUTS={inputs}\nAROS_MAKE_INITIAL_NAMESPACE={namespace}\nAROS_MAKE_INITIAL_VALUES={values}\n"
        )
    }

    fn validate_output(
        output: &str,
        expected: &MakeScopeContext,
    ) -> Result<ValidatedMakeScopeSnapshot> {
        ValidatedMakeScopeSnapshot::from_make_stdout(output.as_bytes(), expected)
    }

    #[test]
    fn accepts_arbitrary_selector_and_file_or_default_make_version() {
        let expected = context(
            "build.owner",
            "/work/aros",
            "CUSTOM_SELECTOR",
            "literal %|; input",
        );
        for origin in ["file", "default"] {
            let snapshot = validate_output(&stdout(&expected, origin), &expected).unwrap();
            assert_eq!(snapshot.context(), &expected);
            assert_eq!(
                snapshot.namespace().origin("CUSTOM_SELECTOR"),
                Some(MakeVariableOrigin::CommandLine)
            );
            assert_eq!(
                snapshot.values().entries().next().unwrap().name,
                "MAKE_VERSION"
            );
        }
    }

    #[test]
    fn expected_context_changes_are_rejected() {
        let expected = context("owner-a", "/work/aros", "SELECTOR", "original");
        let output = stdout(&expected, "file");

        let owner_swap = context("owner-b", "/work/aros", "SELECTOR", "original");
        let changed_input = context("owner-a", "/work/aros", "SELECTOR", "changed");
        let changed_cwd = context("owner-a", "/work/other", "SELECTOR", "original");
        let mut changed_expected_digest = expected.clone();
        changed_expected_digest.source_inputs_sha256 = sha256_bytes(b"different source inputs");
        assert!(validate_output(&output, &owner_swap).is_err());
        assert!(validate_output(&output, &changed_input).is_err());
        assert!(validate_output(&output, &changed_cwd).is_err());
        assert!(validate_output(&output, &changed_expected_digest).is_err());

        let changed_digest = output.replace(
            &expected.identity_sha256().unwrap().to_string(),
            &sha256_bytes(b"other context").to_string(),
        );
        assert!(validate_output(&changed_digest, &expected).is_err());
    }

    #[test]
    fn scope_metadata_requires_one_exact_record() {
        let expected = context("owner", "/work/aros", "SELECTOR", "safe");
        let good = stdout(&expected, "file");
        assert!(validate_output(
            &good.replace("AROS_MAKE_SCOPE_INPUTS=", "OTHER="),
            &expected
        )
        .is_err());
        assert!(validate_output(
            &format!(
                "{good}AROS_MAKE_SCOPE_CONTEXT_SHA256={}\n",
                expected.identity_sha256().unwrap()
            ),
            &expected
        )
        .is_err());
        assert!(validate_output(
            &format!("{good}AROS_MAKE_SCOPE_INPUTS=v1;SELECTOR|command line|recursive|safe;end\n"),
            &expected
        )
        .is_err());
        assert!(validate_output(&format!("{good}recipe output\n"), &expected).is_err());
    }

    #[test]
    fn scope_inputs_reject_extra_duplicate_malformed_and_refused_values() {
        let expected = context("owner", "/work/aros", "SELECTOR", "safe");
        let good = stdout(&expected, "file");
        let replace_inputs = |frame: &str| {
            good.lines()
                .map(|line| {
                    if line.starts_with("AROS_MAKE_SCOPE_INPUTS=") {
                        format!("AROS_MAKE_SCOPE_INPUTS={frame}")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        for frame in [
            "v1;SELECTOR|command line|recursive|safe;OTHER|command line|recursive|x;end",
            "v1;SELECTOR|command line|recursive|safe;SELECTOR|command line|recursive|safe;end",
            "v1;SELECTOR|file|recursive|safe;end",
            "v1;SELECTOR|command line|simple|safe;end",
            "v1;SELECTOR|command line|recursive|bad%2Fescape;end",
            "v1;SELECTOR|command line|recursive|bad$value;end",
        ] {
            let output = replace_inputs(frame);
            let error = validate_output(&output, &expected).unwrap_err().to_string();
            assert!(!error.contains("bad$value"));
        }

        let control = replace_inputs("v1;SELECTOR|command line|recursive|bad\tvalue;end");
        assert!(validate_output(&control, &expected).is_err());
    }

    #[test]
    fn empty_input_requires_an_explicit_fourth_field() {
        let expected = context("owner", "/work/aros", "SELECTOR", "");
        let good = stdout(&expected, "file");
        validate_output(&good, &expected).unwrap();
        let missing_field = good.replace(
            "SELECTOR|command line|recursive|;end",
            "SELECTOR|command line|recursive;end",
        );
        assert_ne!(missing_field, good);
        let error = validate_output(&missing_field, &expected).unwrap_err();
        assert!(error.to_string().contains("missing a record field"));
    }

    #[test]
    fn additional_command_line_variables_cannot_escape_the_context() {
        let expected = context("owner", "/work/aros", "SELECTOR", "safe");
        let good = stdout(&expected, "file");
        validate_output(&good, &expected).unwrap();
        let additional = good.replace(
            "v1:3;.VARIABLES|default;",
            "v1:4;.VARIABLES|default;UNBOUND|command line;",
        );
        assert_ne!(additional, good);
        let error = validate_output(&additional, &expected).unwrap_err();
        assert!(error.to_string().contains("unbound command-line variable"));
    }

    #[test]
    fn context_validation_enforces_bounds_and_path_shape() {
        let mut invalid = context("owner", "/work/aros", "SELECTOR", "safe");
        invalid.owner = "..".to_owned();
        assert!(invalid.validate().is_err());

        let invalid = context("owner", "/work/aros/../other", "SELECTOR", "safe");
        assert!(invalid.validate().is_err());
        let invalid = context("owner", "/work//aros", "SELECTOR", "safe");
        assert!(invalid.validate().is_err());

        let too_long = "x".repeat(MAX_INPUT_VALUE_BYTES + 1);
        let invalid = context("owner", "/work/aros", "SELECTOR", &too_long);
        assert!(invalid.validate().is_err());

        let mut invalid = context("owner", "/work/aros", "SELECTOR", "safe");
        invalid.inputs = (0..16)
            .map(|index| (format!("INPUT_{index}"), "x".repeat(MAX_INPUT_VALUE_BYTES)))
            .collect();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn serde_rejects_duplicate_input_keys() {
        let digest = sha256_bytes(b"x");
        let json = format!(
            "{{\"owner\":\"owner\",\"source_inputs_sha256\":\"{digest}\",\"compiler_tree_sha256\":\"{digest}\",\"configuration_stdout_sha256\":\"{digest}\",\"working_directory\":\"/work\",\"inputs\":{{\"A\":\"one\",\"A\":\"two\"}}}}"
        );
        assert!(serde_json::from_str::<MakeScopeContext>(&json).is_err());
    }

    #[test]
    fn stdout_and_scope_data_are_bounded() {
        let expected = context("owner", "/work/aros", "SELECTOR", "safe");
        assert!(ValidatedMakeScopeSnapshot::from_make_stdout(
            &vec![b'x'; MAX_MAKE_STDOUT_BYTES + 1],
            &expected
        )
        .is_err());
    }
}
