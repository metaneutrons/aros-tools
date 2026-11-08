//! The only door to GitHub and the runner.
//!
//! Every observation the commands make goes through [`Provider`]. The live
//! implementation ([`GhProvider`]) is a fixed `gh` invocation against
//! `github.com` with the job token, exactly like the Python oracle: bounded
//! output, a deadline, no credential fallback, no retry. Tests substitute an
//! offline fixture provider.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::json::parse;
use super::{fail, integrity, require, Result, MAX_API_BYTES, REPOSITORY};

pub const API_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_API_STDERR: usize = 64 * 1024;
pub const MAX_HEADERS: usize = 64 * 1024;
const FALLBACK_CREDENTIALS: [&str; 4] = [
    "GITHUB_TOKEN",
    "GH_AUTH_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];
const OIDC_CREDENTIALS: [&str; 2] = [
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
];

/// One signature-verification request for a private subject copy.
#[derive(Debug, Clone)]
pub struct Attestation<'a> {
    pub subject: &'a Path,
    pub bundle: &'a Path,
    pub repository: &'a str,
    pub signer_workflow: &'a str,
    pub commit: &'a str,
}

/// Everything the dispatcher commands observe outside their own process.
pub trait Provider {
    /// Runner environment variable.
    fn var(&self, name: &str) -> Option<String>;
    /// One fixed-repository GET answered with HTTP 200 JSON.
    ///
    /// # Errors
    ///
    /// Any other status, an unbounded body or a transport failure rejects.
    fn get(&self, path: &str) -> Result<Value>;
    /// Raw bytes of a committed file of the fixed repository at a commit.
    ///
    /// # Errors
    ///
    /// Rejects on any failure or oversize answer.
    fn committed_file(&self, commit: &str, path: &str, limit: usize) -> Result<Vec<u8>>;
    /// The start artifact ZIP, bounded by the provider-reported size.
    ///
    /// # Errors
    ///
    /// Rejects on any failure; never retried.
    fn download_artifact(&self, artifact_id: u64, limit: usize) -> Result<Vec<u8>>;
    /// A release asset by numeric ID (`Accept: application/octet-stream`).
    ///
    /// # Errors
    ///
    /// Rejects on any failure or a status other than 200; never retried.
    fn download_asset(&self, asset_id: u64, limit: usize) -> Result<Vec<u8>>;
    /// Check ID of the executing job, read from the authenticated runner OIDC
    /// issuer (never from a caller-supplied token).
    ///
    /// # Errors
    ///
    /// Rejects when OIDC is unavailable or its claims differ.
    fn oidc_check_id(&self, context: &Value, expected_event: &str) -> Result<u64>;
    /// `gh attestation verify` of one private subject and bundle.
    ///
    /// # Errors
    ///
    /// Rejects when the verification fails or is unavailable.
    fn attest(&self, request: &Attestation<'_>) -> Result<Value>;
}

/// A fixed-repository API path; anything else is outside the boundary.
///
/// # Errors
///
/// Rejects other repositories and traversal.
pub fn require_fixed_path(path: &str) -> Result<()> {
    let inside =
        path == format!("repos/{REPOSITORY}") || path.starts_with(&format!("repos/{REPOSITORY}/"));
    require(
        inside && !path.split('/').any(|part| part == ".."),
        "provider endpoint is outside its fixed repository",
    )
}

/// Split an `--include` response into status and body (HTTP/1.x, 2, 3).
///
/// # Errors
///
/// Rejects a missing or malformed status line or header block.
pub fn included_response(raw: &[u8]) -> Result<(u16, Vec<u8>)> {
    require(
        raw.len() <= MAX_API_BYTES,
        "HTTP response exceeds the metadata limit",
    )?;
    let line_end = raw.iter().position(|b| *b == b'\n').map_or(0, |at| at + 1);
    let line = std::str::from_utf8(&raw[..line_end]).unwrap_or_default();
    let status = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("HTTP/")
        .and_then(|rest| rest.split_once(' '))
        .filter(|(version, _)| matches!(*version, "1.0" | "1.1" | "2" | "2.0" | "3" | "3.0"))
        .and_then(|(_, rest)| rest.get(..3))
        .filter(|code| code.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|code| code.parse::<u16>().ok());
    let Some(status) = status else {
        return fail("HTTP status is missing or malformed");
    };
    let divider = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| at + 4)
        .into_iter()
        .chain(raw.windows(2).position(|w| w == b"\n\n").map(|at| at + 2))
        .min();
    match divider {
        Some(end) if end <= MAX_HEADERS => Ok((status, raw[end..].to_vec())),
        _ => fail("HTTP header block is missing or too large"),
    }
}

