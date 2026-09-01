#!/usr/bin/env python3
"""Validate the single-version contract shared by Cargo and Release Please.

Release Please's Rust strategy cannot update a virtual Cargo workspace whose
members inherit ``workspace.package.version``.  The repository therefore uses
its version-file-less Go strategy only as a changelog strategy and delegates
the two version updates to generic TOML updaters.  This validator makes that
deliberate adapter fail closed if either tool changes its assumptions.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent.parent
EXPECTED_BOOTSTRAP_SHA = "fb6ba7807c859c6ca20f0f019f1100c94df1375a"
EXPECTED_INITIAL_VERSION = "0.1.0"
EXPECTED_SCHEMA = (
    "https://raw.githubusercontent.com/googleapis/release-please/"
    "main/schemas/config.json"
)
EXPECTED_CONFIG_KEYS = {
    "$schema",
    "bootstrap-sha",
    "bump-minor-pre-major",
    "bump-patch-for-minor-pre-major",
    "include-component-in-tag",
    "include-v-in-tag",
    "initial-version",
    "group-pull-request-title-pattern",
    "packages",
    "release-type",
    "skip-github-release",
}
EXPECTED_ROOT_PACKAGE_KEYS = {
    "changelog-path",
    "extra-files",
    "package-name",
}
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
EXPECTED_EXTRA_FILES = [
    {
        "type": "toml",
        "path": "Cargo.toml",
        "jsonpath": "$.workspace.package.version",
    },
    {
        "type": "toml",
        "path": "Cargo.lock",
        "jsonpath": "$.package[?(!@.source)].version",
    },
]


class ContractErrors:
    """Collect independent contract violations before returning one failure."""

    def __init__(self) -> None:
        self.messages: list[str] = []

    def require(self, condition: bool, message: str) -> None:
        if not condition:
            self.messages.append(message)

    def finish(self) -> None:
        if not self.messages:
            print("version contract: valid")
            return
        for message in self.messages:
            print(f"version contract: {message}", file=sys.stderr)
        raise SystemExit(1)


def read_toml(path: Path) -> dict[str, Any]:
    if path.is_symlink():
        print(f"version contract: {path.relative_to(ROOT)} must not be a symlink", file=sys.stderr)
        raise SystemExit(1)
    try:
        with path.open("rb") as stream:
            value = tomllib.load(stream)
    except (OSError, tomllib.TOMLDecodeError) as error:
        print(f"version contract: cannot read {path.relative_to(ROOT)}: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    if not isinstance(value, dict):
        print(f"version contract: {path.relative_to(ROOT)} is not a TOML table", file=sys.stderr)
        raise SystemExit(1)
    return value


def read_json(path: Path) -> dict[str, Any]:
    if path.is_symlink():
        print(f"version contract: {path.relative_to(ROOT)} must not be a symlink", file=sys.stderr)
        raise SystemExit(1)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        print(f"version contract: cannot read {path.relative_to(ROOT)}: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    if not isinstance(value, dict):
        print(f"version contract: {path.relative_to(ROOT)} is not a JSON object", file=sys.stderr)
        raise SystemExit(1)
    return value


def canonical_semver(value: object) -> bool:
    return isinstance(value, str) and SEMVER.fullmatch(value) is not None


def member_manifests(
    root_manifest: dict[str, Any], errors: ContractErrors
) -> dict[str, Path]:
    workspace = root_manifest.get("workspace")
    members = workspace.get("members") if isinstance(workspace, dict) else None
    if not isinstance(members, list) or not members:
        errors.require(False, "workspace.members must be a non-empty array")
        return {}

    manifests: dict[str, Path] = {}
    for member in members:
        if (
            not isinstance(member, str)
            or not member
            or any(character in member for character in "*?[]")
            or Path(member).is_absolute()
            or ".." in Path(member).parts
        ):
            errors.require(False, f"workspace member {member!r} is not one explicit safe path")
            continue
        candidate = ROOT / member / "Cargo.toml"
        if candidate.is_symlink():
            errors.require(False, f"workspace member {member!r} Cargo.toml must not be a symlink")
            continue
        manifest = candidate.resolve()
        try:
            manifest.relative_to(ROOT)
        except ValueError:
            errors.require(False, f"workspace member {member!r} resolves outside the repository")
            continue
        if not manifest.is_file():
            errors.require(False, f"workspace member {member!r} has no regular Cargo.toml")
            continue
        document = read_toml(manifest)
        package = document.get("package")
        name = package.get("name") if isinstance(package, dict) else None
        errors.require(
            isinstance(name, str) and bool(name),
            f"workspace member {member!r} has no package.name",
        )
        if not isinstance(name, str) or not name:
            continue
        errors.require(name not in manifests, f"workspace package name {name!r} is duplicated")
        version = package.get("version") if isinstance(package, dict) else None
        errors.require(
            version == {"workspace": True},
            f"{member}/Cargo.toml must inherit package.version exclusively from the workspace",
        )
        rust_version = package.get("rust-version") if isinstance(package, dict) else None
        errors.require(
            rust_version == {"workspace": True},
            f"{member}/Cargo.toml must inherit package.rust-version exclusively from the workspace",
        )
        manifests[name] = manifest
    errors.require(
        len(manifests) == len(members),
        "every workspace member must resolve to one uniquely named package",
    )
    return manifests


def validate_metadata(
    version: object, manifests: dict[str, Path], errors: ContractErrors
) -> None:
    try:
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--offline",
                "--no-deps",
                "--format-version",
                "1",
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        metadata = json.loads(result.stdout)
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) else str(error)
        errors.require(False, f"cargo metadata --locked --offline failed: {detail}")
        return

    packages = {package["id"]: package for package in metadata.get("packages", [])}
    workspace_ids = metadata.get("workspace_members")
    if not isinstance(workspace_ids, list):
        errors.require(False, "cargo metadata returned no workspace_members array")
        return
    workspace_packages = [packages.get(identifier) for identifier in workspace_ids]
    errors.require(
        all(isinstance(package, dict) for package in workspace_packages),
        "cargo metadata contains an unresolved workspace member",
    )
    resolved = [package for package in workspace_packages if isinstance(package, dict)]
    errors.require(
        {package.get("name") for package in resolved} == set(manifests),
        "cargo metadata workspace names differ from workspace.members",
    )
    for package in resolved:
        errors.require(
            package.get("version") == version,
            f"cargo metadata resolved {package.get('name')!r} to {package.get('version')!r}, expected {version!r}",
        )


def main() -> None:
    errors = ContractErrors()
    cargo = read_toml(ROOT / "Cargo.toml")
    workspace = cargo.get("workspace")
    package_defaults = workspace.get("package") if isinstance(workspace, dict) else None
    version = package_defaults.get("version") if isinstance(package_defaults, dict) else None
    errors.require(canonical_semver(version), "workspace.package.version is not canonical SemVer")
    minimum_rust = package_defaults.get("rust-version") if isinstance(package_defaults, dict) else None
    errors.require(
        isinstance(minimum_rust, str)
        and re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", minimum_rust) is not None,
        "workspace.package.rust-version must be one exact major.minor release",
    )
    toolchain = read_toml(ROOT / "rust-toolchain.toml").get("toolchain")
    toolchain_channel = toolchain.get("channel") if isinstance(toolchain, dict) else None
    errors.require(
        isinstance(toolchain_channel, str)
        and isinstance(minimum_rust, str)
        and canonical_semver(toolchain_channel)
        and toolchain_channel.rsplit(".", 1)[0] == minimum_rust,
        "rust-toolchain.toml must select one exact patch release in workspace.package.rust-version",
    )

    manifests = member_manifests(cargo, errors)

    lock = read_toml(ROOT / "Cargo.lock")
    lock_packages = lock.get("package")
    if not isinstance(lock_packages, list):
        errors.require(False, "Cargo.lock has no package array")
        local_packages: list[dict[str, Any]] = []
    else:
        local_packages = [
            package
            for package in lock_packages
            if isinstance(package, dict) and "source" not in package
        ]
    local_names = [package.get("name") for package in local_packages]
    errors.require(
        len(local_names) == len(set(local_names)),
        "Cargo.lock contains duplicate source-less package names",
    )
    errors.require(
        set(local_names) == set(manifests),
        "Cargo.lock source-less packages must be exactly the workspace members",
    )
    for package in local_packages:
        errors.require(
            package.get("version") == version,
            f"Cargo.lock package {package.get('name')!r} has version {package.get('version')!r}, expected {version!r}",
        )

    config = read_json(ROOT / "release-please-config.json")
    initial_version = config.get("initial-version")
    errors.require(
        set(config) == EXPECTED_CONFIG_KEYS,
        "Release Please root configuration contains missing or unreviewed keys",
    )
    errors.require(
        config.get("$schema") == EXPECTED_SCHEMA,
        "Release Please must reference its canonical configuration schema",
    )
    errors.require(config.get("release-type") == "go", "Release Please must use the version-file-less Go strategy adapter")
    errors.require(
        initial_version == EXPECTED_INITIAL_VERSION,
        f"Release Please initial-version must remain {EXPECTED_INITIAL_VERSION}",
    )
    errors.require(config.get("include-v-in-tag") is True, "Release Please tag versions must retain the v prefix")
    errors.require(
        config.get("include-component-in-tag") is False,
        "Release Please must not add a component to the workspace tag",
    )
    errors.require(
        config.get("bump-minor-pre-major") is True,
        "Release Please must retain the reviewed pre-1.0 feature bump policy",
    )
    errors.require(
        config.get("bump-patch-for-minor-pre-major") is True,
        "Release Please must retain the reviewed pre-1.0 fix bump policy",
    )
    errors.require(config.get("skip-github-release") is True, "Release Please must remain PR-only")
    errors.require(
        config.get("group-pull-request-title-pattern")
        == "chore${scope}: release${component} ${version}",
        "Release Please grouped PR titles must retain scope, component, and version",
    )
    errors.require(not config.get("plugins"), "Release Please plugins must not include cargo-workspace")
    bootstrap_sha = config.get("bootstrap-sha")
    errors.require(
        bootstrap_sha == EXPECTED_BOOTSTRAP_SHA,
        f"Release Please bootstrap-sha must remain {EXPECTED_BOOTSTRAP_SHA}",
    )
    packages = config.get("packages")
    root_package = packages.get(".") if isinstance(packages, dict) else None
    errors.require(
        isinstance(packages, dict) and set(packages) == {"."},
        "Release Please must manage exactly the root workspace",
    )
    errors.require(
        isinstance(root_package, dict) and set(root_package) == EXPECTED_ROOT_PACKAGE_KEYS,
        "Release Please root package contains missing or unreviewed keys",
    )
    errors.require(
        isinstance(root_package, dict)
        and root_package.get("package-name") == "aros-tools"
        and root_package.get("changelog-path") == "CHANGELOG.md",
        "Release Please must manage the aros-tools root changelog",
    )
    errors.require(
        isinstance(root_package, dict)
        and root_package.get("extra-files") == EXPECTED_EXTRA_FILES,
        "Release Please must update exactly the workspace version and source-less lock-package versions",
    )

    manifest = read_json(ROOT / ".release-please-manifest.json")
    manifest_version = manifest.get(".")
    errors.require(set(manifest) == {"."}, "Release Please manifest must contain only the root workspace")
    errors.require(canonical_semver(manifest_version), "Release Please manifest version is not canonical SemVer")
    bootstrap = manifest_version == "0.0.0" and version == initial_version
    errors.require(
        manifest_version == version or bootstrap,
        "Release Please state must match the workspace version (except the explicit 0.0.0 bootstrap)",
    )

    if canonical_semver(version) and manifests:
        validate_metadata(version, manifests, errors)
    errors.finish()


if __name__ == "__main__":
    main()
