#!/usr/bin/env python3
"""Synthetic NativeKobj adapter: real ld -r followed by copy-only localization.

This fixture does not inspect collector set files or localize symbols. Its
success proves only the CMake dependency graph and a synthetic host ELF path.
"""

import json
import os
import shutil
import subprocess
import sys


def record(operation, **details):
    log_path = os.environ.get("NATIVE_CORE_COLLECTOR_LOG")
    if log_path:
        with open(log_path, "a", encoding="utf-8") as log:
            log.write(json.dumps({"operation": operation, **details}) + "\n")


def main():
    args = sys.argv[1:]
    if args and args[0] == "--ld":
        try:
            linker = args[1]
            args.index("--report")
            link_args = args[args.index("--") + 1 :]
        except (ValueError, IndexError):
            print("malformed direct-link argv", file=sys.stderr)
            return 91
        if not link_args or link_args[0] != "-r":
            print("fixture expects NativeKobj's direct -r link", file=sys.stderr)
            return 92
        result = subprocess.run([linker, *link_args], check=False)
        if result.returncode == 0:
            record("--ld", args=link_args)
        return result.returncode

    if args and args[0] == "--localize-kobj":
        if len(args) < 2:
            print("missing synthetic localization stage", file=sys.stderr)
            return 93
        stage = args[1]
        copied_stage = f"{stage}.fixture-copy"
        try:
            shutil.copy2(stage, copied_stage)
            os.replace(copied_stage, stage)
        except OSError as error:
            try:
                os.unlink(copied_stage)
            except FileNotFoundError:
                pass
            print(f"cannot copy synthetic localization stage: {error}", file=sys.stderr)
            return 94
        record("--localize-kobj", stage=stage)
        return 0

    print("unknown fixture collector operation", file=sys.stderr)
    return 95


if __name__ == "__main__":
    sys.exit(main())
