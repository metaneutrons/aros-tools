#!/usr/bin/env python3
"""Closed GitHub Release asset inventory, size, and digest policy.

This module is the single source of truth for API-first release downloads.  It
can emit the compact contract needed by the checkout-free draft-recovery job or
validate GitHub's flattened release-asset JSON before any response body is
downloaded.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


MIB = 1024 * 1024
TOTAL_SIZE_LIMIT = 2 * 1024 * MIB
METADATA_SIZE_LIMIT = MIB
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")
DIGEST = re.compile(r"^sha256:([0-9a-f]{64})$")


def fail(message: str) -> None:
    raise SystemExit(f"::error::AP7081 {message}")


def add_asset(result: dict[str, int], name: str, maximum: int) -> None:
    if name in result:
        fail(f"internal asset contract contains duplicate name: {name}")
    result[name] = maximum


def contract(version: str) -> dict[str, Any]:
    if SEMVER.fullmatch(version) is None:
        fail(f"version is not canonical SemVer: {version!r}")
    assets: dict[str, int] = {}
    for target in (
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ):
        archive = f"aros-tools-v{version}-{target}.tar.gz"
        subjects = {
            archive: 256 * MIB,
            f"{archive}.manifest.json": 4 * MIB,
            f"{archive}.sha256": 64 * 1024,
            f"aros-tools-v{version}-{target}.spdx.json": 32 * MIB,
        }
        for name, maximum in subjects.items():
            add_asset(assets, name, maximum)
            add_asset(assets, f"{name}.sigstore.json", 4 * MIB)
    for architecture in ("amd64", "arm64"):
        package = f"aros-tools_{version}_{architecture}.deb"
        sbom = f"aros-tools_{version}_{architecture}.spdx.json"
        add_asset(assets, package, 256 * MIB)
        add_asset(assets, f"{package}.sigstore.json", 4 * MIB)
        add_asset(assets, sbom, 32 * MIB)
        add_asset(assets, f"{sbom}.sigstore.json", 4 * MIB)
    for name in ("PKGBUILD", "aros-tools.rb", "RELEASE_NOTES.md"):
        add_asset(assets, name, MIB)
        add_asset(assets, f"{name}.sigstore.json", 4 * MIB)
    add_asset(assets, "SHA256SUMS", MIB)
    add_asset(assets, "SHA256SUMS.sigstore.json", 4 * MIB)
    if len(assets) != 48:
        fail(f"internal signed asset contract has {len(assets)} entries instead of 48")
    return {
        "schema_version": 1,
        "version": version,
        "maximum_total_size": TOTAL_SIZE_LIMIT,
        "assets": dict(sorted(assets.items())),
    }


def regular_file(path: Path, label: str) -> None:
    if not path.is_file() or path.is_symlink():
        fail(f"{label} must be one regular file: {path}")


def file_identity(path: Path) -> tuple[int, str]:
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return path.stat().st_size, digest


def load_metadata(path: Path) -> list[dict[str, Any]]:
    regular_file(path, "release asset metadata")
    if path.stat().st_size > METADATA_SIZE_LIMIT:
        fail("release asset metadata exceeds its 1-MiB parsing limit")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"release asset metadata is invalid JSON: {error}")
    if not isinstance(value, list):
        fail("release asset metadata must be one flattened JSON array")
    return value


def validate(args: argparse.Namespace) -> None:
    policy = contract(args.version)
    allowed: dict[str, int] = policy["assets"]
    metadata = load_metadata(args.metadata_json)
    if args.mode == "exact" and len(metadata) != len(allowed):
        fail(f"release has {len(metadata)} assets; expected exactly {len(allowed)}")
    if args.mode == "subset" and len(metadata) > len(allowed):
        fail(f"partial release has {len(metadata)} assets; maximum is {len(allowed)}")

    names: set[str] = set()
    identifiers: set[int] = set()
    rows: list[tuple[str, int, int, str]] = []
    total = 0
    for index, item in enumerate(metadata):
        if not isinstance(item, dict):
            fail(f"release asset metadata entry {index} is not an object")
        name = item.get("name")
        identifier = item.get("id")
        state = item.get("state")
        size = item.get("size")
        digest_value = item.get("digest")
        if not isinstance(name, str) or SAFE_NAME.fullmatch(name) is None:
            fail(f"release asset {index} has an unsafe name")
        if name not in allowed:
            fail(f"release asset is outside the closed inventory: {name}")
        if name in names:
            fail(f"release asset name is duplicated: {name}")
        if not isinstance(identifier, int) or isinstance(identifier, bool) or identifier <= 0:
            fail(f"release asset has an invalid numeric ID: {name}")
        if identifier in identifiers:
            fail(f"release asset ID is duplicated: {identifier}")
        if state != "uploaded":
            fail(f"release asset is not completely uploaded: {name}")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            fail(f"release asset has an invalid size: {name}")
        if size > allowed[name]:
            fail(
                f"release asset exceeds its {allowed[name]}-byte type limit: "
                f"{name} ({size} bytes)"
            )
        if not isinstance(digest_value, str) or DIGEST.fullmatch(digest_value) is None:
            fail(f"release asset has no canonical server SHA-256 digest: {name}")
        names.add(name)
        identifiers.add(identifier)
        total += size
        rows.append((name, identifier, size, digest_value.removeprefix("sha256:")))

    expected_names = set(allowed)
    if args.mode == "exact" and names != expected_names:
        missing = sorted(expected_names - names)
        extra = sorted(names - expected_names)
        fail(f"release asset names differ; missing={missing}, extra={extra}")
    if total > policy["maximum_total_size"]:
        fail(
            f"release asset inventory is {total} bytes; total limit is "
            f"{policy['maximum_total_size']}"
        )

    if args.candidate_dir is not None:
        candidate = args.candidate_dir
        if not candidate.is_dir() or candidate.is_symlink():
            fail("candidate directory must be one real directory")
        candidate_names = set()
        for path in candidate.iterdir():
            regular_file(path, "candidate inventory entry")
            if SAFE_NAME.fullmatch(path.name) is None or path.name in candidate_names:
                fail(f"candidate inventory has an unsafe or duplicate name: {path.name}")
            candidate_names.add(path.name)
        if not candidate_names <= expected_names:
            fail("candidate directory contains a name outside the release contract")
        if args.mode == "exact" and candidate_names != expected_names:
            fail("candidate directory differs from the exact release inventory")
        for name, _identifier, size, digest_value in rows:
            path = candidate / name
            regular_file(path, f"candidate for {name}")
            measured_size, measured_digest = file_identity(path)
            if measured_size != size or measured_digest != digest_value:
                fail(f"GitHub metadata differs from the candidate identity: {name}")

    for name, identifier, size, digest_value in sorted(rows):
        print(f"{identifier}\t{name}\t{size}\t{digest_value}")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    contract_parser = subparsers.add_parser("contract")
    contract_parser.add_argument("--version", required=True)
    validate_parser = subparsers.add_parser("validate")
    validate_parser.add_argument("--version", required=True)
    validate_parser.add_argument("--metadata-json", type=Path, required=True)
    validate_parser.add_argument("--mode", choices=("exact", "subset"), required=True)
    validate_parser.add_argument("--candidate-dir", type=Path)
    identity_parser = subparsers.add_parser("identity")
    identity_parser.add_argument("--file", type=Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_arguments()
    if args.command == "contract":
        print(json.dumps(contract(args.version), sort_keys=True, separators=(",", ":")))
    elif args.command == "identity":
        regular_file(args.file, "identity input")
        size, digest = file_identity(args.file)
        print(f"{size}\t{digest}")
    else:
        validate(args)


if __name__ == "__main__":
    main()
