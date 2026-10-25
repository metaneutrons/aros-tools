#!/usr/bin/env python3
"""Every job downstream of native-ab must survive native-ab being skipped.

native-ab runs only when the classifier requires independent A/B
qualification. A job that lists it among its transitive ancestors and has no
status function in its condition is skipped together with it, which silently
dropped signing and publication of every release without A/B (v0.6.3).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

SKIPPABLE = "native-ab"


def jobs(workflow: str) -> dict[str, str]:
    section = workflow.split("\njobs:\n", 1)[1]
    found = re.finditer(
        r"(?ms)^  (?P<name>[A-Za-z0-9_-]+):\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
        section,
    )
    return {match.group("name"): match.group("body") for match in found}


def needs(body: str) -> list[str]:
    match = re.search(r"(?m)^    needs: \[(?P<list>[^\]]*)\]$", body)
    if match:
        return [item.strip() for item in match.group("list").split(",") if item.strip()]
    single = re.search(r"(?m)^    needs: (?P<name>[A-Za-z0-9_-]+)$", body)
    return [single.group("name")] if single else []


def condition(body: str) -> str:
    match = re.search(r"(?ms)^    if: (?P<value>.*?)(?=^    [a-z-]+:)", body)
    return match.group("value") if match else ""


def ancestors(graph: dict[str, list[str]], name: str) -> set[str]:
    seen: set[str] = set()
    stack = list(graph[name])
    while stack:
        current = stack.pop()
        if current not in seen:
            seen.add(current)
            stack.extend(graph.get(current, []))
    return seen


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    definitions = jobs((root / ".github/workflows/release.yml").read_text(encoding="utf-8"))
    graph = {name: needs(body) for name, body in definitions.items()}
    if SKIPPABLE not in definitions:
        print(f"release workflow has no {SKIPPABLE} job", file=sys.stderr)
        return 1
    failures = [
        name
        for name in definitions
        if SKIPPABLE in ancestors(graph, name) and "always()" not in condition(definitions[name])
    ]
    for name in failures:
        print(
            f"release job {name} is downstream of {SKIPPABLE} but its condition has no always()",
            file=sys.stderr,
        )
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
