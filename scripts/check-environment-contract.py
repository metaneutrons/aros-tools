#!/usr/bin/env python3
"""Verify that environment inputs and their versioned documentation agree."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path


ENV_PATTERNS = (
    re.compile(r'\benv\s*=\s*"([A-Z][A-Z0-9_]*)"'),
    re.compile(r'\bstd::env::var(?:_os)?\(\s*"([A-Z][A-Z0-9_]*)"'),
    re.compile(r'\benvironment\(\s*"([A-Z][A-Z0-9_]*)"'),
    re.compile(r'\btest_point_matches\(\s*"([A-Z][A-Z0-9_]*)"'),
    re.compile(
        r'\b(?:shared_)?requested_diagnostic_format\(\s*[^,\n()]+,\s*"([A-Z][A-Z0-9_]*)"'
    ),
)


def load_contract(path: Path) -> tuple[set[str], set[str], set[str], Path]:
    with path.open("rb") as stream:
        contract = tomllib.load(stream)
    if contract.get("schema") != 1:
        raise ValueError(f"{path}: unsupported environment-contract schema")
    groups = []
    for name in ("public", "ambient", "test_internal"):
        values = contract.get(name)
        if not isinstance(values, list) or not all(
            isinstance(value, str) and re.fullmatch(r"[A-Z][A-Z0-9_]*", value)
            for value in values
        ):
            raise ValueError(f"{path}: {name} must contain valid environment names")
        if values != sorted(set(values)):
            raise ValueError(f"{path}: {name} must be sorted and duplicate-free")
        groups.append(set(values))
    public, ambient, test_internal = groups
    overlaps = (public & ambient) | (public & test_internal) | (ambient & test_internal)
    if overlaps:
        raise ValueError(f"{path}: environment groups overlap: {sorted(overlaps)}")
    documentation = contract.get("documentation")
    if not isinstance(documentation, str) or not documentation:
        raise ValueError(f"{path}: documentation must name the canonical reference")
    return public, ambient, test_internal, Path(documentation)


def discover_environment_inputs(repository: Path) -> set[str]:
    discovered: set[str] = set()
    source_files = sorted(repository.glob("crates/*/src/**/*.rs"))
    source_files.extend(sorted(repository.glob("crates/*/tests/**/*.rs")))
    for path in source_files:
        text = path.read_text(encoding="utf-8")
        for pattern in ENV_PATTERNS:
            discovered.update(pattern.findall(text))
    return discovered


def validate(repository: Path, contract_path: Path) -> list[str]:
    public, ambient, test_internal, documentation_path = load_contract(contract_path)
    declared = public | ambient | test_internal
    discovered = discover_environment_inputs(repository)
    errors = []
    undeclared = discovered - declared
    stale = declared - discovered
    if undeclared:
        errors.append(f"environment inputs absent from contract: {sorted(undeclared)}")
    if stale:
        errors.append(f"contract names absent from code: {sorted(stale)}")

    documentation = repository / documentation_path
    try:
        text = documentation.read_text(encoding="utf-8")
    except OSError as error:
        errors.append(f"cannot read canonical environment documentation: {error}")
    else:
        undocumented = [name for name in sorted(declared) if f"`{name}`" not in text]
        if undocumented:
            errors.append(f"contract names absent from documentation: {undocumented}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--repository", type=Path, default=Path(__file__).resolve().parent.parent
    )
    parser.add_argument(
        "--contract", type=Path, default=Path("contracts/public-environment-v1.toml")
    )
    arguments = parser.parse_args()
    repository = arguments.repository.resolve()
    contract = arguments.contract
    if not contract.is_absolute():
        contract = repository / contract
    try:
        errors = validate(repository, contract)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
