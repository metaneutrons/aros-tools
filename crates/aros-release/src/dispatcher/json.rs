//! Closed JSON reading and canonical writing for the dispatcher contracts.
//!
//! Mirrors the Python oracle (`release-admission-v2.py`): duplicate object keys
//! and non-finite numbers are rejected, canonical output uses sorted keys,
//! compact separators and ASCII-only escapes. Floating-point numbers are
//! rejected outright (a deliberate tightening: every closed contract carries
//! integers only, and the Python checks reject floats through `type(x) is int`).

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use super::{fail, Result};

/// Strict JSON value; objects reject repeated keys.
struct Strict;

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("closed JSON without floats or duplicate keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(Number::from(value)))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Value, E> {
        Err(E::custom(
            "floating-point numbers are not part of the closed contracts",
        ))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut result = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if result.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            let value = map.next_value_seed(Strict)?;
            result.insert(key, value);
        }
        Ok(Value::Object(result))
    }
}

/// Parse bounded, strict JSON.
///
/// # Errors
///
/// Fails when the input exceeds `maximum`, is not UTF-8 JSON, repeats an
/// object key, or contains a floating-point number or trailing data.
pub fn parse(raw: &[u8], label: &str, maximum: usize) -> Result<Value> {
    if raw.len() > maximum {
        return fail(format!("{label} exceeds the metadata size limit"));
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        return fail(format!("{label} is invalid JSON"));
    };
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = Strict
        .deserialize(&mut deserializer)
        .ok()
        .filter(|_| deserializer.end().is_ok());
    value.map_or_else(|| fail(format!("{label} is invalid JSON")), Ok)
}

/// Canonical text identical to Python's
/// `json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"))`.
#[must_use]
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

/// Canonical bytes followed by one newline, the on-disk form of every record.
#[must_use]
pub fn canonical_line(value: &Value) -> Vec<u8> {
    let mut text = canonical(value);
    text.push('\n');
    text.into_bytes()
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // Python sorts by code point; BTreeMap over `String` sorts by UTF-8
            // bytes, which is the same order.
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(item, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    use std::fmt::Write;
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// Require an object with exactly `keys`.
///
/// # Errors
///
/// Fails for a non-object or any missing or extra key.
pub fn exact<'a>(value: &'a Value, keys: &[&str], label: &str) -> Result<&'a Map<String, Value>> {
    match value {
        Value::Object(map)
            if map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)) =>
        {
            Ok(map)
        }
        _ => fail(format!("{label} has an unexpected field set")),
    }
}

/// A JSON integer in `1..2^64` (Python `START.positive`).
///
/// # Errors
///
/// Fails for anything that is not an exact positive integer.
pub fn positive(value: &Value, label: &str) -> Result<u64> {
    match value.as_u64() {
        Some(number) if number > 0 && value.is_u64() => Ok(number),
        _ => fail(format!("{label} is not a positive u64 integer")),
    }
}

/// A JSON integer in `1..=2^53-1` (Python `ORIGIN.positive`).
///
/// # Errors
///
/// Fails for anything outside the exact provider-identity range.
pub fn provider_id(value: &Value, label: &str) -> Result<u64> {
    let number = positive(value, label)?;
    if number > 9_007_199_254_740_991 {
        return fail(format!("{label} exceeds the exact integer limit"));
    }
    Ok(number)
}

/// Lowercase hexadecimal encoding.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Lowercase hexadecimal of exactly `length` characters.
#[must_use]
pub fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Require a canonical lowercase SHA-256 string.
///
/// # Errors
///
/// Fails for any other value.
pub fn digest<'a>(value: &'a Value, label: &str) -> Result<&'a str> {
    match value.as_str() {
        Some(text) if is_hex(text, 64) => Ok(text),
        _ => fail(format!("{label} is not a canonical SHA-256 digest")),
    }
}

/// Require a string field of an object.
///
/// # Errors
///
/// Fails when the field is absent or not a string.
pub fn string<'a>(map: &'a Map<String, Value>, key: &str, label: &str) -> Result<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .map_or_else(|| fail(format!("{label} {key} is not a string")), Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicates_floats_trailing_data_and_oversize() {
        assert!(parse(br#"{"a":1,"a":2}"#, "x", 64).is_err());
        assert!(parse(br#"{"a":{"b":1,"b":1}}"#, "x", 64).is_err());
        assert!(parse(b"1.5", "x", 64).is_err());
        assert!(parse(b"1e3", "x", 64).is_err());
        assert!(parse(b"NaN", "x", 64).is_err());
        assert!(parse(b"{} {}", "x", 64).is_err());
        assert!(parse(b"{}", "x", 1).is_err());
        assert!(parse(&[0xff], "x", 64).is_err());
        assert!(parse(br#"{"a":[1,"b",null,true]}"#, "x", 64).is_ok());
    }

    #[test]
    fn canonical_matches_the_python_encoding() {
        let value = parse(
            "{\"b\":[1,\"é\\u00e9\",\"\u{1F600}\"],\"a\":\"q\\\"\\n\",\"c\":null}".as_bytes(),
            "x",
            256,
        )
        .unwrap();
        assert_eq!(
            canonical(&value),
            r#"{"a":"q\"\n","b":[1,"\u00e9\u00e9","\ud83d\ude00"],"c":null}"#
        );
    }

    #[test]
    fn integer_helpers_are_exact() {
        assert!(positive(&Value::from(0), "x").is_err());
        assert!(positive(&Value::from(-1), "x").is_err());
        assert!(positive(&Value::Bool(true), "x").is_err());
        assert!(positive(&Value::from("1"), "x").is_err());
        assert!(provider_id(&Value::from(9_007_199_254_740_992_u64), "x").is_err());
        assert_eq!(
            provider_id(&Value::from(9_007_199_254_740_991_u64), "x").unwrap(),
            9_007_199_254_740_991
        );
    }
}
