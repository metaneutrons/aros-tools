#!/usr/bin/env python3
"""Validate the central archive's inert manifest against our consumer contract."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import sys
import tomllib

spec = importlib.util.spec_from_file_location("central_apt", Path(__file__).with_name("verify-central-apt.py"))
assert spec is not None and spec.loader is not None
apt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apt)


def validate(manifest: dict, contract: dict) -> None:
    apt.require(isinstance(manifest, dict) and isinstance(manifest.get("projects"), list)
                and all(isinstance(manifest.get(section), dict) for section in ("domain", "signing", "release")),
                "central archive manifest must declare domain, signing, release and projects tables")
    project = [item for item in manifest.get("projects", [])
               if isinstance(item, dict) and item.get("name") == contract["project"]]
    apt.require(len(project) == 1, "central manifest does not declare the project exactly once")
    expected = {
        ("domain", "host"): "deb." + contract["domain"],
        ("domain", "base_url"): contract["base_url"],
        ("domain", "origin"): contract["origin"],
        ("domain", "keyring_package"): contract["keyring"].removesuffix(".pgp"),
        ("domain", "keyring_file"): "/usr/share/keyrings/" + contract["keyring"],
        ("signing", "primary_fingerprint"): contract["primary_fingerprint"],
        ("signing", "signing_subkey"): contract["signing_subkey"],
        ("release", "suite"): contract["suite"],
        ("release", "codename"): contract["suite"],
        ("release", "components"): [contract["component"]],
        ("release", "architectures"): contract["architectures"],
        ("release", "acquire_by_hash"): True,
        ("release", "valid_until_days"): contract["valid_until_days"],
    }
    for (section, field), value in expected.items():
        actual = manifest.get(section, {}).get(field)
        apt.require(type(actual) is type(value) and actual == value,
                    f"central archive {section}.{field} differs from the reviewed consumer contract")
    for field, value in {"prefix": "/" + contract["prefix"],
                         "source_repo": "metaneutrons/aros-tools", "packages": ["aros-tools"],
                         "keep_versions": contract["keep_versions"]}.items():
        apt.require(type(project[0].get(field)) is type(value) and project[0].get(field) == value,
                    f"central archive project.{field} differs from the reviewed consumer contract")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--documentation", type=Path)
    args = parser.parse_args()
    contract = apt.load_contract()
    if args.manifest is not None:
        apt.regular(args.manifest)
        apt.require(args.manifest.stat().st_size <= 1024 * 1024, "central archive manifest is oversized")
        with args.manifest.open("rb") as stream:
            validate(tomllib.load(stream), contract)
    if args.documentation is not None:
        apt.regular(args.documentation)
        text = args.documentation.read_text(encoding="utf-8")
        for value in (contract["primary_fingerprint"], contract["signing_subkey"],
                      contract["keyring"], "Suites: " + contract["suite"],
                      contract["base_url"] + "/" + contract["prefix"]):
            apt.require(value in text, f"installation documentation omits central APT identity {value}")
    print(json.dumps(contract, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (apt.VerificationError, OSError, UnicodeError, KeyError, TypeError, ValueError) as error:
        print(f"::error::AP7251 central APT contract rejected: {error}", file=sys.stderr)
        sys.exit(1)
