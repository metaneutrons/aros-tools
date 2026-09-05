#!/usr/bin/env python3
"""Plan native Homebrew checks; a dated PR exception never grants release coverage.

The reviewed JSON is the matrix/exception SSOT. Dates use the current UTC clock,
never SOURCE_DATE_EPOCH. See docs/homebrew-intel-exception.md for evidence and
removal conditions. No package is installed and no credential is accessed here.
"""

import argparse
from datetime import date, datetime, timezone
import json
from pathlib import Path
import re
import stat
import sys

POLICY = Path(__file__).with_name("homebrew-qualification.json")
DOCUMENTATION = "docs/homebrew-intel-exception.md"


class PolicyError(ValueError):
    """A matrix or temporary-exception contract cannot be established."""


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise PolicyError(f"AP7330 duplicate policy field: {key}")
        result[key] = value
    return result


def load_policy(path):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 16384:
        raise PolicyError("AP7330 policy must be a regular file of at most 16 KiB")
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object)


def validate_policy(policy):
    if (not isinstance(policy, dict) or set(policy) != {"schema", "include", "pr_exception"}
            or type(policy["schema"]) is not int or policy["schema"] != 1):
        raise PolicyError("AP7330 invalid Homebrew policy schema")
    # This closed contract is an independent guard on the editable matrix, not
    # an alternative matrix generator. In particular bare macOS labels are ARM.
    expected = [
        ("linux-x86_64", "ubuntu-24.04", "x86_64-unknown-linux-gnu"),
        ("linux-aarch64", "ubuntu-24.04-arm", "aarch64-unknown-linux-gnu"),
        ("macos-x86_64", "macos-15-intel", "x86_64-apple-darwin"),
        ("macos-aarch64", "macos-15", "aarch64-apple-darwin"),
    ]
    rows = policy["include"]
    if (not isinstance(rows, list) or any(
            not isinstance(row, dict) or set(row) != {"name", "runner", "target"}
            for row in rows)
            or [tuple(row[key] for key in ("name", "runner", "target")) for row in rows] != expected):
        raise PolicyError("AP7330 Homebrew matrix must bind four genuine native hosts")
    exception = policy["pr_exception"]
    if exception is None:
        return
    if (not isinstance(exception, dict)
            or set(exception) != {"id", "host", "starts_on", "expires_on"}
            or exception["id"] != "HB-2026-09-05" or exception["host"] != "macos-x86_64"
            or any(not isinstance(exception[key], str)
                   or re.fullmatch(r"\d{4}-\d{2}-\d{2}", exception[key]) is None
                   for key in ("starts_on", "expires_on"))):
        raise PolicyError("AP7330 only the documented Intel PR exception is allowed")
    try:
        duration = date.fromisoformat(exception["expires_on"]) - date.fromisoformat(exception["starts_on"])
    except ValueError as error:
        raise PolicyError("AP7330 invalid UTC exception dates") from error
    if not 0 < duration.days <= 30:
        raise PolicyError("AP7330 exception must be bounded to at most 30 days")


def plan_matrix(policy, event, ref_type, ref, today):
    validate_policy(policy)
    is_pr = event == "pull_request" and ref_type == "branch" and re.fullmatch(r"refs/pull/[1-9][0-9]*/merge", ref)
    is_tag = event in {"push", "workflow_dispatch"} and ref_type == "tag" and ref.startswith("refs/tags/")
    is_manual = event == "workflow_dispatch" and ref_type == "branch" and ref.startswith("refs/heads/")
    if not (is_pr or is_tag or is_manual):
        raise PolicyError("AP7332 unsupported or contradictory event/ref; no matrix was emitted")
    rows = list(policy["include"])
    coverage = "four-hosts"
    message = "All four native Homebrew hosts are required; installation results are still pending."
    exception = policy["pr_exception"]
    if is_pr and exception is not None:
        starts = date.fromisoformat(exception["starts_on"])
        expires = date.fromisoformat(exception["expires_on"])
        if not starts <= today < expires:
            raise PolicyError(
                f"AP7331 {exception['id']} is outside its UTC window "
                f"[{starts}, {expires}); remove/review it via {DOCUMENTATION}"
            )
        rows = [row for row in rows if row["name"] != exception["host"]]
        coverage = "three-hosts-pr-exception"
        message = (
            f"{exception['id']}: macOS Intel Homebrew is temporarily UNQUALIFIED "
            f"for pull requests, until {expires} 00:00 UTC. Three hosts are scheduled. "
            "Native Intel builds/tests still run. This is NOT release qualification. "
            f"Removal checklist: {DOCUMENTATION}."
        )
    return {"matrix": {"include": rows}, "coverage": coverage, "message": message}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--policy", type=Path, default=POLICY)
    parser.add_argument("--event", required=True)
    parser.add_argument("--ref-type", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--github-summary", type=Path)
    args = parser.parse_args()
    try:
        plan = plan_matrix(load_policy(args.policy), args.event, args.ref_type, args.ref,
                           datetime.now(timezone.utc).date())
        # Write no output on a rejected policy/context. The summary is emitted
        # first: inability to record the exception must not grant coverage.
        if args.github_summary:
            with args.github_summary.open("a", encoding="utf-8") as stream:
                stream.write(f"## Homebrew qualification scope\n\n{plan['message']}\n\n")
        if args.github_output:
            with args.github_output.open("a", encoding="utf-8") as stream:
                stream.write(f"matrix={json.dumps(plan['matrix'], separators=(',', ':'))}\n")
                stream.write(f"coverage={plan['coverage']}\n")
        print(json.dumps(plan, separators=(",", ":")))
        if plan["coverage"] != "four-hosts":
            print(f"::notice::{plan['message']}", file=sys.stderr)
    except (OSError, ValueError, TypeError, KeyError) as error:
        detail = str(error) if isinstance(error, PolicyError) else f"AP7333 policy/output I/O: {error}"
        print(f"::error::Homebrew matrix: {detail}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
