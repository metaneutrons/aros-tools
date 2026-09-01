#!/usr/bin/env python3
"""Classify a stable release for independent A/B producer qualification.

The classifier is deliberately fail closed: only a small, explicit set of
application-source and documentation changes can use the single-producer path.
Unknown paths, malformed history and release/build-graph changes require A/B.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any


STABLE = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def run(*arguments: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(arguments, check=check, text=True, capture_output=True)


def result(required: bool, reason: str, previous: str = "") -> None:
    print(
        json.dumps(
            {"requires_ab": required, "reason": reason, "previous_tag": previous},
            sort_keys=True,
            separators=(",", ":"),
        )
    )


def releases(repository: str, fixture: Path | None) -> list[dict[str, Any]]:
    if fixture is not None:
        if os.environ.get("AROS_RELEASE_POLICY_FIXTURE") != "1":
            raise RuntimeError("fixture history is permitted only in policy tests")
        value: Any = json.loads(fixture.read_text(encoding="utf-8"))
    else:
        response = run(
            "gh",
            "api",
            "-H",
            "X-GitHub-Api-Version: 2026-03-10",
            "--paginate",
            "--slurp",
            f"repos/{repository}/releases?per_page=100",
        )
        value = json.loads(response.stdout)
    flattened: list[dict[str, Any]] = []
    pages = value if isinstance(value, list) else []
    for page in pages:
        if isinstance(page, list):
            flattened.extend(item for item in page if isinstance(item, dict))
        elif isinstance(page, dict):
            flattened.append(page)
    return flattened


def git_text(*arguments: str) -> str:
    return run("git", *arguments).stdout


def toml_at(revision: str, path: str) -> dict[str, Any]:
    return tomllib.loads(git_text("show", f"{revision}:{path}"))


def normalize_manifest(document: dict[str, Any]) -> dict[str, Any]:
    copied = json.loads(json.dumps(document))
    workspace = copied.get("workspace")
    if isinstance(workspace, dict):
        package = workspace.get("package")
        if isinstance(package, dict) and "version" in package:
            package["version"] = "<release-version>"
    package = copied.get("package")
    if isinstance(package, dict) and "version" in package:
        package["version"] = "<release-version>"
    return copied


def normalize_lock(document: dict[str, Any]) -> dict[str, Any]:
    copied = json.loads(json.dumps(document))
    packages = copied.get("package")
    if isinstance(packages, list):
        for package in packages:
            if isinstance(package, dict) and "source" not in package and "version" in package:
                package["version"] = "<release-version>"
                dependencies = package.get("dependencies")
                if isinstance(dependencies, list):
                    package["dependencies"] = [
                        re.sub(
                            r"^(aros-[A-Za-z0-9_-]+)\s+[0-9]+\.[0-9]+\.[0-9]+(?=\s|$)",
                            r"\1 <release-version>",
                            dependency,
                        )
                        if isinstance(dependency, str)
                        else dependency
                        for dependency in dependencies
                    ]
    return copied


def metadata_only(previous: str, source: str, path: str) -> bool:
    try:
        before = toml_at(previous, path)
        after = toml_at(source, path)
    except (subprocess.CalledProcessError, tomllib.TOMLDecodeError):
        return False
    if path == "Cargo.lock":
        return normalize_lock(before) == normalize_lock(after)
    return normalize_manifest(before) == normalize_manifest(after)


def low_risk_path(path: str) -> bool:
    if path in {
        ".editorconfig",
        ".gitattributes",
        ".gitignore",
        ".release-please-manifest.json",
        "CHANGELOG.md",
        "CODE_OF_CONDUCT.md",
        "CONTRIBUTING.md",
        "README.md",
        "SECURITY.md",
        "release-please-config.json",
    }:
        return True
    if path.startswith("docs-site/"):
        return True
    if re.fullmatch(r"LICENSE(?:-[A-Za-z0-9._-]+)?", path):
        return True
    if re.fullmatch(r"crates/[^/]+/(?:README\.md|tests/.+)", path):
        return True
    if re.fullmatch(r"crates/[^/]+/src/.+\.rs", path):
        return not (
            path.startswith("crates/aros-release/")
            or path == "crates/aros-common/src/publication.rs"
            or path.startswith("crates/aros-common/src/publication/")
        )
    return False


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--releases-json", type=Path)
    arguments = parser.parse_args()

    match = STABLE.fullmatch(arguments.tag)
    if match is None:
        result(False, "not-a-stable-release")
        return
    version = tuple(int(part) for part in match.groups())
    if version[2] == 0:
        result(True, "major-or-minor-boundary")
        return

    try:
        history = releases(arguments.repository, arguments.releases_json)
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        result(True, f"release-history-unavailable:{type(error).__name__}")
        return
    candidates: list[tuple[tuple[int, int, int], str]] = []
    seen: set[str] = set()
    for release in history:
        tag = release.get("tag_name")
        previous = STABLE.fullmatch(tag) if isinstance(tag, str) else None
        if previous is None or tag == arguments.tag:
            continue
        if tag in seen:
            result(True, "ambiguous-release-history")
            return
        seen.add(tag)
        previous_version = tuple(int(part) for part in previous.groups())
        if (
            previous_version < version
            and release.get("draft") is False
            and release.get("prerelease") is False
            and release.get("immutable") is True
        ):
            candidates.append((previous_version, tag))
    if not candidates:
        result(True, "first-or-untrusted-stable-release")
        return
    previous_tag = max(candidates)[1]

    try:
        if git_text("cat-file", "-t", f"refs/tags/{previous_tag}").strip() != "tag":
            result(True, "untrusted-previous-tag", previous_tag)
            return
        previous_commit = git_text("rev-parse", f"refs/tags/{previous_tag}^{{}}").strip()
        ancestor = run(
            "git", "merge-base", "--is-ancestor", previous_commit, arguments.source_commit,
            check=False,
        )
        if ancestor.returncode != 0:
            result(True, "previous-release-not-ancestor", previous_tag)
            return
        tag_message = git_text("for-each-ref", "--format=%(contents)", f"refs/tags/{arguments.tag}")
        if "AROS-Release-Qualification: full-ab" in tag_message.splitlines():
            result(True, "explicit-tag-policy", previous_tag)
            return
        paths = [
            line
            for line in git_text(
                "diff", "--name-only", previous_commit, arguments.source_commit, "--"
            ).splitlines()
            if line
        ]
    except subprocess.CalledProcessError:
        result(True, "unverifiable-git-history", previous_tag)
        return
    if not paths:
        result(True, "empty-release-diff", previous_tag)
        return

    for path in paths:
        if path == "Cargo.lock" or path == "Cargo.toml" or path.endswith("/Cargo.toml"):
            if not metadata_only(previous_commit, arguments.source_commit, path):
                result(True, f"build-graph-change:{path}", previous_tag)
                return
            continue
        if not low_risk_path(path):
            result(True, f"unclassified-change:{path}", previous_tag)
            return
    result(False, "closed-low-risk-patch", previous_tag)


if __name__ == "__main__":
    main()
