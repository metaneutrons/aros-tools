#!/usr/bin/env python3
import json
import os
import sys


args = sys.argv[1:]
log_path = os.environ.get("NATIVE_KOBJ_LOG")
if not log_path:
    print("NATIVE_KOBJ_LOG is required", file=sys.stderr)
    sys.exit(90)

with open(log_path, "a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\n")

if args and args[0] == "--ld":
    try:
        report_path = args[args.index("--report") + 1]
        link_args = args[args.index("--") + 1 :]
        output_path = link_args[link_args.index("-o") + 1]
    except (ValueError, IndexError):
        print("malformed direct-link argv", file=sys.stderr)
        sys.exit(91)
    report_content = os.environ.get("NATIVE_KOBJ_REPORT_CONTENT")
    if report_content is None:
        try:
            os.remove(report_path)
        except FileNotFoundError:
            pass
    else:
        with open(report_path, "w", encoding="utf-8") as report:
            report.write(report_content)
    with open(output_path, "wb") as output:
        output.write(b"native-kobj-link-stage\n")
elif args and args[0] == "--localize-kobj":
    if os.environ.get("NATIVE_KOBJ_FAIL_LOCALIZE") == "1":
        print("requested fixture localization failure", file=sys.stderr)
        sys.exit(42)
    try:
        with open(args[1], "ab") as output:
            output.write(b"localized\n")
    except (IndexError, OSError) as error:
        print(f"cannot update localization stage: {error}", file=sys.stderr)
        sys.exit(92)
else:
    print("unknown fake collector operation", file=sys.stderr)
    sys.exit(93)
