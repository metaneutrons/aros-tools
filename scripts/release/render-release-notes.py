#!/usr/bin/env python3
"""Render the one canonical changelog section used as a release body."""

from __future__ import annotations

import argparse
import os
import re
import tempfile
from pathlib import Path


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"AP7080 {message}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--changelog", required=True, type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", args.version) is None:
        fail("release version is malformed")
    if not args.changelog.is_file() or args.changelog.is_symlink():
        fail("CHANGELOG input is missing or unsafe")
    if args.output.exists() and (not args.output.is_file() or args.output.is_symlink()):
        fail("release-notes output is unsafe")
    if not args.output.parent.is_dir() or args.output.parent.is_symlink():
        fail("release-notes output directory is unsafe")

    source = args.changelog.read_text(encoding="utf-8")
    if "\r" in source:
        fail("CHANGELOG must use canonical LF line endings")
    version = re.escape(args.version)
    heading = re.compile(
        rf"^##\s+(?:\[{version}\](?:\([^\n)]+\))?|{version})(?:\s|$).*$",
        re.MULTILINE,
    )
    matches = list(heading.finditer(source))
    if len(matches) != 1:
        fail(f"CHANGELOG must contain exactly one section for {args.version}")
    start = matches[0].start()
    following = re.search(r"^##\s+", source[matches[0].end() :], re.MULTILINE)
    end = matches[0].end() + following.start() if following else len(source)
    body = source[start:end].rstrip("\n") + "\n"
    if len(body.splitlines()) < 2 or not any(line.strip() for line in body.splitlines()[1:]):
        fail("release-notes section is empty")
    if "\x00" in body:
        fail("release-notes section contains a NUL byte")

    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{args.output.name}.", dir=args.output.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as output:
            output.write(body)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, args.output)
    finally:
        if temporary.exists():
            temporary.unlink()


if __name__ == "__main__":
    main()
