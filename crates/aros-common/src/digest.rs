//! Typed SHA-256 parsing and streaming helpers shared by AROS tools.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

const SHA256_BYTES: usize = 32;
const SHA256_HEX_LENGTH: usize = SHA256_BYTES * 2;
const HASH_BUFFER_BYTES: usize = 128 * 1024;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// A validated, normalized SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Parse hexadecimal SHA-256 text and normalize it to lower case.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSha256Digest`] unless `value` contains exactly 64
    /// hexadecimal characters.
    pub fn parse(value: &str) -> Result<Self, InvalidSha256Digest> {
        value.parse()
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        debug_assert_eq!(bytes.len(), SHA256_BYTES);
        let mut output = String::with_capacity(SHA256_HEX_LENGTH);
        for byte in bytes {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Self(output)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Sha256Digest {
    type Err = InvalidSha256Digest;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != SHA256_HEX_LENGTH || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(InvalidSha256Digest);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Error returned for text that is not exactly one SHA-256 digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSha256Digest;

impl fmt::Display for InvalidSha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected a 64-character hexadecimal SHA-256 digest")
    }
}

impl std::error::Error for InvalidSha256Digest {}

/// Digest and byte count produced by one streaming hash operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256Result {
    pub digest: Sha256Digest,
    pub size: u64,
}

/// Hash in-memory bytes.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes))
}

/// Finish a SHA-256 hasher used by a caller that must hash while copying.
#[must_use]
pub fn finish_sha256(hasher: Sha256) -> Sha256Digest {
    Sha256Digest::from_bytes(hasher.finalize())
}

/// Hash a reader without taking ownership of it.
///
/// # Errors
///
/// Returns an I/O error when reading fails or the byte count overflows `u64`.
pub fn sha256_reader(reader: &mut impl Read) -> io::Result<Sha256Result> {
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(u64::try_from(read).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("input is too large to count while hashing"))?;
    }
    Ok(Sha256Result {
        digest: Sha256Digest::from_bytes(hasher.finalize()),
        size,
    })
}

/// Open and hash one regular file.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be opened or read.
pub fn sha256_file(path: &Path) -> io::Result<Sha256Result> {
    sha256_reader(&mut File::open(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_normalizes_and_rejects_non_digests() {
        let upper = "A".repeat(SHA256_HEX_LENGTH);
        assert_eq!(
            Sha256Digest::parse(&upper).unwrap().as_str(),
            "a".repeat(64)
        );
        assert!(Sha256Digest::parse("abc").is_err());
        assert!(Sha256Digest::parse(&"z".repeat(64)).is_err());
    }

    #[test]
    fn bytes_and_reader_are_identical() {
        let input = b"AROS-NG";
        let mut reader = &input[..];
        let streamed = sha256_reader(&mut reader).unwrap();
        assert_eq!(streamed.digest, sha256_bytes(input));
        assert_eq!(streamed.size, input.len() as u64);
    }

    #[test]
    fn serde_rejects_invalid_digests() {
        assert!(serde_json::from_str::<Sha256Digest>("\"invalid\"").is_err());
        let digest = Sha256Digest::parse(&"1".repeat(64)).unwrap();
        assert_eq!(
            serde_json::to_string(&digest).unwrap(),
            format!("\"{digest}\"")
        );
    }
}
