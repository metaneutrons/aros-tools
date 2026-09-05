#!/usr/bin/env python3
"""Wait at most five minutes for tap checks to register on the exact PR head.

An empty rollup is pending, never success. Authentication, transport, schema,
head changes and completed failures are not a registration delay. The workflow
then watches the registered checks and revalidates governance before merging.
"""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
TAP = "metaneutrons/homebrew-tap"


class CheckError(Exception):
    """Tap qualification cannot safely continue."""


def snapshot(pr):
    try:
        result = subprocess.run(
            ["gh", "pr", "view", pr, "--repo", TAP, "--json",
             "state,isDraft,headRefOid,statusCheckRollup"],
            capture_output=True, timeout=30, check=False)
        if result.returncode != 0 or len(result.stdout) > 4 * 1024 * 1024:
            raise CheckError("AP7312 cannot read tap checks; no API error is treated as pending")
        return json.loads(result.stdout)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        raise CheckError("AP7312 tap check response is unavailable or malformed") from error


def registered(data, head, required):
    if (not isinstance(data, dict) or data.get("headRefOid") != head
            or data.get("state") != "OPEN" or data.get("isDraft") is not False):
        raise CheckError("AP7310 tap PR head or reviewable state changed")
    rows = data.get("statusCheckRollup")
    if not isinstance(rows, list):
        raise CheckError("AP7312 tap check inventory is malformed")
    names = set()
    for row in rows:
        if not isinstance(row, dict):
            raise CheckError("AP7312 tap check entry is malformed")
        if row.get("__typename") == "CheckRun":
            name, state, conclusion = row.get("name"), row.get("status"), row.get("conclusion")
            if state not in ("QUEUED", "IN_PROGRESS", "PENDING", "WAITING", "REQUESTED", "COMPLETED"):
                raise CheckError("AP7312 unknown tap check state")
            if state == "COMPLETED" and conclusion not in ("SUCCESS", "NEUTRAL", "SKIPPED"):
                raise CheckError("AP7313 a tap check failed before registration completed")
        elif row.get("__typename") == "StatusContext":
            name, state = row.get("context"), row.get("state")
            if state not in ("PENDING", "SUCCESS", "FAILURE", "ERROR"):
                raise CheckError("AP7312 unknown tap status context")
            if state in ("FAILURE", "ERROR"):
                raise CheckError("AP7313 a tap status failed before registration completed")
        else:
            raise CheckError("AP7312 unknown tap check type")
        if not isinstance(name, str) or not name:
            raise CheckError("AP7312 tap check has no name")
        names.add(name)
    return required <= names


def wait(pr, head, required, *, read=snapshot, clock=time.monotonic, sleep=time.sleep):
    if not re.fullmatch(r"[0-9a-f]{40}", head) or not required:
        raise CheckError("AP7311 exact head and required check contract are mandatory")
    deadline = clock() + 300
    while clock() < deadline:
        if registered(read(pr), head, required):
            return
        remaining = deadline - clock()
        if remaining > 0:
            sleep(min(20, remaining))
    raise CheckError("AP7314 required tap checks did not register within 300 seconds")


def main():
    with (ROOT / "contracts/repository-governance-v1.toml").open("rb") as stream:
        policy = tomllib.load(stream)["repositories"][TAP]
    required = {row["context"] for row in policy["required_status_checks"]}
    wait(os.environ["PR"], os.environ["EXPECTED_HEAD"], required)
    print("Required tap checks registered on the expected PR head; qualification continues.")


if __name__ == "__main__":
    try:
        main()
    except (CheckError, OSError, KeyError, ValueError, TypeError) as error:
        print(f"::error::{error}", file=sys.stderr)
        sys.exit(1)
