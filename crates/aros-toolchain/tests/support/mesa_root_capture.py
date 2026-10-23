"""Capture exactly one native leaf invocation; never execute Make."""

import json
import os
from pathlib import Path
import sys

root = Path(os.environ["AROS_TEST_ROOT_CAPTURE"])
assert root.is_absolute() and root.resolve() == root
descriptor = os.open(root / "capture.json", os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(descriptor, "w", encoding="utf-8") as output:
    json.dump({"argv": sys.argv[1:], "cwd": os.getcwd(), "environment": dict(os.environ)}, output, sort_keys=True)
