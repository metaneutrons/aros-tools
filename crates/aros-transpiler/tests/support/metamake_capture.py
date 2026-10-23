"""Private fixture helper: record one invocation; never launch another tool."""

import json
import os
import sys

directory = os.environ["AROS_TEST_CAPTURE_DIRECTORY"]
for index in range(8):
    path = os.path.join(directory, f"invocation-{index}.json")
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        continue
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        json.dump(
            {"argv": sys.argv[1:], "cwd": os.getcwd(), "environment": dict(os.environ)},
            output,
            sort_keys=True,
        )
    break
else:
    raise SystemExit("private capture invocation bound exceeded")
