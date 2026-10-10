//! Parsing and validation of the legacy-compatible fetch command contract.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticStage, Sha256Digest};
use clap::{Arg, ArgMatches, Command, CommandFactory, Parser};

use crate::engine::cache::CachePayloadNormalization;
use crate::{FetchFailure, FetchResult};

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "Fetch, verify, extract, and patch AROS third-party sources",
    long_about = "Fetch one declared AROS source archive into a local cache, verify every declared SHA-256 digest before use, extract without path escape, and apply a closed patch contract without shell evaluation.",
    after_help = "CHECKSUM CONTRACT:\n  --checksums accepts whitespace-separated filename=sha256:<64-hex-digest> entries.\n  --require-checksums requires complete archive coverage and remote patch coverage.\n  A downloaded payload with another digest, or one that is not the declared\n  archive format, rejects that source and the next declared one is tried; the\n  fetch fails if none yields it. aros-fetch never rewrites or infers a pin.\n\nARCHIVE REPRESENTATION:\n  --normalization exact-bytes-v1 is the default and preserves received bytes.\n  canonical-tar-gzip-v1 requires exactly one .tar.gz or .tgz archive candidate,\n  its explicit SHA-256, and --normalized-size equal to the final cached bytes.\n  HTTPS responses are normalized; local origins and existing cache entries must\n  already be the declared canonical bytes. --offline uses only such verified\n  local/cache bytes and never fetches a missing payload.\n\nPATCH CONTRACT:\n  --patches accepts whitespace-separated name[:subdirectory[:option,...]] entries.\n  Supported options are -p0 through -p9, -f, -N, and --forward.\n\nOBSERVABILITY:\n  Diagnostics are written to stderr; --diagnostic-format=json selects the stable JSON contract.\n  Logging is off by default. A selected --log-file without a selected level uses info;\n  explicit --log-level off creates no sink. A non-off level requires --log-file.\n  --log-format selects human or jsonl.\n  Environment: AROS_FETCH_DIAGNOSTIC_FORMAT, AROS_FETCH_LOG_LEVEL,\n  AROS_FETCH_LOG_FORMAT, AROS_FETCH_LOG_FILE, AROS_FETCH_OFFLINE,\n  AROS_FETCH_REQUIRE_CHECKSUMS."
)]
pub struct Cli {
    /// Whitespace-separated local paths or supported remote origin prefixes.
    #[arg(long = "archive-origins", default_value = ".")]
    pub archive_origins: String,

    /// Exact portable archive basename without a suffix.
    #[arg(long)]
    pub archive: String,

    /// Whitespace-separated candidate suffixes, tried in declaration order.
    #[arg(long, default_value = "")]
    pub suffixes: String,

    /// Directory below which the verified archive is extracted.
    #[arg(long, default_value = ".")]
    pub destination: PathBuf,

    /// Whitespace-separated origins used only for declared patch payloads.
    #[arg(long = "patch-origins", default_value = ".")]
    pub patch_origins: String,

    /// Closed patch declarations: name[:subdirectory[:option,...]].
    #[arg(long, default_value = "")]
    pub patches: String,

    /// Containment base for patch application; defaults to destination.
    #[arg(long)]
    pub base: Option<PathBuf>,

    /// Local verified-payload cache directory.
    #[arg(long, default_value = ".")]
    pub location: PathBuf,

    /// Historical compatibility option; nonempty values are rejected safely.
    #[arg(long = "rename-directory")]
    pub rename_directory: Option<String>,

    /// Whitespace-separated `filename=sha256:<digest>` declarations.
    #[arg(long, default_value = "")]
    pub checksums: String,

    /// Replace an existing extraction only through the validated publication path.
    #[arg(long)]
    pub force: bool,

    /// Use only already verified local cache payloads; never access the network.
    #[arg(
        long,
        env = "AROS_FETCH_OFFLINE",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub offline: bool,

    /// Reject incomplete archive or remote-patch checksum coverage.
    #[arg(
        long = "require-checksums",
        env = "AROS_FETCH_REQUIRE_CHECKSUMS",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    pub require_checksums: bool,

