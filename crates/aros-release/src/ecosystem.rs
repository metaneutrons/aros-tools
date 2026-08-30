//! Fail-closed Homebrew and AUR metadata generation from verified manifests.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage, Sha256Digest};

use crate::archive::{write_new_atomic, ReleaseManifest, MANIFEST_SCHEMA};
use crate::contract::{valid_version, EcosystemFormat, GenerateArgs};
use crate::{ReleaseFailure, ReleaseResult};

const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
];

/// Generate one package-manager file from the complete native manifest set.
///
/// # Errors
///
/// Returns a stable release-contract or publication diagnostic if identities
/// differ, a target is missing, input is malformed, or output is not new.
pub fn generate(args: &GenerateArgs) -> ReleaseResult<()> {
    let release = load_release(args)?;
    let bytes = match args.format {
        EcosystemFormat::Homebrew => render_homebrew(&release),
        EcosystemFormat::Aur => render_aur(&release),
    };
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            generation_failure(
                &args.output,
                format!("cannot create metadata output directory: {error}"),
            )
        })?;
    }
    write_new_atomic(&args.output, bytes.as_bytes())
}

#[derive(Debug)]
struct NativeRelease {
    version: String,
    source_commit: String,
    base_url: String,
    manifests: BTreeMap<String, ReleaseManifest>,
}

impl NativeRelease {
    fn artifact(&self, target: &str) -> &ReleaseManifest {
        self.manifests
            .get(target)
            .expect("complete native manifest set")
    }

    fn url(&self, target: &str) -> String {
        format!("{}/{}", self.base_url, self.artifact(target).archive)
    }
}

fn load_release(args: &GenerateArgs) -> ReleaseResult<NativeRelease> {
    if args.manifests.len() != TARGETS.len() {
        return Err(contract_failure(
            &args.output,
            format!(
                "package-manager metadata needs exactly four manifests; received {}",
                args.manifests.len()
            ),
        ));
    }
    let base_url = args.base_url.trim_end_matches('/');
    if !base_url.starts_with("https://")
        || base_url.bytes().any(|byte| byte.is_ascii_whitespace())
        || base_url.contains(['?', '#'])
    {
        return Err(contract_failure(
            &args.output,
            "base-url must be an HTTPS directory without whitespace, query or fragment",
        ));
    }
    let mut manifests = BTreeMap::new();
    for path in &args.manifests {
        let bytes = fs::read(path).map_err(|error| {
            contract_failure(path, format!("cannot read release manifest: {error}"))
        })?;
        let manifest: ReleaseManifest = serde_json::from_slice(&bytes).map_err(|error| {
            contract_failure(path, format!("cannot parse release manifest: {error}"))
        })?;
        validate_manifest(path, &manifest)?;
        let target = manifest.target.clone();
        if manifests.insert(target.clone(), manifest).is_some() {
            return Err(contract_failure(
                path,
                format!("duplicate native manifest target {target:?}"),
            ));
        }
    }
    let measured_targets: Vec<_> = manifests.keys().map(String::as_str).collect();
    if measured_targets != TARGETS {
        return Err(contract_failure(
            &args.output,
            format!(
                "native manifest matrix is incomplete: measured {}",
                measured_targets.join(", ")
            ),
        ));
    }
    let first = manifests.values().next().expect("nonempty native matrix");
    let version = first.version.clone();
    let source_commit = first.source_commit.clone();
    let source_date_epoch = first.source_date_epoch;
    if manifests.values().any(|manifest| {
        manifest.version != version
            || manifest.source_commit != source_commit
            || manifest.source_date_epoch != source_date_epoch
    }) {
        return Err(contract_failure(
            &args.output,
            "native manifests do not share one version, source commit and source date",
        ));
    }
    Ok(NativeRelease {
        version,
        source_commit,
        base_url: base_url.to_string(),
        manifests,
    })
}

fn validate_manifest(path: &Path, manifest: &ReleaseManifest) -> ReleaseResult<()> {
    if manifest.schema != MANIFEST_SCHEMA
        || manifest.package != "aros-tools"
        || !valid_version(&manifest.version)
        || !TARGETS.contains(&manifest.target.as_str())
        || manifest.source_commit.len() != 40
        || !manifest
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || Sha256Digest::parse(&manifest.archive_sha256).is_err()
    {
        return Err(contract_failure(
            path,
            "release manifest has an unsupported or malformed identity",
        ));
    }
    let expected_archive = format!(
        "aros-tools-v{}-{}.tar.gz",
        manifest.version, manifest.target
    );
    if manifest.archive != expected_archive {
        return Err(contract_failure(
            path,
            format!(
                "manifest archive {:?} does not equal closed name {expected_archive:?}",
                manifest.archive
            ),
        ));
    }
    Ok(())
}

