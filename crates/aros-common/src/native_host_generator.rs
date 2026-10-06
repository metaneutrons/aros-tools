//! Source-owned, sealed inputs for one host-C generated build file.
//!
//! This is metadata, not a shell executor. The transpiler must separately
//! prove the exported invocation against the inventoried Make recipes.

use crate::Sha256Digest;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHostFileGenerator {
    pub owner: String,
    pub recipe: String,
    pub tool_recipe: String,
    pub tool_source: String,
    pub tool_variable: String,
    /// Canonical build-relative output, beginning with `gen/`.
    pub output: String,
    pub input_directory: String,
    pub compile_flags: Vec<String>,
    /// Literal tokens and the two directory placeholders, never shell text.
    pub arguments: Vec<String>,
    pub inputs: Vec<NativeHostFileInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeHostFileInput {
    pub filename: String,
    pub url: String,
    pub sha256: Sha256Digest,
    pub size: u64,
}

/// Validate portable metadata and require every source recipe to be sealed.
///
/// # Errors
/// Returns the first unsafe, ambiguous or unbound declaration.
pub fn validate_generators(
    generators: &[NativeHostFileGenerator],
    source_inputs: &BTreeSet<&str>,
) -> Result<(), String> {
    if generators.len() > 16 {
        return Err("host_file_generators exceeds 16 entries".into());
    }
    if !generators.is_empty()
        && ["Makefile.in", "configure.in"]
            .iter()
            .any(|path| !source_inputs.contains(path))
    {
        return Err(
            "host generators require inventoried Makefile.in and configure.in tool bindings".into(),
        );
    }
    let mut owners = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let mut cache_inputs = BTreeMap::new();
    for generator in generators {
        if !token(&generator.owner)
            || generator.owner.len() > 64
            || !generator.owner.as_bytes()[0].is_ascii_alphanumeric()
            || !owners.insert(&generator.owner)
        {
            return Err("host file generator owner is unsafe or duplicated".into());
        }
        for path in [
            &generator.recipe,
            &generator.tool_recipe,
            &generator.tool_source,
        ] {
            if !relative(path) || !source_inputs.contains(path.as_str()) {
                return Err(format!(
                    "{}: generator source {path:?} is not inventoried",
                    generator.owner
                ));
            }
        }
        if std::path::Path::new(&generator.tool_source)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            != Some("c")
            || generator.tool_variable.is_empty()
            || !generator
                .tool_variable
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            || !generator.tool_variable.as_bytes()[0].is_ascii_uppercase()
        {
            return Err("host generator requires one C source and uppercase tool variable".into());
        }
        if !relative(&generator.output)
            || !generator.output.starts_with("gen/")
            || !outputs.insert(generator.output.to_ascii_lowercase())
            || !relative(&generator.input_directory)
            || !generator.input_directory.starts_with("gen/")
        {
            return Err("host generator output/input directory is unsafe or duplicated".into());
        }
        if generator.compile_flags.len() > 16
            || generator
                .compile_flags
                .iter()
                .any(|flag| !compile_flag(flag))
        {
            return Err("host generator compile flags exceed the closed host-C contract".into());
        }
        if !(2..=16).contains(&generator.arguments.len())
            || generator
                .arguments
                .iter()
                .filter(|arg| arg.as_str() == "@INPUT_DIRECTORY@")
                .count()
                != 1
            || generator
                .arguments
                .iter()
                .filter(|arg| arg.as_str() == "@OUTPUT_DIRECTORY@")
                .count()
                != 1
            || generator
                .arguments
                .iter()
                .any(|arg| arg != "@INPUT_DIRECTORY@" && arg != "@OUTPUT_DIRECTORY@" && !token(arg))
        {
            return Err("host generator arguments must contain both directory placeholders exactly once and safe literal tokens".into());
        }
        if !(1..=16).contains(&generator.inputs.len()) {
            return Err("host generator requires 1..16 sealed inputs".into());
        }
        let mut local_names = BTreeSet::new();
        for input in &generator.inputs {
            if !filename(&input.filename)
                || !local_names.insert(input.filename.to_ascii_lowercase())
                || input.size == 0
                || input.size > 32 * 1024 * 1024
            {
                return Err("host generator input name/size is unsafe or duplicated".into());
            }
            let url =
                url::Url::parse(&input.url).map_err(|_| "invalid host generator input URL")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path_segments().is_some_and(|mut segments| {
                    segments.any(|part| part.eq_ignore_ascii_case("latest"))
                })
                || url
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    != Some(input.filename.as_str())
            {
                return Err("host generator input needs one immutable HTTPS file URL without credentials/query/fragment/latest".into());
            }
            if let Some(previous) = cache_inputs.insert(input.filename.to_ascii_lowercase(), input)
            {
                if previous != input {
                    return Err("host generators disagree on a shared cache filename".into());
                }
            }
        }
    }
    Ok(())
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

fn relative(value: &str) -> bool {
    value.len() <= 4096 && value.split('/').all(token)
}

fn filename(value: &str) -> bool {
    if !token(value)
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || value.contains("..")
        || value.ends_with('.')
    {
        return false;
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        && !["com", "lpt"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        })
}

fn compile_flag(value: &str) -> bool {
    matches!(
        value,
        "-g" | "-O0"
            | "-O1"
            | "-O2"
            | "-O3"
            | "-Os"
            | "-std=c99"
            | "-std=c11"
            | "-std=c17"
            | "-std=c23"
    ) || value.strip_prefix("-W").is_some_and(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> NativeHostFileGenerator {
        NativeHostFileGenerator {
            owner: "fixture-generated".into(),
            recipe: "fixture/mmakefile.src".into(),
            tool_recipe: "tools/fixture/Makefile".into(),
            tool_source: "tools/fixture/tool.c".into(),
            tool_variable: "GENTOOL".into(),
            output: "gen/fixture/output/default.c".into(),
            input_directory: "gen/data".into(),
            compile_flags: vec!["-g".into(), "-Wall".into(), "-O2".into()],
            arguments: vec![
                "@INPUT_DIRECTORY@".into(),
                "@OUTPUT_DIRECTORY@".into(),
                "default".into(),
                "--emit-c".into(),
            ],
            inputs: vec![NativeHostFileInput {
                filename: "data.txt".into(),
                url: "https://example.invalid/16.0.0/data.txt".into(),
                size: 16,
                sha256: crate::sha256_bytes(b"fixture"),
            }],
        }
    }
    fn inventory() -> BTreeSet<&'static str> {
        [
            "fixture/mmakefile.src",
            "tools/fixture/Makefile",
            "tools/fixture/tool.c",
            "Makefile.in",
            "configure.in",
        ]
        .into_iter()
        .collect()
    }
    #[test]
    fn metadata_requires_sealed_recipes_and_unique_safe_outputs() {
        let declared = fixture();
        validate_generators(std::slice::from_ref(&declared), &inventory()).unwrap();
        assert!(validate_generators(std::slice::from_ref(&declared), &BTreeSet::new()).is_err());
        let mut unbound = inventory();
        unbound.remove("configure.in");
        assert!(validate_generators(std::slice::from_ref(&declared), &unbound).is_err());
        assert!(validate_generators(&[declared.clone(), declared.clone()], &inventory()).is_err());
        for output in [
            "/gen/out.c",
            "gen/../escape.c",
            "gen//out.c",
            "out.c",
            "gen/out.c;evil",
        ] {
            let mut changed = declared.clone();
            changed.output = output.into();
            assert!(
                validate_generators(&[changed], &inventory()).is_err(),
                "{output}"
            );
        }
    }
    #[test]
    fn metadata_refuses_unbounded_commands_and_unpinned_input_routes() {
        let declared = fixture();
        for argument in ["../escape", "${EXECUTE_PROCESS}", ";touch", "@UNKNOWN@"] {
            let mut changed = declared.clone();
            changed.arguments.push(argument.into());
            assert!(
                validate_generators(&[changed], &inventory()).is_err(),
                "{argument}"
            );
        }
        for flag in ["-o", "-include", "@response", "-Wl,-rpath,/host"] {
            let mut changed = declared.clone();
            changed.compile_flags.push(flag.into());
            assert!(
                validate_generators(&[changed], &inventory()).is_err(),
                "{flag}"
            );
        }
        for url in [
            "http://example.invalid/1/data.txt",
            "https://user@example.invalid/1/data.txt",
            "https://example.invalid/latest/data.txt",
            "https://example.invalid/1/data.txt?x=y",
            "https://example.invalid/1/other.txt",
        ] {
            let mut changed = declared.clone();
            changed.inputs[0].url = url.into();
            assert!(
                validate_generators(&[changed], &inventory()).is_err(),
                "{url}"
            );
        }
        let mut changed = declared.clone();
        changed.inputs.push(changed.inputs[0].clone());
        assert!(validate_generators(&[changed], &inventory()).is_err());
        let mut changed = declared;
        changed.inputs[0].size = 0;
        assert!(validate_generators(&[changed], &inventory()).is_err());
    }

    #[test]
    fn input_names_are_portable_without_case_folded_collisions() {
        for name in [
            "-data.txt",
            ".hidden",
            "bad..txt",
            "trailing.",
            "CON.txt",
            "Lpt9.txt",
        ] {
            assert!(!filename(name), "{name}");
        }
        let mut declared = fixture();
        let mut alias = declared.inputs[0].clone();
        alias.filename = "DATA.txt".into();
        alias.url = "https://example.invalid/16.0.0/DATA.txt".into();
        declared.inputs.push(alias);
        assert!(validate_generators(&[declared], &inventory()).is_err());
    }
}