    /// Failure renderer: human text or one stable JSON diagnostic document.
    #[arg(long, value_enum, default_value_t = aros_common::DiagnosticFormat::Human, env = "AROS_FETCH_DIAGNOSTIC_FORMAT")]
    pub diagnostic_format: aros_common::DiagnosticFormat,

    /// Minimum level for the explicit local log; non-off requires --log-file.
    #[arg(long, value_enum, default_value_t = aros_common::LogLevel::Off, env = "AROS_FETCH_LOG_LEVEL")]
    pub log_level: aros_common::LogLevel,

    /// Explicit local log encoding.
    #[arg(long, value_enum, default_value_t = aros_common::LogFormat::Human, env = "AROS_FETCH_LOG_FORMAT")]
    pub log_format: aros_common::LogFormat,

    /// Explicit local log file; no log is written while --log-level is off.
    #[arg(long, env = "AROS_FETCH_LOG_FILE")]
    pub log_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSpec {
    pub name: String,
    pub subdirectory: Option<PathBuf>,
    pub options: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub archive: String,
    pub archive_candidates: Vec<String>,
    pub archive_origins: Vec<String>,
    pub destination: PathBuf,
    pub location: PathBuf,
    pub base: PathBuf,
    pub patches: Vec<PatchSpec>,
    pub patch_origins: Vec<String>,
    pub checksums: BTreeMap<String, Sha256Digest>,
    pub force: bool,
    pub offline: bool,
}

/// Additional archive representation policy accepted by the ordinary CLI.
///
/// Kept separate from [`Cli`] and [`FetchRequest`] so existing Rust callers
/// that construct those public structs retain source compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchivePayloadOptions {
    pub normalization: CachePayloadNormalization,
    pub normalized_size: Option<u64>,
}

impl Default for ArchivePayloadOptions {
    fn default() -> Self {
        Self {
            normalization: CachePayloadNormalization::ExactBytesV1,
            normalized_size: None,
        }
    }
}

/// Build the CLI command with the archive representation options while
/// retaining the source-compatible public [`Cli`] struct.
#[must_use]
pub fn command_with_payload_options() -> Command {
    Cli::command()
        .arg(
            Arg::new("normalization")
                .long("normalization")
                .value_name("POLICY")
                .value_parser(clap::builder::EnumValueParser::<CachePayloadNormalization>::new())
                .default_value("exact-bytes-v1")
                .help("Archive representation policy (default: exact-bytes-v1)"),
        )
        .arg(
            Arg::new("normalized_size")
                .long("normalized-size")
                .value_name("NONZERO_BYTES")
                .value_parser(clap::value_parser!(u64))
                .help("Expected final byte size for canonical-tar-gzip-v1"),
        )
}

/// Read the two archive representation options from the extended CLI command.
#[must_use]
pub fn archive_payload_options(matches: &ArgMatches) -> ArchivePayloadOptions {
    ArchivePayloadOptions {
        normalization: matches
            .get_one::<CachePayloadNormalization>("normalization")
            .copied()
            .unwrap_or_default(),
        normalized_size: matches.get_one::<u64>("normalized_size").copied(),
    }
}

