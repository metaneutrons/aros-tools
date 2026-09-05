#!/usr/bin/env python3
"""Validate the public documentation routing and staged asset boundary."""

from __future__ import annotations

import argparse
import datetime
import json
import re
import stat
from pathlib import Path


EXPECTED_ROUTES = [
    {
        "pattern": "aros.metaneutrons.cc/aros-tools",
        "zone_name": "metaneutrons.cc",
    },
    {
        "pattern": "aros.metaneutrons.cc/aros-tools/*",
        "zone_name": "metaneutrons.cc",
    },
]


def fail(message: str) -> None:
    raise SystemExit(f"error: {message}")


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--configuration", type=Path, required=True)
    parser.add_argument("--assets", type=Path, required=True)
    return parser.parse_args()


def validate_configuration(path: Path) -> None:
    try:
        configuration = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot read strict JSON Worker configuration {path}: {error}")
    if not isinstance(configuration, dict):
        fail("Worker configuration root must be an object")

    expected_scalars = {
        "$schema": "./node_modules/wrangler/config-schema.json",
        "name": "aros-tools-docs",
        "account_id": "9122b44fa4c05b23985d6a0b779caa01",
        "workers_dev": False,
        "preview_urls": False,
    }
    expected_keys = {
        "$schema",
        *expected_scalars,
        "compatibility_date",
        "routes",
        "assets",
    }
    if set(configuration) != expected_keys:
        unexpected = sorted(set(configuration) - expected_keys)
        missing = sorted(expected_keys - set(configuration))
        fail(
            "Worker configuration keys differ from the static-only contract "
            f"(unexpected={unexpected}, missing={missing})"
        )
    for key, expected in expected_scalars.items():
        if configuration.get(key) != expected:
            fail(f"Worker {key} must be {expected!r}")
    compatibility_date = configuration.get("compatibility_date")
    if not isinstance(compatibility_date, str) or re.fullmatch(
        r"20\d{2}-\d{2}-\d{2}", compatibility_date
    ) is None:
        fail("Worker compatibility_date is missing or malformed")
    try:
        datetime.date.fromisoformat(compatibility_date)
    except ValueError:
        fail("Worker compatibility_date is not a calendar date")
    if configuration.get("routes") != EXPECTED_ROUTES:
        fail("Worker routes do not exactly own the two /aros-tools boundaries")
    if "main" in configuration:
        fail("documentation Worker must remain static-assets-only")
    expected_assets = {
        "directory": "./worker-dist",
        "html_handling": "auto-trailing-slash",
        "not_found_handling": "404-page",
    }
    if configuration.get("assets") != expected_assets:
        fail("Worker asset configuration differs from the static SSG contract")


def validate_assets(root: Path) -> None:
    try:
        root_status = root.lstat()
    except OSError as error:
        fail(f"cannot inspect Worker asset root {root}: {error}")
    if not stat.S_ISDIR(root_status.st_mode) or root.is_symlink():
        fail(f"Worker asset root is not a real directory: {root}")

    entries = sorted(root.iterdir(), key=lambda entry: entry.name)
    if [entry.name for entry in entries] != ["aros-tools"]:
        fail("Worker asset root must contain only the aros-tools prefix")
    prefix = entries[0]
    required = {prefix / "index.html", prefix / "404.html"}
    files: set[Path] = set()
    pending = [prefix]
    while pending:
        directory = pending.pop()
        for entry in directory.iterdir():
            status = entry.lstat()
            if entry.is_symlink():
                fail(f"Worker asset tree contains a symbolic link: {entry}")
            if stat.S_ISDIR(status.st_mode):
                pending.append(entry)
            elif stat.S_ISREG(status.st_mode):
                files.add(entry)
            else:
                fail(f"Worker asset tree contains a non-regular entry: {entry}")
    if not required.issubset(files):
        fail("Worker asset tree lacks index.html or 404.html")
    if prefix / "CNAME" in files:
        fail("Worker asset tree contains the retired GitHub Pages CNAME")
    if not files:
        fail("Worker asset tree is empty")
    print(f"validated {len(files)} regular documentation assets below /aros-tools/")


def main() -> None:
    arguments = parse_arguments()
    try:
        validate_configuration(arguments.configuration)
        validate_assets(arguments.assets)
    except (OSError, UnicodeError) as error:
        fail(f"cannot validate documentation publication contract: {error}")


if __name__ == "__main__":
    main()