struct Output {
    status: Option<i32>,
    stdout: Vec<u8>,
}

fn drain(
    mut stream: impl Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<Option<Vec<u8>>> {
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = vec![0u8; 65536];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => return Some(buffer),
                Ok(count) => {
                    buffer.extend_from_slice(&chunk[..count]);
                    if buffer.len() > limit {
                        return None;
                    }
                }
                Err(_) => return None,
            }
        }
    })
}

fn bounded(mut command: Command, stdout_limit: usize, label: &str) -> Result<Output> {
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return integrity(format!("{label} could not be started"));
    };
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        return integrity(format!("{label} has no output pipes"));
    };
    let out = drain(out, stdout_limit);
    let err = drain(err, MAX_API_STDERR);
    let deadline = Instant::now() + API_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return integrity(format!("{label} failed or is unknown"));
            }
        }
    };
    match (out.join(), err.join()) {
        (Ok(Some(stdout)), Ok(Some(_))) => Ok(Output { status, stdout }),
        _ => integrity(format!("{label} exceeded its output limit")),
    }
}

/// The live provider: `gh` against github.com with the job token only.
#[derive(Debug, Default)]
pub struct GhProvider;

impl GhProvider {
    fn command(args: &[&str]) -> Result<Command> {
        let token = std::env::var("GH_TOKEN").unwrap_or_default();
        require(
            std::env::var("GH_HOST").as_deref() == Ok("github.com") && !token.trim().is_empty(),
            "provider access requires GH_HOST=github.com and the GitHub job token",
        )?;
        let mut command = Command::new("gh");
        command.args(args);
        for key in FALLBACK_CREDENTIALS
            .iter()
            .chain(&OIDC_CREDENTIALS)
            .chain(&["GOVERNANCE_TOKEN"])
        {
            command.env_remove(key);
        }
        command
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1");
        Ok(command)
    }

    fn run(args: &[&str], limit: usize, label: &str) -> Result<Vec<u8>> {
        let output = bounded(Self::command(args)?, limit, label)?;
        if output.status != Some(0) {
            return integrity(format!(
                "{label} failed or is unknown; no retry or credential fallback"
            ));
        }
        Ok(output.stdout)
    }
}

impl Provider for GhProvider {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn get(&self, path: &str) -> Result<Value> {
        require_fixed_path(path)?;
        let raw = Self::run(
            &[
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                path,
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
            MAX_API_BYTES,
            "GitHub GET response",
        )?;
        if raw.is_empty() {
            return integrity("GitHub GET returned an empty response");
        }
        parse(&raw, "GitHub GET response", MAX_API_BYTES)
    }

