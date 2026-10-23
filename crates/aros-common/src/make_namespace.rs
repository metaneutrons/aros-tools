//! Bounded, data-only observations of a GNU Make variable namespace.
//!
//! An observation records variable names and GNU Make origins, never values.
//! The data does not authenticate its producer. Its completeness is meaningful
//! only when the caller obtains it through an external snapshot protocol that
//! establishes when and how the observation was taken.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const MAX_MEASUREMENT_BYTES: usize = 64 * 1024;
const MAX_NAMESPACE_ENTRIES: usize = 4096;
const MAX_VARIABLE_NAME_BYTES: usize = 128;
const CONFIGURATION_FILE: &str = "Make initial namespace";

/// Metadata key in the retained Make stdout, not a measured variable value.
pub const MAKE_NAMESPACE_METADATA_KEY: &str = "AROS_MAKE_INITIAL_NAMESPACE";

/// Raw namespace data. This describes names and origins only; it contains no
/// variable values and does not authenticate the process that produced it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeNamespaceObservation {
    pub version: u32,
    pub entries: Vec<MakeNamespaceEntry>,
}

/// A raw GNU Make variable name and its reported origin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakeNamespaceEntry {
    pub name: String,
    pub origin: MakeVariableOrigin,
}

/// The exact origin labels returned by GNU Make's `origin` function.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MakeVariableOrigin {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "environment")]
    Environment,
    #[serde(rename = "environment override")]
    EnvironmentOverride,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "command line")]
    CommandLine,
    #[serde(rename = "override")]
    Override,
    #[serde(rename = "automatic")]
    Automatic,
}

impl MakeVariableOrigin {
    /// Exact label returned by GNU Make's `origin` function.
    #[must_use]
    pub const fn as_gnu_label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Environment => "environment",
            Self::EnvironmentOverride => "environment override",
            Self::File => "file",
            Self::CommandLine => "command line",
            Self::Override => "override",
            Self::Automatic => "automatic",
        }
    }

    fn from_gnu_label(label: &str) -> Option<Self> {
        match label {
            "default" => Some(Self::Default),
            "environment" => Some(Self::Environment),
            "environment override" => Some(Self::EnvironmentOverride),
            "file" => Some(Self::File),
            "command line" => Some(Self::CommandLine),
            "override" => Some(Self::Override),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }
}

/// A validated, deterministic view of a Make namespace.
///
/// The map contains only names and origins. Any claim that the observation is
/// complete depends on an external snapshot protocol; this type alone cannot
/// authenticate the producer or establish completeness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedMakeNamespace {
    entries: BTreeMap<String, MakeVariableOrigin>,
}