fn render_homebrew(release: &NativeRelease) -> String {
    let mac_arm = release.artifact("aarch64-apple-darwin");
    let mac_x86 = release.artifact("x86_64-apple-darwin");
    let linux_arm = release.artifact("aarch64-unknown-linux-gnu");
    let linux_x86 = release.artifact("x86_64-unknown-linux-gnu");
    let mut output = String::new();
    writeln!(output, "class ArosTools < Formula").unwrap();
    writeln!(
        output,
        "  desc \"Reproducible host-side build and development tools for AROS\""
    )
    .unwrap();
    writeln!(
        output,
        "  homepage \"https://github.com/metaneutrons/aros-tools\""
    )
    .unwrap();
    writeln!(output, "  version \"{}\"", release.version).unwrap();
    writeln!(output, "  license any_of: [\"MIT\", \"Apache-2.0\"]").unwrap();
    writeln!(output).unwrap();
    write_homebrew_os(
        &mut output,
        "macos",
        &release.url("aarch64-apple-darwin"),
        &mac_arm.archive_sha256,
        &release.url("x86_64-apple-darwin"),
        &mac_x86.archive_sha256,
    );
    writeln!(output).unwrap();
    write_homebrew_os(
        &mut output,
        "linux",
        &release.url("aarch64-unknown-linux-gnu"),
        &linux_arm.archive_sha256,
        &release.url("x86_64-unknown-linux-gnu"),
        &linux_x86.archive_sha256,
    );
    writeln!(output).unwrap();
    writeln!(output, "  def install").unwrap();
    writeln!(output, "    bin.install Dir[\"bin/*\"]").unwrap();
    writeln!(output, "  end").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "  test do").unwrap();
    writeln!(
        output,
        "    assert_match version.to_s, shell_output(\"#{{bin}}/aros --version\")"
    )
    .unwrap();
    writeln!(output, "    system \"#{{bin}}/aros-collect\", \"--help\"").unwrap();
    writeln!(output, "  end").unwrap();
    writeln!(output, "end").unwrap();
    output
}

fn write_homebrew_os(
    output: &mut String,
    os: &str,
    arm_url: &str,
    arm_sha: &str,
    x86_url: &str,
    x86_sha: &str,
) {
    writeln!(output, "  on_{os} do").unwrap();
    writeln!(output, "    if Hardware::CPU.arm?").unwrap();
    writeln!(output, "      url \"{arm_url}\"").unwrap();
    writeln!(output, "      sha256 \"{arm_sha}\"").unwrap();
    writeln!(output, "    else").unwrap();
    writeln!(output, "      url \"{x86_url}\"").unwrap();
    writeln!(output, "      sha256 \"{x86_sha}\"").unwrap();
    writeln!(output, "    end").unwrap();
    writeln!(output, "  end").unwrap();
}

fn render_aur(release: &NativeRelease) -> String {
    let arm = release.artifact("aarch64-unknown-linux-gnu");
    let x86 = release.artifact("x86_64-unknown-linux-gnu");
    let pkgver = release.version.replace('-', "_");
    let mut output = String::new();
    writeln!(
        output,
        "# Generated from verified aros-tools release manifests."
    )
    .unwrap();
    writeln!(output, "# Source commit: {}", release.source_commit).unwrap();
    writeln!(output, "pkgname=aros-tools-bin").unwrap();
    writeln!(output, "pkgver={pkgver}").unwrap();
    writeln!(output, "pkgrel=1").unwrap();
    writeln!(
        output,
        "pkgdesc='Reproducible host-side build and development tools for AROS'"
    )
    .unwrap();
    writeln!(output, "arch=('x86_64' 'aarch64')").unwrap();
    writeln!(output, "url='https://github.com/metaneutrons/aros-tools'").unwrap();
    writeln!(output, "license=('MIT' 'Apache-2.0')").unwrap();
    writeln!(output, "depends=('glibc' 'gcc-libs' 'xz')").unwrap();
    writeln!(output, "provides=('aros-tools')").unwrap();
    writeln!(output, "conflicts=('aros-tools')").unwrap();
    writeln!(output, "options=('!strip')").unwrap();
    writeln!(output, "source_x86_64=('{}')", release.url(&x86.target)).unwrap();
    writeln!(output, "sha256sums_x86_64=('{}')", x86.archive_sha256).unwrap();
    writeln!(output, "source_aarch64=('{}')", release.url(&arm.target)).unwrap();
    writeln!(output, "sha256sums_aarch64=('{}')", arm.archive_sha256).unwrap();
    writeln!(output).unwrap();
    writeln!(output, "package() {{").unwrap();
    writeln!(output, "  local target").unwrap();
    writeln!(output, "  case \"$CARCH\" in").unwrap();
    writeln!(output, "    x86_64) target='{}' ;;", x86.target).unwrap();
    writeln!(output, "    aarch64) target='{}' ;;", arm.target).unwrap();
    writeln!(output, "    *) return 1 ;;").unwrap();
    writeln!(output, "  esac").unwrap();
    writeln!(
        output,
        "  local root=\"$srcdir/aros-tools-v{}-$target\"",
        release.version
    )
    .unwrap();
    writeln!(
        output,
        "  install -Dm755 \"$root\"/bin/* -t \"$pkgdir/usr/bin\""
    )
    .unwrap();
    writeln!(
        output,
        "  install -Dm644 \"$root/README.md\" -t \"$pkgdir/usr/share/doc/aros-tools\""
    )
    .unwrap();
    writeln!(
        output,
        "  install -Dm644 \"$root\"/LICENSE-* -t \"$pkgdir/usr/share/licenses/aros-tools\""
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    output
}

fn contract_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleaseContract,
            DiagnosticStage::ReleaseContract,
            message,
        )
        .with_context(DiagnosticContext {
            target: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        })
        .with_hint("use the complete verified four-host manifest set from one immutable release"),
    )
}

