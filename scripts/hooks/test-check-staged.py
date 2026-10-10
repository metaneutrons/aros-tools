#!/usr/bin/env python3
"""Check source-module admission without weakening staged artifact guards."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


HOOK = Path(__file__).with_name("check-staged.sh").resolve()


class StagedPathTests(unittest.TestCase):
    def probe(self, path: str, accepted: bool) -> None:
        with tempfile.TemporaryDirectory(prefix="aros-staged-path-") as root:
            subprocess.run(
                ["git", "init", "--quiet", "--initial-branch=fix/probe", root],
                check=True,
            )
            subject = Path(root) / path
            subject.parent.mkdir(parents=True, exist_ok=True)
            subject.write_text("// harmless staged-path fixture\n", encoding="utf-8")
            subprocess.run(["git", "-C", root, "add", "--", path], check=True)
            environment = dict(os.environ)
            environment.pop("ALLOW_COMMIT_ON_DEFAULT", None)
            environment.pop("MAX_STAGED_BYTES", None)
            environment.pop("GIT_DIR", None)
            environment.pop("GIT_WORK_TREE", None)
            result = subprocess.run(
                ["sh", str(HOOK)],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0 if accepted else 1, result.stderr)
            if not accepted:
                self.assertIn("Build- oder Abhängigkeitsverzeichnis", result.stderr)

    def test_immediate_rust_build_modules_are_sources(self) -> None:
        for path in (
            "crates/aros-cli/src/build/native_contract.rs",
            "crates/example/src/build/host_inputs.rs",
            "crates/example/src/build/riscv32.rs",
            "crates/example/src/main.rs",
        ):
            with self.subTest(path=path):
                self.probe(path, True)

    def test_artifacts_and_nested_build_files_remain_rejected(self) -> None:
        for path in (
            "build/kernel.elf",
            "crates/example/build/generated.rs",
            "crates/example/src/build/compiler.elf",
            "crates/example/src/build/nested/generated.rs",
            "crates/example/src/build/target/generated.rs",
            "target/generated.rs",
            "docs-site/dist/index.html",
            "docs-site/node_modules/package/index.js",
            ".next/server.js",
            "coverage/report.json",
        ):
            with self.subTest(path=path):
                self.probe(path, False)


if __name__ == "__main__":
    unittest.main()
