//! Bounded observations of GNU Make's initial variable values.
//!
//! The legacy default packet retains only ordinary file/default origins, not
//! environment values or special dot variables. Separate explicitly selected
//! command-line and environment packets share the codec, not that origin policy.
//! This is data consistency, not execution authority.
//! Values belong to their original Make context. In particular, changing an
//! input after a simple value was computed does not recompute that value; an
//! importer must prove the same context or obtain a fresh scoped observation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};

const MAX_MEASUREMENT_BYTES: usize = 128 * 1024;
const MAX_STDOUT_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_RAW_VALUE_BYTES: usize = 64 * 1024;
const MAX_RAW_TOTAL_BYTES: usize = 128 * 1024;
const RESERVED_HELPERS: [&str; 2] = ["__AROS_INITIAL_VALUES_ITEM", "__AROS_INITIAL_VALUES_NL"];
const CONFIGURATION_FILE: &str = "Make initial values";

mod command_line;
mod environment;
mod parser_controls;
mod recipe_controls;
mod recipe_engine;
pub(crate) use command_line::validate_command_line_names;
pub use command_line::{ValidatedMakeCommandLineValues, MAKE_COMMAND_LINE_VALUES_METADATA_KEY};
pub(crate) use environment::validate_environment_names;
pub use environment::{ValidatedMakeEnvironmentValues, MAKE_ENVIRONMENT_VALUES_METADATA_KEY};
pub use parser_controls::{ValidatedMakeParserControls, MAKE_PARSER_CONTROLS_METADATA_KEY};
pub use recipe_controls::{ValidatedMakeRecipeControls, MAKE_RECIPE_CONTROLS_METADATA_KEY};
pub use recipe_engine::{
    ValidatedMakeRecipeEngineValues, MAKE_CLI_PROPAGATION_VARIABLE,
    MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY,
};

/// Metadata key in retained Make stdout.
pub const MAKE_INITIAL_VALUES_METADATA_KEY: &str = "AROS_MAKE_INITIAL_VALUES";

/// Whether GNU Make expands a variable recursively or immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MakeInitialValueFlavor {
    Simple,
    Recursive,
}

/// One initial value together with its observed GNU Make origin and flavor.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeInitialValueObservation {
    pub name: String,
    pub value: String,
    pub flavor: MakeInitialValueFlavor,
    pub origin: MakeVariableOrigin,
}

impl std::fmt::Debug for MakeInitialValueObservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakeInitialValueObservation")
            .field("name", &self.name)
            .field("flavor", &self.flavor)
            .field("origin", &self.origin)
            .field("value_bytes", &self.value.len())
            .finish_non_exhaustive()
    }
}

/// Raw observation with versioned initial-value entries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeInitialValuesObservation {
    pub version: u32,
    pub entries: Vec<MakeInitialValueObservation>,
}

/// A validated, deterministic map of the eligible initial Make values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMakeInitialValues {
    entries: BTreeMap<String, MakeInitialValueObservation>,
}

impl ValidatedMakeInitialValues {
    /// Validate values against the exact names and origins in a namespace.
    ///
    /// Values are retained verbatim, including leading and trailing spaces.
    ///
    /// # Errors
    /// Rejects unsupported versions, incomplete or extra membership, origin
    /// mismatches, invalid control characters and records over protocol bounds.
    pub fn validate(
        raw: MakeInitialValuesObservation,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: validate_entries(raw, namespace, ValueSelection::Defaults)?,
        })
    }

    /// Parse the bounded `v1;NAME|origin|flavor|value;...;end` transport form.
    ///
    /// The value field percent-escapes `%`, LF, TAB, `|` and `;`. One optional
    /// separator space before each name is removed; value whitespace is exact.
    ///
    /// # Errors
    /// Rejects malformed framing, invalid escapes, refused controls, namespace
    /// mismatches and measurements exceeding the explicit byte or entry limits.
    pub fn from_measurement(
        measurement: &str,
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Ok(Self {
            entries: parse_measurement(measurement, namespace, ValueSelection::Defaults)?,
        })
    }

    /// Select and parse exactly one metadata line from bounded Make stdout.
    ///
    /// # Errors
    /// Rejects oversized or invalid UTF-8 stdout, missing or duplicate metadata,
    /// and every malformed or refused measurement described by
    /// [`Self::from_measurement`].
    pub fn from_make_stdout(
        bytes: &[u8],
        namespace: &ValidatedMakeNamespace,
    ) -> crate::Result<Self> {
        Self::from_measurement(
            metadata_measurement(bytes, MAKE_INITIAL_VALUES_METADATA_KEY)?,
            namespace,
        )
    }

    /// Return a stable, name-sorted raw representation.
    #[must_use]
    pub fn to_observation(&self) -> MakeInitialValuesObservation {
        MakeInitialValuesObservation {
            version: 1,
            entries: self.entries.values().cloned().collect(),
        }
    }

    /// Iterate over validated entries in name order.
    pub fn entries(&self) -> impl Iterator<Item = &MakeInitialValueObservation> {
        self.entries.values()
    }
}