fn generation_failure(path: &Path, message: impl Into<String>) -> ReleaseFailure {
    ReleaseFailure::new(
        Diagnostic::error(
            DiagnosticCode::ReleasePublication,
            DiagnosticStage::Publication,
            message,
        )
        .with_context(DiagnosticContext {
            output: Some(path.display().to_string()),
            ..DiagnosticContext::default()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::ReleaseFile;

    fn manifest(target: &str, byte: char) -> ReleaseManifest {
        let version = "1.2.3";
        ReleaseManifest {
            schema: MANIFEST_SCHEMA,
            package: "aros-tools".into(),
            version: version.into(),
            target: target.into(),
            source_commit: "a".repeat(40),
            source_date_epoch: 1_700_000_000,
            archive: format!("aros-tools-v{version}-{target}.tar.gz"),
            archive_sha256: byte.to_string().repeat(64),
            archive_size: 1,
            files: vec![ReleaseFile {
                path: "bin/aros".into(),
                mode: "0755".into(),
                sha256: "f".repeat(64),
                size: 1,
            }],
        }
    }

    fn inputs(root: &Path) -> Vec<std::path::PathBuf> {
        TARGETS
            .iter()
            .enumerate()
            .map(|(index, target)| {
                let path = root.join(format!("{index}.json"));
                fs::write(
                    &path,
                    serde_json::to_vec(&manifest(
                        target,
                        char::from(b'a' + u8::try_from(index).unwrap()),
                    ))
                    .unwrap(),
                )
                .unwrap();
                path
            })
            .collect()
    }

    #[test]
    fn homebrew_uses_all_four_measured_hashes() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("aros-tools.rb");
        generate(&GenerateArgs {
            format: EcosystemFormat::Homebrew,
            base_url: "https://example.invalid/v1.2.3".into(),
            manifests: inputs(root.path()),
            output: output.clone(),
        })
        .unwrap();
        let rendered = fs::read_to_string(output).unwrap();
        for byte in ['a', 'b', 'c', 'd'] {
            assert!(rendered.contains(&byte.to_string().repeat(64)));
        }
        assert!(rendered.contains("on_macos"));
        assert!(rendered.contains("on_linux"));
    }

    #[test]
    fn aur_uses_both_linux_targets_without_skip_hashes() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("PKGBUILD");
        generate(&GenerateArgs {
            format: EcosystemFormat::Aur,
            base_url: "https://example.invalid/v1.2.3".into(),
            manifests: inputs(root.path()),
            output: output.clone(),
        })
        .unwrap();
        let rendered = fs::read_to_string(output).unwrap();
        assert!(rendered.contains("source_x86_64"));
        assert!(rendered.contains("source_aarch64"));
        assert!(!rendered.contains("SKIP"));
    }

    #[test]
    fn mixed_source_identity_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let manifests = inputs(root.path());
        let mut changed: ReleaseManifest =
            serde_json::from_slice(&fs::read(&manifests[0]).unwrap()).unwrap();
        changed.source_commit = "b".repeat(40);
        fs::write(&manifests[0], serde_json::to_vec(&changed).unwrap()).unwrap();
        let failure = generate(&GenerateArgs {
            format: EcosystemFormat::Aur,
            base_url: "https://example.invalid/v1.2.3".into(),
            manifests,
            output: root.path().join("PKGBUILD"),
        })
        .unwrap_err();
        assert_eq!(failure.diagnostic().code, DiagnosticCode::ReleaseContract);
    }
}