impl FetchRequest {
    /// Validate command-line values and close them into a typed request.
    ///
    /// # Errors
    ///
    /// Returns a stable contract diagnostic for malformed names, paths,
    /// patch options, origins, or incomplete checksum declarations.
    pub fn from_cli(cli: &Cli) -> FetchResult<Self> {
        validate_basename(&cli.archive, "archive")?;
        let suffixes = words(&cli.suffixes);
        for suffix in &suffixes {
            validate_suffix(suffix)?;
        }
        let archive_candidates = if suffixes.is_empty() {
            vec![cli.archive.clone()]
        } else {
            suffixes
                .iter()
                .map(|suffix| format!("{}.{suffix}", cli.archive))
                .collect()
        };
        let archive_origins = nonempty_words(&cli.archive_origins, "archive origins")?;
        let patch_origins = nonempty_words(&cli.patch_origins, "patch origins")?;
        let patches = words(&cli.patches)
            .into_iter()
            // Upstream fetch.sh treats the exact triple-field sentinel `::`
            // as an empty patch name and therefore a no-op.  The transpiler
            // emits that canonical value when an optional Make patch variable
            // expands to empty.  No other malformed empty-name form is
            // accepted by the native contract.
            .filter(|value| value != "::")
            .map(|value| parse_patch(&value))
            .collect::<FetchResult<Vec<_>>>()?;
        if let Some(rename) = &cli.rename_directory {
            validate_basename(rename, "renamed directory")?;
            return Err(contract_failure(format!(
                "rename-directory '{rename}' is not supported because the upstream fetch.sh option has no defined behavior"
            ))
            .with_hint("remove -rn; if a real package needs renaming, define and test that operation explicitly"));
        }
        let checksums = parse_checksums(&cli.checksums)?;
        let mut known: BTreeSet<String> = archive_candidates.iter().cloned().collect();
        for patch in &patches {
            known.insert(patch.name.clone());
            for suffix in ["tar.bz2", "tar.gz", "zip"] {
                known.insert(format!("{}.{suffix}", patch.name));
            }
        }
        for name in checksums.keys() {
            if !known.contains(name) {
                return Err(contract_failure(format!(
                    "checksum payload '{name}' is not a declared candidate for archive '{}'",
                    cli.archive
                )));
            }
        }
        if !checksums.is_empty() || cli.require_checksums {
            for candidate in &archive_candidates {
                if !checksums.contains_key(candidate) {
                    return Err(contract_failure(format!(
                        "checksum contract does not cover archive candidate '{candidate}'"
                    ))
                    .with_hint("declare every candidate as filename=sha256:<digest>; aros-fetch never infers or generates pins"));
                }
            }
        }
        if cli.require_checksums && patch_origins.iter().any(|origin| is_remote(origin)) {
            for patch in &patches {
                let covered = checksums.contains_key(&patch.name)
                    || ["tar.bz2", "tar.gz", "zip"]
                        .iter()
                        .any(|suffix| checksums.contains_key(&format!("{}.{suffix}", patch.name)));
                if !covered {
                    return Err(contract_failure(format!(
                        "strict checksum mode has no payload checksum for remote patch '{}'",
                        patch.name
                    ))
                    .with_hint("declare at least one exact patch payload checksum or use only local patch origins"));
                }
            }
        }
        Ok(Self {
            archive: cli.archive.clone(),
            archive_candidates,
            archive_origins,
            destination: absolute(&cli.destination)?,
            location: absolute(&cli.location)?,
            base: absolute(cli.base.as_deref().unwrap_or(&cli.destination))?,
            patches,
            patch_origins,
            checksums,
            force: cli.force,
            offline: cli.offline,
        })
    }