#[derive(Clone, Copy)]
enum ValueSelection {
    Defaults,
    CommandLine,
    Environment,
    ParserControls,
    RecipeControls,
    RecipeEngine,
}
impl ValueSelection {
    fn accepts(self, origin: MakeVariableOrigin) -> bool {
        match self {
            Self::Defaults | Self::ParserControls | Self::RecipeControls => matches!(
                origin,
                MakeVariableOrigin::File | MakeVariableOrigin::Default
            ),
            Self::CommandLine => origin == MakeVariableOrigin::CommandLine,
            Self::Environment => origin == MakeVariableOrigin::Environment,
            Self::RecipeEngine => origin == MakeVariableOrigin::Automatic,
        }
    }
    fn name_eligible(self, name: &str) -> bool {
        match self {
            Self::Defaults => is_eligible_name(name),
            Self::ParserControls => name == ".RECIPEPREFIX",
            Self::RecipeControls => name == ".SHELLFLAGS",
            Self::RecipeEngine => name == MAKE_CLI_PROPAGATION_VARIABLE,
            Self::CommandLine | Self::Environment => true,
        }
    }
}
fn validate_entries(
    raw: MakeInitialValuesObservation,
    namespace: &ValidatedMakeNamespace,
    selection: ValueSelection,
) -> crate::Result<BTreeMap<String, MakeInitialValueObservation>> {
    if raw.version != 1 {
        return Err(initial_values_error("unsupported observation version"));
    }
    if raw.entries.len() > MAX_ENTRIES {
        return Err(initial_values_error("entry count exceeds its limit"));
    }
    if matches!(selection, ValueSelection::RecipeEngine)
        && namespace
            .origin(MAKE_CLI_PROPAGATION_VARIABLE)
            .is_some_and(|origin| origin != MakeVariableOrigin::Automatic)
    {
        return Err(initial_values_error(
            "recipe engine name has an unsupported namespace origin",
        ));
    }

    let mut eligible = BTreeMap::new();
    for entry in namespace.to_observation().entries {
        if selection.name_eligible(&entry.name) && selection.accepts(entry.origin) {
            eligible.insert(entry.name, entry.origin);
        }
    }

    let mut entries = BTreeMap::new();
    let mut raw_total = 0usize;
    let mut encoded_len = "v1;".len().saturating_add("end".len());
    for entry in raw.entries {
        if !selection.name_eligible(&entry.name) {
            return Err(initial_values_error("observation contains a refused name"));
        }
        if !selection.accepts(entry.origin) {
            return Err(initial_values_error(
                "observation contains a refused origin",
            ));
        }
        if matches!(selection, ValueSelection::RecipeEngine)
            && entry.flavor != MakeInitialValueFlavor::Simple
        {
            return Err(initial_values_error(
                "recipe engine scalar must have measured simple flavor",
            ));
        }
        if namespace.origin(&entry.name) != Some(entry.origin)
            || eligible.get(&entry.name) != Some(&entry.origin)
        {
            return Err(initial_values_error(
                "name or origin does not match the namespace",
            ));
        }
        if entries.contains_key(&entry.name) {
            return Err(initial_values_error(
                "observation contains a duplicate name",
            ));
        }

        let raw_value_bytes = entry.value.len();
        if raw_value_bytes > MAX_RAW_VALUE_BYTES {
            return Err(initial_values_error("raw value exceeds its byte limit"));
        }
        if contains_refused_control(&entry.value) {
            return Err(initial_values_error(
                "raw value contains a refused control character",
            ));
        }
        raw_total = raw_total.saturating_add(raw_value_bytes);
        if raw_total > MAX_RAW_TOTAL_BYTES {
            return Err(initial_values_error(
                "combined raw values exceed their byte limit",
            ));
        }

        let encoded_value_bytes = encoded_value_len(&entry.value);
        encoded_len = encoded_len
            .saturating_add(entry.name.len())
            .saturating_add(1)
            .saturating_add(origin_label(entry.origin).len())
            .saturating_add(1)
            .saturating_add(flavor_label(entry.flavor).len())
            .saturating_add(1)
            .saturating_add(encoded_value_bytes)
            .saturating_add(1); // The record separator before the next record/end.
        if encoded_len > MAX_MEASUREMENT_BYTES {
            return Err(initial_values_error(
                "encoded measurement exceeds its byte limit",
            ));
        }
        entries.insert(entry.name.clone(), entry);
    }

    if entries.len() != eligible.len() || eligible.keys().any(|name| !entries.contains_key(name)) {
        return Err(initial_values_error(
            "observation does not contain every eligible namespace name",
        ));
    }

    Ok(entries)
}
fn parse_measurement(
    measurement: &str,
    namespace: &ValidatedMakeNamespace,
    selection: ValueSelection,
) -> crate::Result<BTreeMap<String, MakeInitialValueObservation>> {
    if measurement.len() > MAX_MEASUREMENT_BYTES {
        return Err(initial_values_error(
            "encoded measurement exceeds its byte limit",
        ));
    }
    if measurement.chars().any(char::is_control) {
        return Err(initial_values_error(
            "measurement contains a control character",
        ));
    }

    let mut records = measurement.split(';');
    if records.next() != Some("v1") {
        return Err(initial_values_error("measurement has invalid framing"));
    }

    let mut entries = Vec::new();
    loop {
        let record = records
            .next()
            .ok_or_else(|| initial_values_error("measurement is missing its end marker"))?;
        if record == "end" {
            if records.next().is_some() {
                return Err(initial_values_error(
                    "measurement has data after its end marker",
                ));
            }
            break;
        }
        if entries.len() >= MAX_ENTRIES {
            return Err(initial_values_error("entry count exceeds its limit"));
        }

        let record = record.strip_prefix(' ').unwrap_or(record);
        let mut fields = record.split('|');
        let name = fields
            .next()
            .ok_or_else(|| initial_values_error("record is missing its name field"))?;
        let origin = fields
            .next()
            .ok_or_else(|| initial_values_error("record is missing its origin field"))?;
        let flavor = fields
            .next()
            .ok_or_else(|| initial_values_error("record is missing its flavor field"))?;
        let encoded_value = fields
            .next()
            .ok_or_else(|| initial_values_error("record is missing its value field"))?;
        if fields.next().is_some() {
            return Err(initial_values_error("record has an invalid field count"));
        }

        let origin = match origin {
            "file" => MakeVariableOrigin::File,
            "default" => MakeVariableOrigin::Default,
            "command line" => MakeVariableOrigin::CommandLine,
            "environment" => MakeVariableOrigin::Environment,
            "automatic" if matches!(selection, ValueSelection::RecipeEngine) => {
                MakeVariableOrigin::Automatic
            }
            _ => return Err(initial_values_error("record has a refused origin")),
        };
        let flavor = match flavor {
            "simple" => MakeInitialValueFlavor::Simple,
            "recursive" => MakeInitialValueFlavor::Recursive,
            _ => return Err(initial_values_error("record has an invalid flavor")),
        };
        let value = decode_value(encoded_value)?;
        entries.push(MakeInitialValueObservation {
            name: name.to_owned(),
            value,
            flavor,
            origin,
        });
    }

    validate_entries(
        MakeInitialValuesObservation {
            version: 1,
            entries,
        },
        namespace,
        selection,
    )
}
fn metadata_measurement<'a>(bytes: &'a [u8], metadata_key: &str) -> crate::Result<&'a str> {
    if bytes.len() > MAX_STDOUT_BYTES {
        return Err(initial_values_error("Make stdout exceeds its byte limit"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| initial_values_error("Make stdout is not UTF-8"))?;
    let mut selected = None;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key == metadata_key && selected.replace(value).is_some() {
                return Err(initial_values_error("duplicate metadata line"));
            }
        }
    }
    let measurement = selected.ok_or_else(|| initial_values_error("metadata line is missing"))?;
    Ok(measurement)
}