    fn committed_file(&self, commit: &str, path: &str, limit: usize) -> Result<Vec<u8>> {
        require(
            super::json::is_hex(commit, 40) && !path.split('/').any(|p| p == ".." || p.is_empty()),
            "committed file selection is outside the closed contract",
        )?;
        let endpoint = format!("repos/{REPOSITORY}/contents/{path}?ref={commit}");
        require_fixed_path(&endpoint)?;
        let raw = Self::run(
            &[
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                &endpoint,
                "-H",
                "Accept: application/vnd.github.raw+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
            limit,
            "committed input read",
        )?;
        Ok(raw)
    }

    fn download_artifact(&self, artifact_id: u64, limit: usize) -> Result<Vec<u8>> {
        let endpoint = format!("repos/{REPOSITORY}/actions/artifacts/{artifact_id}/zip");
        Self::run(
            &[
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                &endpoint,
            ],
            limit,
            "start artifact ZIP download",
        )
    }

    fn download_asset(&self, asset_id: u64, limit: usize) -> Result<Vec<u8>> {
        let endpoint = format!("repos/{REPOSITORY}/releases/assets/{asset_id}");
        let raw = Self::run(
            &[
                "api",
                &endpoint,
                "--method",
                "GET",
                "--include",
                "--hostname",
                "github.com",
                "-H",
                "Accept: application/octet-stream",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
            limit + MAX_HEADERS,
            "selected claim numeric readback",
        )?;
        let (status, body) = included_response(&raw)?;
        if status != 200 || body.len() > limit {
            return integrity("selected claim readback is not bounded HTTP 200; no retry");
        }
        Ok(body)
    }

    fn oidc_check_id(&self, _context: &Value, _expected_event: &str) -> Result<u64> {
        // The issuer request needs an HTTPS client with redirects disabled and
        // the runner's bearer token; the claims check itself is implemented in
        // `observe::require_oidc_claims`. The live fetch lands with the
        // workflow wiring (aros-toolchains#106). Fail closed until then.
        integrity("runner OIDC acquisition is not wired yet; the dispatcher stays inactive")
    }

    fn attest(&self, request: &Attestation<'_>) -> Result<Value> {
        let signer = format!("{}/{}", request.repository, request.signer_workflow);
        let subject = request.subject.to_string_lossy();
        let bundle = request.bundle.to_string_lossy();
        let raw = Self::run(
            &[
                "attestation",
                "verify",
                &subject,
                "--bundle",
                &bundle,
                "--hostname",
                "github.com",
                "--repo",
                request.repository,
                "--signer-workflow",
                &signer,
                "--signer-digest",
                request.commit,
                "--source-digest",
                request.commit,
                "--source-ref",
                super::MAIN_REF,
                "--deny-self-hosted-runners",
                "--format",
                "json",
            ],
            MAX_API_BYTES,
            "signature authentication",
        )?;
        parse(&raw, "signature authentication", MAX_API_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_fixed_repository_is_reachable() {
        assert!(require_fixed_path("repos/metaneutrons/aros-toolchains/actions/runs/1").is_ok());
        assert!(require_fixed_path("repos/metaneutrons/aros-toolchains").is_ok());
        for bad in [
            "repos/metaneutrons/aros-toolchains-evil/x",
            "repos/other/aros-toolchains/x",
            "user",
            "repos/metaneutrons/aros-toolchains/../../x",
        ] {
            assert!(require_fixed_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn included_responses_are_split_and_bounded() {
        let (status, body) =
            included_response(b"HTTP/2.0 200 OK\r\nA: b\r\n\r\n{\"x\":1}").unwrap();
        assert_eq!((status, body.as_slice()), (200, &b"{\"x\":1}"[..]));
        assert_eq!(included_response(b"HTTP/1.1 204\nA: b\n\n").unwrap().0, 204);
        assert!(included_response(b"{}").is_err());
        assert!(included_response(b"HTTP/9 200 OK\r\n\r\n").is_err());
        assert!(included_response(b"HTTP/1.1 2x0 OK\r\n\r\n").is_err());
        assert!(included_response(b"HTTP/1.1 200 OK\r\nno divider").is_err());
        let mut huge = b"HTTP/1.1 200 OK\r\n".to_vec();
        huge.extend(std::iter::repeat_n(b'a', MAX_HEADERS + 1));
        huge.extend_from_slice(b"\r\n\r\nbody");
        assert!(included_response(&huge).is_err());
    }

    #[test]
    fn the_live_provider_refuses_without_the_fixed_host_and_token() {
        // The test environment never sets GH_HOST=github.com together with a
        // token for the fixed provider; the command builder must refuse.
        if std::env::var("GH_HOST").as_deref() != Ok("github.com") {
            assert!(GhProvider
                .get("repos/metaneutrons/aros-toolchains")
                .is_err());
        }
    }
}
