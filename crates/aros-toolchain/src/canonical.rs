//! Recipe/receipt digest encoding, deliberately independent of archive formats.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::ContractError;

/// Maximum JSON input/output size for the current small producer contracts.
/// This is a parser resource bound, not a compiler resource estimate.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 64;

/// Encode sorted keys, unescaped UTF-8, unsigned integers and one final LF.
///
/// The caller must reject duplicate keys while parsing the original input:
/// converting an untrusted document to `Value` first would lose duplicates.
/// Arrays retain order. This is not an archive or payload-inventory algorithm.
///
/// # Errors
///
/// Rejects floats, negative numbers, excessive nesting and oversized output.
pub fn bytes(value: &Value) -> Result<Vec<u8>, ContractError> {
    let ordered = ordered(value, 0)?;
    let mut result = serde_json::to_vec(&ordered)
        .map_err(|_| ContractError::invalid("cannot serialize canonical producer JSON"))?;
    if result.len() >= MAX_DOCUMENT_BYTES {
        return Err(ContractError::invalid(
            "canonical producer JSON exceeds 1 MiB",
        ));
    }
    result.push(b'\n');
    Ok(result)
}

fn ordered(value: &Value, depth: usize) -> Result<Value, ContractError> {
    if depth > MAX_DEPTH {
        return Err(ContractError::invalid(
            "producer JSON nesting exceeds 64 levels",
        ));
    }
    match value {
        Value::Object(object) => {
            let sorted: BTreeMap<_, _> = object.iter().collect();
            let members = sorted
                .into_iter()
                .map(|(key, child)| Ok((key.clone(), ordered(child, depth + 1)?)))
                .collect::<Result<serde_json::Map<_, _>, ContractError>>()?;
            Ok(Value::Object(members))
        }
        Value::Array(array) => array
            .iter()
            .map(|child| ordered(child, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Number(number) if number.as_u64().is_none() => Err(ContractError::invalid(
            "producer JSON numbers must be unsigned 64-bit integers",
        )),
        _ => Ok(value.clone()),
    }
}