fn is_eligible_name(name: &str) -> bool {
    !name.starts_with('.') && !RESERVED_HELPERS.contains(&name)
}

const fn origin_label(origin: MakeVariableOrigin) -> &'static str {
    match origin {
        MakeVariableOrigin::File => "file",
        MakeVariableOrigin::Default => "default",
        MakeVariableOrigin::CommandLine => "command line",
        MakeVariableOrigin::Environment => "environment",
        MakeVariableOrigin::Automatic => "automatic",
        _ => "",
    }
}

const fn flavor_label(flavor: MakeInitialValueFlavor) -> &'static str {
    match flavor {
        MakeInitialValueFlavor::Simple => "simple",
        MakeInitialValueFlavor::Recursive => "recursive",
    }
}

fn contains_refused_control(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
}

fn encoded_value_len(value: &str) -> usize {
    value.bytes().fold(0usize, |length, byte| {
        length.saturating_add(if matches!(byte, b'%' | b'\n' | b'\t' | b'|' | b';') {
            3
        } else {
            1
        })
    })
}

fn decode_value(encoded: &str) -> crate::Result<String> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0usize;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let escape = bytes
                .get(offset..offset.saturating_add(3))
                .ok_or_else(|| initial_values_error("value has an incomplete escape"))?;
            let byte = match escape {
                b"%25" => b'%',
                b"%0A" => b'\n',
                b"%09" => b'\t',
                b"%7C" => b'|',
                b"%3B" => b';',
                _ => return Err(initial_values_error("value has an invalid escape")),
            };
            decoded.push(byte);
            offset += 3;
        } else {
            decoded.push(bytes[offset]);
            offset += 1;
        }
        if decoded.len() > MAX_RAW_VALUE_BYTES {
            return Err(initial_values_error("raw value exceeds its byte limit"));
        }
    }

    let value = String::from_utf8(decoded)
        .map_err(|_| initial_values_error("decoded value is not UTF-8"))?;
    if contains_refused_control(&value) {
        return Err(initial_values_error(
            "raw value contains a refused control character",
        ));
    }
    Ok(value)
}