    /// Validate the explicit representation options against this archive
    /// selection without changing the exact-byte legacy default.
    ///
    /// # Errors
    ///
    /// Returns a contract diagnostic when the selected policy has an invalid
    /// size, candidate set, checksum, or origin declaration.
    pub fn validate_archive_payload_options(
        &self,
        options: ArchivePayloadOptions,
    ) -> FetchResult<()> {
        match options.normalization {
            CachePayloadNormalization::ExactBytesV1 => {
                if options.normalized_size.is_some() {
                    return Err(contract_failure(
                        "--normalized-size is valid only with --normalization canonical-tar-gzip-v1",
                    ));
                }
            }
            CachePayloadNormalization::CanonicalTarGzipV1 => {
                let Some(size) = options.normalized_size else {
                    return Err(contract_failure(
                        "canonical-tar-gzip-v1 requires --normalized-size for the final cached bytes",
                    ));
                };
                if size == 0 {
                    return Err(contract_failure(
                        "--normalized-size must be a nonzero byte count",
                    ));
                }
                if self.archive_candidates.len() != 1 {
                    return Err(contract_failure(
                        "canonical-tar-gzip-v1 requires exactly one archive candidate",
                    ));
                }
                let candidate = &self.archive_candidates[0];
                let is_tar_gzip = [".tar.gz", ".tgz"].iter().any(|suffix| {
                    candidate.len() >= suffix.len()
                        && candidate[candidate.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
                });
                if !is_tar_gzip {
                    return Err(contract_failure(
                        "canonical-tar-gzip-v1 requires one .tar.gz or .tgz archive candidate",
                    ));
                }
                if !self.checksums.contains_key(candidate) {
                    return Err(contract_failure(format!(
                        "canonical-tar-gzip-v1 requires an explicit SHA-256 for '{candidate}'"
                    )));
                }
                for origin in &self.archive_origins {
                    validate_canonical_archive_origin(origin)?;
                }
            }
        }
        Ok(())
    }
}

fn validate_canonical_archive_origin(origin: &str) -> FetchResult<()> {
    if !origin.contains("://") {
        return Ok(());
    }
    if origin.starts_with("https://") {
        let parsed = reqwest::Url::parse(origin).map_err(|error| {
            contract_failure(format!("invalid HTTPS archive origin '{origin}': {error}"))
        })?;
        if parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
        {
            return Ok(());
        }
        return Err(contract_failure(
            "canonical archive HTTPS origins must be credential-free and omit query or fragment",
        ));
    }
    for prefix in [
        "cache://",
        "gnu://",
        "archives://",
        "sf://",
        "sourceforge://",
        "github://",
    ] {
        if let Some(path) = origin.strip_prefix(prefix) {
            if path.contains(['\\', '?', '#', '%'])
                || path
                    .split('/')
                    .any(|component| matches!(component, "." | ".."))
            {
                return Err(contract_failure(format!(
                    "HTTPS archive alias '{origin}' contains an unsafe or ambiguous path"
                )));
            }
            return Ok(());
        }
    }
    Err(contract_failure(format!(
        "canonical-tar-gzip-v1 accepts only HTTPS origins or local payload paths; unsupported origin '{origin}'"
    )))
}

/// Translate the historical multi-character options accepted by fetch.sh.
#[must_use]
pub fn normalize_legacy_arguments(arguments: Vec<OsString>) -> Vec<OsString> {
    arguments
        .into_iter()
        .map(|argument| match argument.to_str() {
            Some("-ao") => OsString::from("--archive-origins"),
            Some("-a") => OsString::from("--archive"),
            Some("-s") => OsString::from("--suffixes"),
            Some("-d") => OsString::from("--destination"),
            Some("-po") => OsString::from("--patch-origins"),
            Some("-p") => OsString::from("--patches"),
            Some("-b") => OsString::from("--base"),
            Some("-l") => OsString::from("--location"),
            Some("-rn") => OsString::from("--rename-directory"),
            Some("-cs") => OsString::from("--checksums"),
            Some("-f") => OsString::from("--force"),
            _ => argument,
        })
        .collect()
}

fn parse_patch(value: &str) -> FetchResult<PatchSpec> {
    let mut fields = value.splitn(3, ':');
    let name = fields.next().unwrap_or_default();
    validate_basename(name, "patch")?;
    let subdirectory = fields
        .next()
        .filter(|field| !field.is_empty())
        .map(PathBuf::from);
    if let Some(path) = &subdirectory {
        validate_relative(path, "patch subdirectory")?;
    }
    let options = fields
        .next()
        .filter(|field| !field.is_empty())
        .map_or_else(Vec::new, |field| {
            field.split(',').map(str::to_owned).collect()
        });
    for option in &options {
        let allowed = matches!(option.as_str(), "-f" | "-N" | "--forward")
            || option.strip_prefix("-p").is_some_and(|level| {
                level.len() == 1 && level.bytes().all(|byte| byte.is_ascii_digit())
            });
        if !allowed {
            return Err(contract_failure(format!(
                "unsupported patch option '{option}' in '{value}'"
            ))
            .with_hint("supported options are -p0 through -p9, -f, -N, and --forward"));
        }
    }
    Ok(PatchSpec {
        name: name.to_owned(),
        subdirectory,
        options,
    })
}

fn parse_checksums(value: &str) -> FetchResult<BTreeMap<String, Sha256Digest>> {
    let mut result = BTreeMap::new();
    for declaration in words(value) {
        let Some((name, digest)) = declaration.split_once("=sha256:") else {
            return Err(
                contract_failure(format!("invalid checksum declaration '{declaration}'"))
                    .with_hint("expected filename=sha256:<64 hexadecimal digits>"),
            );
        };
        validate_basename(name, "checksum payload")?;
        let digest = Sha256Digest::parse(digest)
            .map_err(|error| contract_failure(format!("invalid SHA-256 for '{name}': {error}")))?;
        if result.insert(name.to_owned(), digest).is_some() {
            return Err(contract_failure(format!(
                "duplicate checksum declaration for '{name}'"
            )));
        }
    }
    Ok(result)
}

fn validate_basename(value: &str, role: &str) -> FetchResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+~-".contains(&byte))
    {
        return Err(contract_failure(format!(
            "invalid {role} name '{value}'; an exact portable basename is required"
        )));
    }
    Ok(())
}