impl ValidatedMakeNamespace {
    /// Read exactly one namespace metadata line from bounded retained stdout.
    /// Other measured values are not expanded, interpreted or returned here.
    ///
    /// # Errors
    /// Refuses invalid UTF-8, oversized logs, missing/duplicate metadata and
    /// malformed namespace records. This does not authenticate the log.
    pub fn from_make_stdout(bytes: &[u8]) -> crate::Result<Self> {
        if bytes.len() > 256 * 1024 {
            return Err(configuration_error("Make stdout exceeds its byte limit"));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| configuration_error("Make stdout is not UTF-8"))?;
        let mut selected = None;
        for line in text.lines() {
            if let Some((MAKE_NAMESPACE_METADATA_KEY, value)) = line.split_once('=') {
                if selected.replace(value).is_some() {
                    return Err(configuration_error("duplicate namespace metadata"));
                }
            }
        }
        Self::from_measurement(
            selected.ok_or_else(|| configuration_error("missing namespace metadata"))?,
        )
    }
    /// Validate a raw namespace observation and store its entries by name.
    ///
    /// # Errors
    /// Rejects unsupported versions, invalid/duplicate names, missing anchors
    /// and measurements exceeding the explicit byte, count or name bounds.
    pub fn validate(raw: MakeNamespaceObservation) -> crate::Result<Self> {
        if raw.version != 1 {
            return Err(configuration_error(format!(
                "unsupported observation version {}; expected 1",
                raw.version
            )));
        }
        if raw.entries.len() > MAX_NAMESPACE_ENTRIES {
            return Err(configuration_error(format!(
                "namespace has {} entries; limit is {MAX_NAMESPACE_ENTRIES}",
                raw.entries.len()
            )));
        }

        // Keep the raw form within the same encoded measurement bound as the
        // textual form, so callers cannot bypass that limit through serde.
        let mut encoded_len = format!("v1:{};", raw.entries.len()).len() + "end".len();
        for entry in &raw.entries {
            encoded_len = encoded_len.saturating_add(
                entry
                    .name
                    .len()
                    .saturating_add(entry.origin.as_gnu_label().len())
                    .saturating_add(2), // `|` and the record's trailing `;`
            );
        }
        if encoded_len > MAX_MEASUREMENT_BYTES {
            return Err(configuration_error(format!(
                "namespace measurement is {encoded_len} bytes; limit is {MAX_MEASUREMENT_BYTES}"
            )));
        }

        let mut entries = BTreeMap::new();
        for entry in raw.entries {
            if !is_valid_gnu_variable_name(&entry.name) {
                return Err(configuration_error(format!(
                    "invalid GNU Make variable name {:?}",
                    entry.name
                )));
            }
            if entry.name.len() > MAX_VARIABLE_NAME_BYTES {
                return Err(configuration_error(format!(
                    "variable name {:?} exceeds the {MAX_VARIABLE_NAME_BYTES}-byte limit",
                    entry.name
                )));
            }
            if entries.insert(entry.name.clone(), entry.origin).is_some() {
                return Err(configuration_error(format!(
                    "duplicate variable name {:?}",
                    entry.name
                )));
            }
        }

        for required in [".VARIABLES", "MAKE_VERSION"] {
            if !entries.contains_key(required) {
                return Err(configuration_error(format!(
                    "namespace is missing required completeness anchor {required:?}"
                )));
            }
        }

        Ok(Self { entries })
    }

