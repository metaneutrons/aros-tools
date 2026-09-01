#!/usr/bin/env python3
"""Fail closed when local development runtimes violate the versioned contract."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 and older: reported without a traceback.
    tomllib = None  # type: ignore[assignment]


def fail(message: str) -> None:
    raise SystemExit(f"error: {message}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--require-node", action="store_true")
    parser.add_argument("--python-version-for-test", help=argparse.SUPPRESS)
    parser.add_argument(
        "--contract",
        type=Path,
        default=Path(__file__).resolve().parent.parent
        / "contracts/development-runtimes-v1.toml",
    )
    args = parser.parse_args()
    if args.python_version_for_test is not None:
        if os.environ.get("AROS_RUNTIME_CONTRACT_FIXTURE") != "1":
            fail("synthetic runtime versions are restricted to policy fixtures")
        try:
            actual_python = tuple(map(int, args.python_version_for_test.split(".")))
        except ValueError:
            fail("synthetic Python version is malformed")
        if len(actual_python) != 2:
            fail("synthetic Python version is malformed")
    else:
        actual_python = sys.version_info[:2]
    try:
        contract_text = args.contract.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read development runtime contract: {error}")
    python_rows = re.findall(
        r'^python_minimum = "(\d+)\.(\d+)"$', contract_text, re.MULTILINE
    )
    if len(python_rows) != 1:
        fail("development runtime contract has no singular canonical Python minimum")
    minimum_python = tuple(map(int, python_rows[0]))
    if actual_python < minimum_python or tomllib is None:
        fail(
            f"Python >= {minimum_python[0]}.{minimum_python[1]} with tomllib is required; "
            f"found {actual_python[0]}.{actual_python[1]}"
        )
    with args.contract.open("rb") as stream:
        contract = tomllib.load(stream)
    if set(contract) != {"schema_version", "python_minimum", "node_minimum_major"}:
        fail("development runtime contract has unexpected fields")
    if contract["schema_version"] != 1:
        fail("unsupported development runtime contract schema")
    match = re.fullmatch(r"(\d+)\.(\d+)", str(contract["python_minimum"]))
    if match is None:
        fail("Python minimum in the development runtime contract is malformed")
    parsed_minimum_python = tuple(map(int, match.groups()))
    if parsed_minimum_python != minimum_python:
        fail("Python minimum differs between bootstrap and parsed runtime contract")
    if actual_python < parsed_minimum_python:
        fail(
            f"Python >= {parsed_minimum_python[0]}.{parsed_minimum_python[1]} is required; "
            f"found {actual_python[0]}.{actual_python[1]}"
        )

    minimum_node = contract["node_minimum_major"]
    if not isinstance(minimum_node, int) or isinstance(minimum_node, bool) or minimum_node < 1:
        fail("Node minimum in the development runtime contract is malformed")
    if args.require_node:
        try:
            completed = subprocess.run(
                ["node", "--version"],
                check=True,
                capture_output=True,
                text=True,
                timeout=10,
            )
        except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
            fail(f"Node.js >= {minimum_node} is required by the documentation gate")
        node = completed.stdout.strip()
        node_match = re.fullmatch(r"v(\d+)\.\d+\.\d+(?:[-+].*)?", node)
        if node_match is None or int(node_match.group(1)) < minimum_node:
            fail(f"Node.js >= {minimum_node} is required; found {node!r}")


if __name__ == "__main__":
    main()
