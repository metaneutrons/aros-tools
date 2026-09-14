#!/usr/bin/env python3
"""Plan the closed native Homebrew qualification matrix.

The policy is deliberately a release contract, not a temporary runner-cost
exception: Linux x86-64, Linux ARM64 and macOS Apple silicon are the only
supported native Homebrew hosts. Intel macOS is not a release target.
"""

import argparse
import json
from pathlib import Path
import re
import stat
import sys


POLICY = Path(__file__).with_name("homebrew-qualification.json")


class PolicyError(ValueError):
    """A release-host matrix cannot be established."""


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise PolicyError(f"AP7330 duplicate policy field: {key}")
        result[key] = value
    return result


def load_policy(path):
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 16_384:
        raise PolicyError("AP7330 policy must be a regular file of at most 16 KiB")
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_object)


def validate_policy(policy):
    if (not isinstance(policy, dict) or set(policy) != {"schema", "include"}
            or type(policy["schema"]) is not int or policy["schema"] != 1):
        raise PolicyError("AP7330 invalid Homebrew policy schema")
    expected = [
        ("linux-x86_64", "ubuntu-24.04", "x86_64-unknown-linux-gnu"),
        ("linux-aarch64", "ubuntu-24.04-arm", "aarch64-unknown-linux-gnu"),
        ("macos-aarch64", "macos-15", "aarch64-apple-darwin"),
    ]
    rows = policy["include"]
    if (not isinstance(rows, list) or any(
            not isinstance(row, dict) or set(row) != {"name", "runner", "target"}
            for row in rows)
            or [tuple(row[key] for key in ("name", "runner", "target")) for row in rows] != expected):
        raise PolicyError("AP7330 Homebrew matrix must bind three genuine native release hosts")


def plan_matrix(policy, event, ref_type, ref):
    validate_policy(policy)
    is_pr = event == "pull_request" and ref_type == "branch" and re.fullmatch(
        r"refs/pull/[1-9][0-9]*/merge", ref
    )
    is_tag = event in {"push", "workflow_dispatch"} and ref_type == "tag" and ref.startswith("refs/tags/")
    is_manual = event == "workflow_dispatch" and ref_type == "branch" and ref.startswith("refs/heads/")
    if not (is_pr or is_tag or is_manual):
        raise PolicyError("AP7332 unsupported or contradictory event/ref; no matrix was emitted")
    return {
        "matrix": {"include": list(policy["include"])},
        "coverage": "release-hosts",
        "message": "All supported native Homebrew hosts are scheduled; macOS Intel is not a release target.",
    }


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
        plan = plan_matrix(load_policy(args.policy), args.event, args.ref_type, args.ref)
        if args.github_summary:
            with args.github_summary.open("a", encoding="utf-8") as stream:
                stream.write(f"## Homebrew qualification scope\n\n{plan['message']}\n\n")
        if args.github_output:
            with args.github_output.open("a", encoding="utf-8") as stream:
                stream.write(f"matrix={json.dumps(plan['matrix'], separators=(',', ':'))}\n")
                stream.write(f"coverage={plan['coverage']}\n")
        print(json.dumps(plan, separators=(",", ":")))
    except (OSError, ValueError, TypeError, KeyError) as error:
        detail = str(error) if isinstance(error, PolicyError) else f"AP7333 policy/output I/O: {error}"
        print(f"::error::Homebrew matrix: {detail}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