    /// Parse the bounded `v1:<count>;<name>|<origin>;...;end` transport form.
    ///
    /// Whitespace around each record is trimmed; field contents remain exact,
    /// including internal spaces in GNU origin labels.
    ///
    /// # Errors
    /// Rejects malformed/truncated frames, invalid origins and every invalid
    /// raw observation described by [`Self::validate`].
    pub fn from_measurement(measurement: &str) -> crate::Result<Self> {
        if measurement.len() > MAX_MEASUREMENT_BYTES {
            return Err(configuration_error(format!(
                "namespace measurement is {} bytes; limit is {MAX_MEASUREMENT_BYTES}",
                measurement.len()
            )));
        }

        let measurement = measurement.trim();
        let mut records = measurement.split(';');
        let header = records.next().unwrap_or_default();
        let count_text = header
            .strip_prefix("v1:")
            .ok_or_else(|| configuration_error("measurement must begin with the v1 header"))?;
        if count_text.is_empty() || !count_text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(configuration_error(
                "measurement entry count must be an unsigned decimal integer",
            ));
        }
        let count = count_text.parse::<usize>().map_err(|_| {
            configuration_error("measurement entry count is outside the supported range")
        })?;
        if count > MAX_NAMESPACE_ENTRIES {
            return Err(configuration_error(format!(
                "namespace has {count} entries; limit is {MAX_NAMESPACE_ENTRIES}"
            )));
        }

        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let record = records.next().ok_or_else(|| {
                configuration_error(format!(
                    "measurement is truncated before entry {} of {count}",
                    index + 1
                ))
            })?;
            let record = record.trim();
            let (name, origin) = record.split_once('|').ok_or_else(|| {
                configuration_error(format!(
                    "entry {} must contain one name/origin separator",
                    index + 1
                ))
            })?;
            if origin.contains('|') {
                return Err(configuration_error(format!(
                    "entry {} contains more than one name/origin separator",
                    index + 1
                )));
            }
            let origin = MakeVariableOrigin::from_gnu_label(origin).ok_or_else(|| {
                configuration_error(format!(
                    "entry {} has undefined GNU Make origin {:?}",
                    index + 1,
                    origin
                ))
            })?;
            entries.push(MakeNamespaceEntry {
                name: name.to_owned(),
                origin,
            });
        }

        let terminator = records
            .next()
            .ok_or_else(|| configuration_error("measurement is missing the end marker"))?;
        if terminator.trim() != "end" {
            return Err(configuration_error(
                "measurement entry count does not match its end marker",
            ));
        }
        if records.next().is_some() {
            return Err(configuration_error(
                "measurement contains data after the end marker",
            ));
        }

        Self::validate(MakeNamespaceObservation {
            version: 1,
            entries,
        })
    }

    /// Return a stable, name-sorted raw representation.
    #[must_use]
    pub fn to_observation(&self) -> MakeNamespaceObservation {
        MakeNamespaceObservation {
            version: 1,
            entries: self
                .entries
                .iter()
                .map(|(name, origin)| MakeNamespaceEntry {
                    name: name.clone(),
                    origin: *origin,
                })
                .collect(),
        }
    }

    /// Return exact membership, including GNU's non-scalar built-in names.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Return the exact observed origin without implying that its value is known.
    #[must_use]
    pub fn origin(&self, name: &str) -> Option<MakeVariableOrigin> {
        self.entries.get(name).copied()
    }

    /// Return whether an admitted literal variable name is absent.
    ///
    /// This conclusion is useful only if an external snapshot protocol makes
    /// the observation complete. Invalid query names return `false`.
    #[must_use]
    pub fn proves_absent(&self, name: &str) -> bool {
        is_literal_variable_name(name) && !self.entries.contains_key(name)
    }

    /// Prove startup absence for an already-expanded source reference name.
    ///
    /// Reference names are data, not assignment syntax. This admits ordinary
    /// single-character references as well as the existing literal-name set.
    /// Longer punctuation-bearing names may designate unobserved GNU internal
    /// state and remain unproved; assignment-name admission is unchanged.
    /// Automatic and multi-character dot-prefixed engine state still need a
    /// separate context; the single-character `.` is an ordinary reference.
    /// As with `proves_absent`, completeness requires the external observation
    /// protocol; this does not prove absence after an unmodeled state change.
    #[must_use]
    pub fn proves_absent_source_reference(&self, name: &str) -> bool {
        if name.len() > MAX_VARIABLE_NAME_BYTES
            || !is_valid_gnu_variable_name(name)
            || (name.starts_with('.') && name.len() != 1)
            || (!is_literal_variable_name(name) && name.len() != 1)
        {
            return false;
        }
        let bytes = name.as_bytes();
        let automatic = b"@%<?^+*|".contains(&bytes[0])
            && (bytes.len() == 1 || (bytes.len() == 2 && matches!(bytes[1], b'D' | b'F')));
        !automatic && !self.entries.contains_key(name)
    }
}

fn is_valid_gnu_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b';' && byte != b'|')
}