fn validate_suffix(value: &str) -> FetchResult<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    {
        return Err(contract_failure(format!(
            "invalid archive suffix '{value}'"
        )));
    }
    Ok(())
}

pub(crate) fn validate_relative(path: &Path, role: &str) -> FetchResult<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(contract_failure(format!(
            "invalid {role} '{}'; only a relative contained path is allowed",
            path.display()
        )));
    }
    Ok(())
}

fn absolute(path: &Path) -> FetchResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|error| contract_failure(format!("cannot resolve current directory: {error}")))
}

fn words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

fn nonempty_words(value: &str, role: &str) -> FetchResult<Vec<String>> {
    let values = words(value);
    if values.is_empty() {
        Err(contract_failure(format!("{role} must not be empty")))
    } else {
        Ok(values)
    }
}

fn is_remote(origin: &str) -> bool {
    [
        "http://",
        "https://",
        "ftp://",
        "archives://",
        "gnu://",
        "sf://",
        "sourceforge://",
        "github://",
        "cache://",
    ]
    .iter()
    .any(|prefix| origin.starts_with(prefix))
}

fn contract_failure(message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(Diagnostic::error(
        DiagnosticCode::FetchContract,
        DiagnosticStage::FetchContract,
        message,
    ))
}

trait FailureHint {
    fn with_hint(self, hint: impl Into<String>) -> Self;
}

