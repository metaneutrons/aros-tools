#!/usr/bin/env python3
"""Plan the bounded host-test matrix from immutable GitHub event inputs.

The planner is deliberately fail-closed.  Only a narrow documentation-only
allowlist may use the inexpensive Linux lane; an empty, malformed or unfamiliar
change list always receives the full native four-host matrix.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path, PurePosixPath
import sys
from typing import Final


HOSTS: Final = (
    {"name": "linux-x86_64", "runner": "ubuntu-24.04"},
    {"name": "linux-aarch64", "runner": "ubuntu-24.04-arm"},
    {"name": "macos-x86_64", "runner": "macos-15-intel"},
    {"name": "macos-aarch64", "runner": "macos-15"},
)
DOCUMENTATION_FILES: Final = {
    "README.md",
    "CONTRIBUTING.md",
}
DOCUMENTATION_DIRECTORIES: Final = ("docs/", "docs-site/")
MAX_CHANGED_PATHS_BYTES: Final = 1024 * 1024


class PolicyError(ValueError):
    """The caller supplied an input which cannot safely select a reduced gate."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PolicyError(message)


def validate_path(value: str) -> str:
    require(value and "\x00" not in value and "\n" not in value and "\r" not in value,
            "changed path is empty or contains a control separator")
    path = PurePosixPath(value)
    require(not path.is_absolute() and ".." not in path.parts and "." not in path.parts,
            f"changed path is not repository-relative: {value!r}")
    require(all(part and not any(character.isspace() and character not in " \t" for character in part)
                for part in path.parts),
            f"changed path contains an unsafe control character: {value!r}")
    return value


def read_changed_paths(path: Path) -> tuple[str, ...]:
    try:
        status = path.lstat()
        require(not path.is_symlink() and path.is_file(), "changed-path input must be a regular file")
        require(status.st_size <= MAX_CHANGED_PATHS_BYTES,
                "changed-path input exceeds its one-megabyte bound")
        payload = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        raise PolicyError(f"cannot read changed-path input: {error}") from error
    paths = tuple(validate_path(line) for line in payload.splitlines())
    require(paths, "changed-path input is empty")
    require(len(paths) == len(set(paths)), "changed-path input contains a duplicate path")
    return paths


def is_documentation_only(paths: tuple[str, ...]) -> bool:
    return bool(paths) and all(
        path in DOCUMENTATION_FILES or path.startswith(DOCUMENTATION_DIRECTORIES)
        for path in paths
    )


def matrix(scope: str, *, source_qualification: bool) -> dict[str, list[dict[str, object]]]:
    require(scope in {"fast", "full"}, f"unsupported matrix scope: {scope!r}")
    selected = HOSTS[:1] if scope == "fast" else HOSTS
    return {
        "include": [
            {**host, "source_qualification": source_qualification and host["name"] == "linux-x86_64"}
            for host in selected
        ]
    }


def plan(*, event: str, changed_paths: tuple[str, ...] | None, dispatch_scope: str) -> tuple[str, str, dict[str, list[dict[str, object]]]]:
    require(event in {"pull_request", "push", "schedule", "workflow_dispatch"},
            f"unsupported GitHub event: {event!r}")
    require(dispatch_scope in {"fast", "full"},
            f"unsupported manual matrix scope: {dispatch_scope!r}")
    if event == "pull_request":
        require(changed_paths is not None, "pull-request planning requires changed paths")
        if is_documentation_only(changed_paths):
            return "fast", "documentation-only pull request", matrix("fast", source_qualification=False)
        return "full", "pull request changes executable or unclassified inputs", matrix("full", source_qualification=True)
    if event == "push":
        return "fast", "integrated main checkpoint", matrix("fast", source_qualification=True)
    if event == "schedule":
        return "full", "scheduled native host sweep", matrix("full", source_qualification=True)
    scope = dispatch_scope
    return scope, f"manual {scope} host qualification", matrix(scope, source_qualification=True)


def write_github_output(path: Path, *, scope: str, reason: str, value: dict[str, list[dict[str, object]]]) -> None:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
    require("\n" not in encoded and "\r" not in encoded, "matrix output is not single-line JSON")
    with path.open("a", encoding="utf-8") as stream:
        stream.write(f"matrix={encoded}\n")
        stream.write(f"scope={scope}\n")
        stream.write(f"reason={reason}\n")


def write_github_summary(path: Path, *, scope: str, reason: str, value: dict[str, list[dict[str, object]]]) -> None:
    host_names = ", ".join(entry["name"] for entry in value["include"])
    with path.open("a", encoding="utf-8") as stream:
        stream.write("## Host-test policy\n\n")
        stream.write(f"- Scope: `{scope}`\n")
        stream.write(f"- Reason: {reason}\n")
        stream.write(f"- Native hosts: {host_names}\n")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", required=True)
    parser.add_argument("--changed-paths", type=Path)
    parser.add_argument("--dispatch-scope", default="full")
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--github-summary", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        paths = (
            read_changed_paths(args.changed_paths)
            if args.event == "pull_request" and args.changed_paths
            else None
        )
        scope, reason, value = plan(
            event=args.event,
            changed_paths=paths,
            dispatch_scope=args.dispatch_scope,
        )
        if args.github_output:
            write_github_output(args.github_output, scope=scope, reason=reason, value=value)
        if args.github_summary:
            write_github_summary(args.github_summary, scope=scope, reason=reason, value=value)
        print(json.dumps({"scope": scope, "reason": reason, "matrix": value}, sort_keys=True))
    except PolicyError as error:
        print(f"::error::CI platform matrix policy failed: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
