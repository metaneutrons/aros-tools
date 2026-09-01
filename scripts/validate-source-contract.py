#!/usr/bin/env python3
"""Validate the immutable AROS source contract consumed by CI.

With ``--producer-workflow``, the validator also proves that the pinned
toolchain-producer workflow selects the exact same repository and commit.
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path, PurePosixPath


SHA1 = re.compile(r"^[0-9a-f]{40}$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"source-contract: {message}")


def exact_keys(table: dict[str, object], expected: set[str], label: str) -> None:
    actual = set(table)
    if actual != expected:
        missing = ", ".join(sorted(expected - actual)) or "none"
        unexpected = ", ".join(sorted(actual - expected)) or "none"
        fail(f"{label} keys differ (missing: {missing}; unexpected: {unexpected})")


def required_table(document: dict[str, object], name: str) -> dict[str, object]:
    value = document.get(name)
    if not isinstance(value, dict):
        fail(f"missing [{name}] table")
    return value


def required_string(table: dict[str, object], key: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        fail(f"{key} must be a non-empty string")
    return value


def workflow_value(text: str, name: str) -> str:
    matches = re.findall(
        rf"^\s*{re.escape(name)}:\s*([^#\s]+)\s*(?:#.*)?$",
        text,
        re.MULTILINE,
    )
    if not matches:
        fail(f"producer workflow has no scalar {name}")
    if len(matches) != 1:
        fail(f"producer workflow must have exactly one scalar {name}")
    return matches[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--contract",
        type=Path,
        default=Path("contracts/aros-source-v1.toml"),
    )
    parser.add_argument("--producer-workflow", type=Path)
    args = parser.parse_args()

    with args.contract.open("rb") as source_file:
        document = tomllib.load(source_file)
    exact_keys(document, {"schema_version", "source", "producer"}, "document")
    if document.get("schema_version") != 1:
        fail("schema_version must be exactly 1")

    source = required_table(document, "source")
    producer = required_table(document, "producer")
    exact_keys(source, {"repository", "commit"}, "[source]")
    exact_keys(producer, {"repository", "commit", "workflow"}, "[producer]")
    source_repository = required_string(source, "repository")
    source_commit = required_string(source, "commit")
    producer_repository = required_string(producer, "repository")
    producer_commit = required_string(producer, "commit")
    producer_workflow = required_string(producer, "workflow")

    for label, repository in (
        ("source.repository", source_repository),
        ("producer.repository", producer_repository),
    ):
        if REPOSITORY.fullmatch(repository) is None:
            fail(f"{label} is not an owner/repository name: {repository}")
    for label, commit in (
        ("source.commit", source_commit),
        ("producer.commit", producer_commit),
    ):
        if SHA1.fullmatch(commit) is None:
            fail(f"{label} is not a full lowercase Git commit: {commit}")
    workflow_path = PurePosixPath(producer_workflow)
    if (
        not producer_workflow.startswith(".github/workflows/")
        or ".." in workflow_path.parts
        or workflow_path.suffix not in {".yml", ".yaml"}
        or any(character.isspace() for character in producer_workflow)
    ):
        fail("producer.workflow must safely identify one YAML workflow below .github/workflows")

    if args.producer_workflow is not None:
        workflow = args.producer_workflow.read_text(encoding="utf-8")
        selected_repository = workflow_value(workflow, "AROS_SOURCE_REPOSITORY")
        selected_commit = workflow_value(workflow, "AROS_SOURCE_COMMIT")
        if selected_repository != source_repository:
            fail(
                "producer repository mismatch: "
                f"contract={source_repository}, workflow={selected_repository}"
            )
        if selected_commit != source_commit:
            fail(
                "producer commit mismatch: "
                f"contract={source_commit}, workflow={selected_commit}"
            )

    print(
        "source-contract: valid "
        f"source={source_repository}@{source_commit} "
        f"producer={producer_repository}@{producer_commit}"
    )


if __name__ == "__main__":
    main()