fn initial_values_error(message: &str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: CONFIGURATION_FILE.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace() -> ValidatedMakeNamespace {
        ValidatedMakeNamespace::from_measurement(
            "v1:6;.VARIABLES|default;MAKE_VERSION|file;A|file;B|default;.HIDDEN|file;__AROS_INITIAL_VALUES_ITEM|file;end",
        )
        .unwrap()
    }

    fn observation(entries: Vec<MakeInitialValueObservation>) -> MakeInitialValuesObservation {
        MakeInitialValuesObservation {
            version: 1,
            entries,
        }
    }

    fn entry(
        name: &str,
        value: &str,
        flavor: MakeInitialValueFlavor,
        origin: MakeVariableOrigin,
    ) -> MakeInitialValueObservation {
        MakeInitialValueObservation {
            name: name.to_owned(),
            value: value.to_owned(),
            flavor,
            origin,
        }
    }

    #[test]
    fn parser_preserves_whitespace_dollars_and_escaped_multiline_values() {
        let validated = ValidatedMakeInitialValues::from_measurement(
            "v1; A|file|recursive|  $$HOME  ; B|default|simple|line%0Awith%09tab%25%7C%3B; MAKE_VERSION|file|simple|4.4;end",
            &namespace(),
        )
        .unwrap();
        let entries = validated.to_observation().entries;
        assert_eq!(entries[0].name, "A");
        assert_eq!(entries[0].value, "  $$HOME  ");
        assert_eq!(entries[0].flavor, MakeInitialValueFlavor::Recursive);
        assert_eq!(entries[1].value, "line\nwith\ttab%|;");
        assert_eq!(entries[2].value, "4.4");
    }

    #[test]
    fn stdout_selects_one_exact_metadata_line() {
        let good = b"OTHER=ignored\nAROS_MAKE_INITIAL_VALUES=v1;A|file|simple|a;B|default|recursive|b;MAKE_VERSION|file|simple|4;end\n";
        assert!(ValidatedMakeInitialValues::from_make_stdout(good, &namespace()).is_ok());
        assert!(ValidatedMakeInitialValues::from_make_stdout(
            b"AROS_MAKE_INITIAL_VALUES_EXTRA=v1;end\n",
            &namespace()
        )
        .is_err());
        assert!(ValidatedMakeInitialValues::from_make_stdout(
            b"AROS_MAKE_INITIAL_VALUES=v1;A|file|simple|a;B|default|recursive|b;MAKE_VERSION|file|simple|4;end\nAROS_MAKE_INITIAL_VALUES=v1;end\n",
            &namespace()
        )
        .is_err());
    }

    #[test]
    fn validation_rejects_membership_origin_framing_and_escape_errors() {
        let ns = namespace();
        let valid = vec![
            entry(
                "A",
                "a",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::File,
            ),
            entry(
                "B",
                "b",
                MakeInitialValueFlavor::Recursive,
                MakeVariableOrigin::Default,
            ),
            entry(
                "MAKE_VERSION",
                "4",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::File,
            ),
        ];
        assert!(ValidatedMakeInitialValues::validate(observation(valid.clone()), &ns).is_ok());

        let mut omitted = valid.clone();
        omitted.pop();
        assert!(ValidatedMakeInitialValues::validate(observation(omitted), &ns).is_err());
        let mut wrong_origin = valid.clone();
        wrong_origin[0].origin = MakeVariableOrigin::Default;
        assert!(ValidatedMakeInitialValues::validate(observation(wrong_origin), &ns).is_err());
        let mut duplicate = valid.clone();
        duplicate.push(valid[0].clone());
        assert!(ValidatedMakeInitialValues::validate(observation(duplicate), &ns).is_err());
        let mut extra = valid;
        extra[0].name = "UNKNOWN".to_owned();
        assert!(ValidatedMakeInitialValues::validate(observation(extra), &ns).is_err());

        for malformed in [
            "v2;A|file|simple|a;B|default|simple|b;MAKE_VERSION|file|simple|4;end",
            "v1;A|file|simple|a;B|default|simple|b;MAKE_VERSION|file|simple|4",
            "v1;A|file|simple|a;B|default|simple|b;MAKE_VERSION|file|simple|4;end;tail",
            "v1;A|file|simple|bad%2Fescape;B|default|simple|b;MAKE_VERSION|file|simple|4;end",
            "v1;A|file|simple|bad%;B|default|simple|b;MAKE_VERSION|file|simple|4;end",
            "v1;A|file|simple;B|default|simple|b;MAKE_VERSION|file|simple|4;end",
        ] {
            assert!(ValidatedMakeInitialValues::from_measurement(malformed, &ns).is_err());
        }

        assert!(ValidatedMakeInitialValues::from_measurement(
            "v1;A|file|simple|;B|default|simple|b;MAKE_VERSION|file|simple|4;end",
            &ns,
        )
        .is_ok());
    }

    #[test]
    fn controls_and_all_byte_bounds_are_enforced_for_raw_observations() {
        let ns = namespace();
        let mut entries = vec![
            entry(
                "A",
                "x",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::File,
            ),
            entry(
                "B",
                "y",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::Default,
            ),
            entry(
                "MAKE_VERSION",
                "4",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::File,
            ),
        ];
        entries[0].value = "bad\rvalue".to_owned();
        assert!(ValidatedMakeInitialValues::validate(observation(entries.clone()), &ns).is_err());

        entries[0].value = "x".repeat(MAX_RAW_VALUE_BYTES + 1);
        assert!(ValidatedMakeInitialValues::validate(observation(entries.clone()), &ns).is_err());

        entries[0].value = "|".repeat(40 * 1024);
        entries[1].value = "|".repeat(40 * 1024);
        entries[2].value.clear();
        assert!(ValidatedMakeInitialValues::validate(observation(entries.clone()), &ns).is_err());

        entries[0].value = "x".repeat(43 * 1024);
        entries[1].value = "x".repeat(43 * 1024);
        entries[2].value = "x".repeat(43 * 1024);
        assert!(ValidatedMakeInitialValues::validate(observation(entries), &ns).is_err());

        let repeated = vec![
            entry(
                "A",
                "",
                MakeInitialValueFlavor::Simple,
                MakeVariableOrigin::File
            );
            MAX_ENTRIES + 1
        ];
        assert!(ValidatedMakeInitialValues::validate(observation(repeated), &ns).is_err());

        assert!(ValidatedMakeInitialValues::from_measurement(
            &"x".repeat(MAX_MEASUREMENT_BYTES + 1),
            &ns
        )
        .is_err());
        assert!(ValidatedMakeInitialValues::from_make_stdout(
            &vec![b'x'; MAX_STDOUT_BYTES + 1],
            &ns
        )
        .is_err());
    }
}
