"""Private synthetic MetaMake effect probe; never launches Make or configure."""

import json
import os
from pathlib import Path
import sys

root = Path(os.environ["AROS_TEST_EFFECT_ROOT"])
assert root.is_absolute() and root.resolve() == root
stage = sys.argv[1]
assert stage in ("globals", "generate", "leaf")
with (root / "effects.jsonl").open("a", encoding="utf-8") as output:
    output.write(json.dumps({"stage": stage, "cwd": os.getcwd(), "argv": sys.argv[2:]}) + "\n")

if stage == "globals":
    assert sys.argv[2:] == []
    with (root / "build/globals.cfg").open("x", encoding="utf-8") as output:
        output.write("PROBE = generated\n")
elif stage == "generate":
    assert len(sys.argv) == 3
    # Native MetaMake owns this temporary list, not a Source command.
    pairs = Path(sys.argv[2]).read_text(encoding="utf-8").splitlines()
    assert len(pairs) == 1
    source, destination = map(Path, pairs[0].split())
    assert source == root / "source/module/mmakefile.src"
    assert destination == root / "build/module/mmakefile"
    assert source.resolve() == source and destination.parent.resolve() == destination.parent
    assert not destination.is_symlink()
    assert not destination.exists() or destination.is_file()
    with destination.open("wb") as output:
        output.write(source.read_bytes())
