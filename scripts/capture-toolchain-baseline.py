#!/usr/bin/env python3
"""Replay a trusted historical producer; print, never overwrite, a golden vector.

This developer-only tool executes the explicitly selected producer's Python
functions. Use a reviewed, isolated checkout, never untrusted PR code. It does
not fetch, run compilers, install packages or qualify release artifacts.
"""

from __future__ import annotations

import argparse
import ast
import base64
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "scripts/fixtures/toolchain-producer"
TREE = ROOT / "crates/aros-cli/tests/fixtures/tree-digest-v1.fixture.json"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def changed_manifest(base: dict, changes: list[dict]) -> dict:
    result = copy.deepcopy(base)
    for change in changes:
        destination = result
        for key in change["path"][:-1]:
            destination = destination[key]
        destination[change["path"][-1]] = copy.deepcopy(change["value"])
    return result


def capture(producer: Path) -> dict:
    def git(*args: str) -> str:
        return subprocess.check_output(
            ["git", "-C", str(producer), *args], text=True, timeout=30
        ).strip()

    if git("status", "--porcelain", "--untracked-files=all"):
        raise ValueError("baseline producer checkout must be clean")
    commit = git("rev-parse", "HEAD")
    source = producer / "scripts/toolchain/producer.py"
    spec = importlib.util.spec_from_file_location("legacy_producer", source)
    if spec is None or spec.loader is None:
        raise ValueError("cannot load selected producer")
    sys.dont_write_bytecode = True
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    tree = json.loads(TREE.read_text(encoding="utf-8"))
    if TREE.read_bytes() != (producer / "toolchains/tree-digest-v1.fixture.json").read_bytes():
        raise ValueError("shared tree fixture differs across repositories")

    epoch = 946684800
    manifest = {
        "schema": 1,
        "release_id": "fixture-only",
        "host": "linux-x86_64",
        "target_profile": "pc-x86_64",
        "target_triple": "x86_64-unknown-aros",
        "tree_sha256": tree["tree_sha256"],
        "llvm_version": "11.0.0",
        "recipe_sha256": "6" * 64,
        "source_lock_sha256": "4" * 64,
        "profiles_sha256": "5" * 64,
        "source_commit": "1" * 40,
        "producer_commit": "2" * 40,
        "tools_commit": "3" * 40,
        "source_date_epoch": epoch,
        "capabilities": ["c"],
        "build_environment": {},
        "files": tree["entries"],
    }
    module.validate_manifest(manifest)
    manifest_bytes = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
    with tempfile.TemporaryDirectory(prefix="tcp-vector-") as temporary:
        root = Path(temporary) / "payload"
        root.mkdir()
        for entry in tree["entries"]:
            path = root / entry["path"]
            if entry["type"] == "directory":
                path.mkdir()
            elif entry["type"] == "symlink":
                path.symlink_to(entry["target"])
            else:
                path.write_bytes(tree["file_content_utf8"].encode())
        inventory, measured_tree = module.tree_inventory(root)
        if inventory != tree["entries"] or measured_tree != tree["tree_sha256"]:
            raise ValueError("legacy inventory differs from the shared vector")
        (root / "toolchain-manifest.json").write_bytes(manifest_bytes)
        for path in (root, *root.rglob("*")):
            if not path.is_symlink():
                path.chmod(0o755 if path.is_dir() else 0o644)
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:xz", format=tarfile.PAX_FORMAT, preset=9) as archive:
            module.add_tar_entry(archive, root, root, epoch)
            for path in sorted(root.rglob("*"), key=lambda value: value.relative_to(root).as_posix()):
                module.add_tar_entry(archive, root, path, epoch)
        archive_bytes = stream.getvalue()

    cases = json.loads((FIXTURES / "manifest-cases.json").read_text(encoding="utf-8"))
    for case in cases["cases"]:
        try:
            module.validate_manifest(changed_manifest(manifest, case["changes"]))
            actual = "accept"
        except SystemExit:
            actual = "reject"
        if actual != case["legacy"]:
            raise ValueError(f"legacy behavior changed for {case['name']}: {actual}")
    canonical = module.json_bytes(tree["entries"][2])
    return {
        "schema": "aros-toolchain-package-baseline-v1",
        "scope": "synthetic-envelope-not-an-installable-toolchain",
        "producer_commit": commit,
        "producer_script_sha256": digest(source.read_bytes()),
        "producer_functions": [
            item.name for item in ast.parse(source.read_text(encoding="utf-8")).body
            if isinstance(item, ast.FunctionDef)
        ],
        "tree_fixture_sha256": digest(TREE.read_bytes()),
        "manifest": manifest,
        "manifest_sha256": digest(manifest_bytes),
        "canonical_entry_utf8": canonical.decode(),
        "canonical_entry_sha256": digest(canonical),
        "archive_base64": base64.b64encode(archive_bytes).decode(),
        "archive_sha256": digest(archive_bytes),
        "archive_size": len(archive_bytes),
        "legacy_manifest_cases": len(cases["cases"]),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--producer-dir", type=Path, required=True)
    parser.add_argument("--check", action="store_true", help="compare without updating the golden file")
    args = parser.parse_args()
    try:
        measured = capture(args.producer_dir.resolve())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"producer baseline: {error}\n")
    if args.check:
        expected = json.loads((FIXTURES / "package-v1.json").read_text(encoding="utf-8"))
        if measured != expected:
            raise SystemExit("producer baseline differs; inspect it, do not automatically update the golden")
        print(f"producer baseline: exact vector and {measured['legacy_manifest_cases']} legacy cases passed")
    else:
        print(json.dumps(measured, sort_keys=True, indent=2))


if __name__ == "__main__":
    main()