fn is_literal_variable_name(name: &str) -> bool {
    // GNU names need not be C identifiers. Keep the source reader's literal
    // alphanumeric/underscore/hyphen subset, excluding automatic variables,
    // computed names and syntax with expansion or delimiter semantics.
    !name.is_empty()
        && name.len() <= MAX_VARIABLE_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn configuration_error(message: impl Into<String>) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: CONFIGURATION_FILE.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_stdout_selects_one_metadata_line_without_reading_values() {
        let good = b"UNRELATED=not-a-namespace\nAROS_MAKE_INITIAL_NAMESPACE=v1:2;.VARIABLES|default;MAKE_VERSION|default;end\n";
        assert!(ValidatedMakeNamespace::from_make_stdout(good)
            .unwrap()
            .proves_absent("ANY_SCALAR"));
        assert!(ValidatedMakeNamespace::from_make_stdout(b"OTHER=value\n").is_err());
        let mut duplicate = good.to_vec();
        duplicate.extend_from_slice(good);
        assert!(ValidatedMakeNamespace::from_make_stdout(&duplicate).is_err());
        assert!(ValidatedMakeNamespace::from_make_stdout(&[0xff]).is_err());
        assert!(ValidatedMakeNamespace::from_make_stdout(&vec![b'x'; 256 * 1024 + 1]).is_err());
    }

    fn entry(name: &str, origin: MakeVariableOrigin) -> MakeNamespaceEntry {
        MakeNamespaceEntry {
            name: name.to_owned(),
            origin,
        }
    }

    fn raw(entries: Vec<MakeNamespaceEntry>) -> MakeNamespaceObservation {
        MakeNamespaceObservation {
            version: 1,
            entries,
        }
    }

    fn framed(records: &[(&str, &str)]) -> String {
        let mut measurement = format!("v1:{};", records.len());
        for (name, origin) in records {
            measurement.push_str(name);
            measurement.push('|');
            measurement.push_str(origin);
            measurement.push(';');
        }
        measurement.push_str("end");
        measurement
    }

    fn anchored_records() -> Vec<(&'static str, &'static str)> {
        vec![(".VARIABLES", "default"), ("MAKE_VERSION", "file")]
    }

    #[test]
    fn source_reference_absence_is_distinct_from_assignment_admission() {
        let namespace =
            ValidatedMakeNamespace::from_measurement(&framed(&anchored_records())).unwrap();
        for name in ["/", ".", "!", "=", ":", "ORDINARY_NAME"] {
            assert!(namespace.proves_absent_source_reference(name));
        }
        assert!(!namespace.proves_absent("/"));
        let mut records = anchored_records();
        records.push(("/", "file"));
        let present = ValidatedMakeNamespace::from_measurement(&framed(&records)).unwrap();
        assert!(present.contains("/"));
        assert!(!present.proves_absent_source_reference("/"));
        let mut records = anchored_records();
        records.push((".", "file"));
        let present = ValidatedMakeNamespace::from_measurement(&framed(&records)).unwrap();
        assert!(present.contains("."));
        assert!(!present.proves_absent_source_reference("."));
        assert!(!namespace.proves_absent("."));
    }

    #[test]
    fn source_reference_absence_does_not_invent_automatic_or_engine_state() {
        let namespace =
            ValidatedMakeNamespace::from_measurement(&framed(&anchored_records())).unwrap();
        for name in [
            "",
            ".MISSING",
            ".SHELLSTATUS",
            ".VARIABLES",
            "A B",
            "A|B",
            "A;B",
            "é",
            "A=B",
            "A%",
            "query/name",
            "+ordinary",
            "-*-eval-flags-*-",
        ] {
            assert!(!namespace.proves_absent_source_reference(name));
        }
        for automatic in ["@", "%", "<", "?", "^", "+", "*", "|"] {
            for suffix in ["", "D", "F"] {
                assert!(!namespace.proves_absent_source_reference(&format!("{automatic}{suffix}")));
            }
        }
        assert!(!namespace.proves_absent_source_reference(&"x".repeat(MAX_VARIABLE_NAME_BYTES + 1)));
    }

    #[test]
    fn validates_and_returns_deterministic_observation() {
        let namespace = ValidatedMakeNamespace::validate(raw(vec![
            entry("ZED", MakeVariableOrigin::Environment),
            entry(".VARIABLES", MakeVariableOrigin::Default),
            entry("MAKE_VERSION", MakeVariableOrigin::File),
            entry("ALPHA", MakeVariableOrigin::CommandLine),
        ]))
        .unwrap();

        assert!(namespace.contains("MAKE_VERSION"));
        assert!(namespace.contains("ALPHA"));
        assert_eq!(
            namespace
                .to_observation()
                .entries
                .into_iter()
                .map(|item| item.name)
                .collect::<Vec<_>>(),
            vec![
                ".VARIABLES".to_owned(),
                "ALPHA".to_owned(),
                "MAKE_VERSION".to_owned(),
                "ZED".to_owned()
            ]
        );
    }

    #[test]
    fn measurement_accepts_all_exact_gnu_origins() {
        let measurement = framed(&[
            (".VARIABLES", "default"),
            ("MAKE_VERSION", "environment"),
            ("A", "environment override"),
            ("B", "file"),
            ("C", "command line"),
            ("D", "override"),
            ("E", "automatic"),
        ]);
        let namespace = ValidatedMakeNamespace::from_measurement(&measurement).unwrap();

        assert_eq!(namespace.entries[".VARIABLES"], MakeVariableOrigin::Default);
        assert_eq!(
            namespace.entries["MAKE_VERSION"],
            MakeVariableOrigin::Environment
        );
        assert_eq!(
            namespace.entries["A"],
            MakeVariableOrigin::EnvironmentOverride
        );
        assert_eq!(namespace.entries["B"], MakeVariableOrigin::File);
        assert_eq!(namespace.entries["C"], MakeVariableOrigin::CommandLine);
        assert_eq!(namespace.entries["D"], MakeVariableOrigin::Override);
        assert_eq!(namespace.entries["E"], MakeVariableOrigin::Automatic);
    }

    #[test]
    fn measurement_accepts_other_graphic_gnu_names() {
        let measurement = framed(&[
            (".VARIABLES", "default"),
            ("MAKE_VERSION", "file"),
            ("%D", "automatic"),
            (",D", "automatic"),
            ("<D", "automatic"),
            ("?D", "automatic"),
            ("@D", "automatic"),
            ("^D", "automatic"),
        ]);

        let namespace = ValidatedMakeNamespace::from_measurement(&measurement).unwrap();
        for name in ["%D", ",D", "<D", "?D", "@D", "^D"] {
            assert!(namespace.entries.contains_key(name));
        }
    }

    #[test]
    fn measurement_trims_record_margins_and_preserves_origin_spaces() {
        let namespace = ValidatedMakeNamespace::from_measurement(
            "v1:2; .VARIABLES|default; MAKE_VERSION|environment override ; end\n",
        )
        .unwrap();

        assert_eq!(
            namespace.entries["MAKE_VERSION"],
            MakeVariableOrigin::EnvironmentOverride
        );
    }

    #[test]
    fn raw_structs_reject_unknown_serde_fields() {
        assert!(serde_json::from_str::<MakeNamespaceObservation>(
            r#"{"version":1,"entries":[],"extra":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MakeNamespaceEntry>(
            r#"{"name":"A","origin":"file","extra":true}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MakeNamespaceEntry>(
            r#"{"name":"A","origin":"environment-override"}"#
        )
        .is_err());
    }

    #[test]
    fn rejects_invalid_version_and_missing_anchors() {
        assert!(ValidatedMakeNamespace::validate(MakeNamespaceObservation {
            version: 2,
            entries: vec![entry(".VARIABLES", MakeVariableOrigin::Default)],
        })
        .is_err());
        assert!(ValidatedMakeNamespace::validate(raw(vec![entry(
            ".VARIABLES",
            MakeVariableOrigin::Default
        )]))
        .is_err());
    }

    #[test]
    fn rejects_bad_frames_truncation_and_undefined_origins() {
        let valid = framed(&anchored_records());
        let cases = [
            valid.replacen("v1:", "v2:", 1),
            "v1:2;.VARIABLES|default;MAKE_VERSION|file".to_owned(),
            "v1:2;.VARIABLES|default;MAKE_VERSION|file;wrong".to_owned(),
            format!("{valid};extra"),
            "v1:2;.VARIABLES |default;MAKE_VERSION|file;end".to_owned(),
            "v1:2;.VARIABLES| default;MAKE_VERSION|file;end".to_owned(),
            framed(&[(".VARIABLES", "default"), ("MAKE_VERSION", "not an origin")]),
            "v1:x;.VARIABLES|default;MAKE_VERSION|file;end".to_owned(),
        ];
        for measurement in cases {
            assert!(
                ValidatedMakeNamespace::from_measurement(&measurement).is_err(),
                "accepted malformed measurement {measurement:?}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_names_and_count_mismatch() {
        assert!(ValidatedMakeNamespace::from_measurement(&framed(&[
            (".VARIABLES", "default"),
            ("MAKE_VERSION", "file"),
            ("MAKE_VERSION", "environment"),
        ]))
        .is_err());
        assert!(ValidatedMakeNamespace::from_measurement(
            "v1:3;.VARIABLES|default;MAKE_VERSION|file;end"
        )
        .is_err());
        assert!(ValidatedMakeNamespace::from_measurement(
            "v1:1;.VARIABLES|default;MAKE_VERSION|file;end"
        )
        .is_err());
    }

    #[test]
    fn enforces_measurement_entry_name_and_byte_limits() {
        let mut at_limit = vec![
            (".VARIABLES".to_owned(), "default".to_owned()),
            ("MAKE_VERSION".to_owned(), "file".to_owned()),
        ];
        at_limit.extend(
            (0..MAX_NAMESPACE_ENTRIES - 2).map(|index| (format!("V{index}"), "file".to_owned())),
        );
        let at_limit = at_limit
            .iter()
            .map(|(name, origin)| (name.as_str(), origin.as_str()))
            .collect::<Vec<_>>();
        assert!(ValidatedMakeNamespace::from_measurement(&framed(&at_limit)).is_ok());

        let mut too_many = at_limit
            .iter()
            .map(|(name, origin)| ((*name).to_owned(), (*origin).to_owned()))
            .collect::<Vec<_>>();
        too_many.push(("EXTRA".to_owned(), "file".to_owned()));
        let too_many = too_many
            .iter()
            .map(|(name, origin)| (name.as_str(), origin.as_str()))
            .collect::<Vec<_>>();
        assert!(ValidatedMakeNamespace::from_measurement(&framed(&too_many)).is_err());

        let too_long = format!("A{}", "B".repeat(MAX_VARIABLE_NAME_BYTES));
        assert!(ValidatedMakeNamespace::from_measurement(&framed(&[
            (".VARIABLES", "default"),
            ("MAKE_VERSION", "file"),
            (&too_long, "file"),
        ]))
        .is_err());

        let longest_valid = format!("A{}", "B".repeat(MAX_VARIABLE_NAME_BYTES - 1));
        assert!(ValidatedMakeNamespace::from_measurement(&framed(&[
            (".VARIABLES", "default"),
            ("MAKE_VERSION", "file"),
            (&longest_valid, "file"),
        ]))
        .is_ok());

        assert!(
            ValidatedMakeNamespace::from_measurement(&"x".repeat(MAX_MEASUREMENT_BYTES + 1))
                .is_err()
        );
    }

    #[test]
    fn rejects_non_graphic_non_ascii_or_delimiter_names() {
        for unsafe_name in [
            "HAS SPACE",
            "HAS\tTAB",
            "HAS;SEMICOLON",
            "HAS|PIPE",
            "NÖNASCII",
        ] {
            let measurement = framed(&[
                (".VARIABLES", "default"),
                ("MAKE_VERSION", "file"),
                (unsafe_name, "file"),
            ]);
            assert!(
                ValidatedMakeNamespace::from_measurement(&measurement).is_err(),
                "accepted unsafe name {unsafe_name:?}"
            );
        }
    }

    #[test]
    fn absence_queries_accept_literal_names_not_automatic_or_computed_names() {
        let namespace = ValidatedMakeNamespace::validate(raw(vec![
            entry(".VARIABLES", MakeVariableOrigin::Default),
            entry("MAKE_VERSION", MakeVariableOrigin::File),
            entry("PRESENT_1", MakeVariableOrigin::Environment),
            entry("HAS-DASH", MakeVariableOrigin::File),
        ]))
        .unwrap();

        assert!(namespace.contains("PRESENT_1"));
        assert!(namespace.contains("HAS-DASH"));
        assert!(namespace.contains(".VARIABLES"));
        assert!(!namespace.contains("1LEADING_DIGIT"));
        assert!(namespace.proves_absent("MISSING_2"));
        assert!(namespace.proves_absent("MISSING-NAME"));
        assert!(namespace.proves_absent("1LEADING_DIGIT"));
        assert!(namespace.proves_absent("-"));
        assert!(!namespace.proves_absent("PRESENT_1"));
        assert!(!namespace.proves_absent("HAS-DASH"));
        assert!(!namespace.proves_absent(".VARIABLES"));
        assert!(!namespace.proves_absent(""));
        for name in [
            "@",
            "<",
            "@D",
            ".MISSING",
            "$(MISSING)",
            "A B",
            "A=B",
            "A|B",
            "A;B",
            "A%",
            "é",
        ] {
            assert!(!namespace.proves_absent(name));
        }
        assert!(!namespace.proves_absent(&"x".repeat(MAX_VARIABLE_NAME_BYTES + 1)));
    }
}
