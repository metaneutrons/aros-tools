//! Embeds the CMake engine and pins its identity at compile time.
//!
//! The engine is data the binary carries, not a directory it hopes to find, so
//! it is walked here and turned into one sorted table of `include_str!` entries.
//! Sorting is what makes the digest reproducible: a directory walk has no
//! defined order, and an unstable digest would report a changed engine on every
//! rebuild.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let engine =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("engine");
    println!("cargo:rerun-if-changed={}", engine.display());

    let mut files = BTreeMap::new();
    collect(&engine, &engine, &mut files);
    assert!(
        !files.is_empty(),
        "{}: the engine directory holds no files",
        engine.display()
    );

    let mut hasher = Sha256::new();
    let mut table = String::from("pub(crate) static FILES: &[(&str, &str)] = &[\n");
    for (relative, absolute) in &files {
        println!("cargo:rerun-if-changed={}", absolute.display());
        let bytes =
            fs::read(absolute).unwrap_or_else(|error| panic!("{}: {error}", absolute.display()));
        // Path and length go into the digest as well as the content, so a
        // rename or a truncation cannot leave the digest unchanged.
        hasher.update(relative.as_bytes());
        hasher.update(bytes.len().to_le_bytes());
        hasher.update(&bytes);
        writeln!(
            table,
            "    ({relative:?}, include_str!({:?})),",
            absolute.display().to_string()
        )
        .expect("writing to a String cannot fail");
    }
    table.push_str("];\n");

    // sha2 hands back a plain array, so the hex form is spelled out here the
    // same way aros-common's Sha256Digest does it.
    let digest = hex(hasher.finalize().as_slice());

    // The API version lives in the engine, not here, so the two cannot drift.
    let version_file = engine.join("EngineVersion.cmake");
    let version_text = fs::read_to_string(&version_file)
        .unwrap_or_else(|error| panic!("{}: {error}", version_file.display()));
    let api_version = parse_api_version(&version_text).unwrap_or_else(|| {
        panic!(
            "{}: no `set(AROS_CMAKE_ENGINE_API_VERSION <n>)` line",
            version_file.display()
        )
    });

    let out = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(out.join("engine_files.rs"), table).expect("write engine table");
    fs::write(
        out.join("engine_digest.rs"),
        format!(
            "pub(crate) const DIGEST: &str = {digest:?};\n\
             pub(crate) const API_VERSION: u32 = {api_version};\n"
        ),
    )
    .expect("write engine digest");
}

/// Reads `set(AROS_CMAKE_ENGINE_API_VERSION <n>)` out of the engine's own file.
///
/// Deliberately narrow: only that exact statement is recognised, so a comment
/// mentioning the variable cannot be mistaken for the declaration.
fn parse_api_version(text: &str) -> Option<u32> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("set(AROS_CMAKE_ENGINE_API_VERSION") else {
            continue;
        };
        let value = rest.trim_start().strip_suffix(')')?.trim();
        return value.parse().ok();
    }
    None
}

/// Walks the engine directory into a path-sorted map.
///
/// Directories are recursed in whatever order the platform returns them; the
/// `BTreeMap` is what imposes the order the digest depends on.
fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<String, PathBuf>) {
    let entries =
        fs::read_dir(directory).unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("engine path inside the engine root")
            .to_str()
            .unwrap_or_else(|| panic!("{}: path is not UTF-8", path.display()))
            .replace('\\', "/");
        files.insert(relative, path);
    }
}

/// Lower-case hexadecimal, matching `aros_common::Sha256Digest`.
///
/// Spelled out rather than pulled from a dependency: a build script that hashes
/// its own inputs should not need one.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}