impl FailureHint for FetchFailure {
    fn with_hint(self, hint: impl Into<String>) -> Self {
        Self::new(self.into_diagnostic().with_hint(hint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_request(
        archive: &str,
        suffixes: &str,
        origin: &str,
        checksums: &str,
    ) -> FetchRequest {
        let cli = Cli::try_parse_from([
            "aros-fetch",
            "--archive",
            archive,
            "--suffixes",
            suffixes,
            "--archive-origins",
            origin,
            "--checksums",
            checksums,
        ])
        .unwrap();
        FetchRequest::from_cli(&cli).unwrap()
    }

    fn digest() -> String {
        "a".repeat(64)
    }

    fn cli() -> Cli {
        Cli::try_parse_from([
            "aros-fetch",
            "--archive",
            "pkg-1",
            "--suffixes",
            "tar.gz tar.xz",
        ])
        .unwrap()
    }

    #[test]
    fn legacy_arguments_are_translated_without_shell_parsing() {
        let normalized = normalize_legacy_arguments(
            ["aros-fetch", "-ao", "cache://", "-a", "pkg", "-f"]
                .into_iter()
                .map(OsString::from)
                .collect(),
        );
        assert_eq!(normalized[1], "--archive-origins");
        assert_eq!(normalized[3], "--archive");
        assert_eq!(normalized[5], "--force");
    }

    #[test]
    fn checksum_contract_must_cover_every_candidate() {
        let mut value = cli();
        value.checksums = format!("pkg-1.tar.gz=sha256:{}", "1".repeat(64));
        assert!(FetchRequest::from_cli(&value).is_err());
    }

    #[test]
    fn patch_contract_rejects_shell_options_and_parent_paths() {
        assert!(parse_patch("fix.diff:../escape:-p1").is_err());
        assert!(parse_patch("fix.diff:src:--output=owned").is_err());
        assert!(parse_patch("fix.diff:src:-f,-p1").is_ok());
    }

    #[test]
    fn exact_legacy_empty_patch_sentinel_is_a_noop() {
        let mut value = cli();
        value.patches = ":: fix.diff:src:-f,-p1".to_owned();
        let request = FetchRequest::from_cli(&value).unwrap();
        assert_eq!(request.patches.len(), 1);
        assert_eq!(request.patches[0].name, "fix.diff");

        value.patches = ":src:-p1".to_owned();
        assert!(FetchRequest::from_cli(&value).is_err());
    }

    #[test]
    fn archive_normalization_options_have_explicit_compatibility_default() {
        let matches = command_with_payload_options()
            .try_get_matches_from(["aros-fetch", "--archive", "pkg.tar.gz"])
            .unwrap();
        assert_eq!(
            archive_payload_options(&matches),
            ArchivePayloadOptions::default()
        );

        let matches = command_with_payload_options()
            .try_get_matches_from([
                "aros-fetch",
                "--archive",
                "pkg.tar.gz",
                "--normalization",
                "canonical-tar-gzip-v1",
                "--normalized-size",
                "42",
            ])
            .unwrap();
        assert_eq!(
            archive_payload_options(&matches),
            ArchivePayloadOptions {
                normalization: CachePayloadNormalization::CanonicalTarGzipV1,
                normalized_size: Some(42),
            }
        );
        assert!(command_with_payload_options()
            .try_get_matches_from([
                "aros-fetch",
                "--archive",
                "pkg.tar.gz",
                "--normalization",
                "future-policy",
            ])
            .is_err());
    }

    #[test]
    fn canonical_archive_policy_requires_a_single_pinned_tarball_and_final_size() {
        let options = ArchivePayloadOptions {
            normalization: CachePayloadNormalization::CanonicalTarGzipV1,
            normalized_size: Some(42),
        };
        let request = canonical_request(
            "pkg.tar.gz",
            "",
            "https://example.test/releases",
            &format!("pkg.tar.gz=sha256:{}", digest()),
        );
        assert!(request.validate_archive_payload_options(options).is_ok());
        assert!(request
            .validate_archive_payload_options(ArchivePayloadOptions {
                normalized_size: None,
                ..options
            })
            .is_err());
        assert!(request
            .validate_archive_payload_options(ArchivePayloadOptions {
                normalized_size: Some(0),
                ..options
            })
            .is_err());
        assert!(request
            .validate_archive_payload_options(ArchivePayloadOptions {
                normalization: CachePayloadNormalization::ExactBytesV1,
                normalized_size: Some(42),
            })
            .is_err());

        let multiple = canonical_request(
            "pkg",
            "tar.gz tgz",
            "https://example.test/releases",
            &format!("pkg.tar.gz=sha256:{} pkg.tgz=sha256:{}", digest(), digest()),
        );
        assert!(multiple.validate_archive_payload_options(options).is_err());
        let wrong_extension = canonical_request(
            "pkg",
            "zip",
            "https://example.test/releases",
            &format!("pkg.zip=sha256:{}", digest()),
        );
        assert!(wrong_extension
            .validate_archive_payload_options(options)
            .is_err());
        let unpinned = canonical_request("pkg.tar.gz", "", "https://example.test", "");
        assert!(unpinned.validate_archive_payload_options(options).is_err());
    }

    #[test]
    fn canonical_archive_policy_rejects_non_https_and_ambiguous_origins() {
        let options = ArchivePayloadOptions {
            normalization: CachePayloadNormalization::CanonicalTarGzipV1,
            normalized_size: Some(42),
        };
        for origin in [
            "http://example.test/releases",
            "ftp://example.test/releases",
            "https://user@example.test/releases",
            "https://example.test/releases?token=ambiguous",
            "github://owner/repo/../archive",
        ] {
            let request = canonical_request(
                "pkg.tar.gz",
                "",
                origin,
                &format!("pkg.tar.gz=sha256:{}", digest()),
            );
            assert!(
                request.validate_archive_payload_options(options).is_err(),
                "origin unexpectedly accepted: {origin}"
            );
        }
        for origin in ["https://example.test/releases", "cache://sources/pkg"] {
            let request = canonical_request(
                "pkg.tar.gz",
                "",
                origin,
                &format!("pkg.tar.gz=sha256:{}", digest()),
            );
            assert!(request.validate_archive_payload_options(options).is_ok());
        }
    }
}
