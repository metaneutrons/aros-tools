#!/usr/bin/env python3
"""Counter-probes for the fail-closed CI host-matrix planner."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("ci_matrix", ROOT / "scripts/plan_ci_platform_matrix.py")
planner = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(planner)


class PlatformMatrixTests(unittest.TestCase):
    def test_documentation_only_pr_is_linux_only_and_source_independent(self):
        scope, reason, value = planner.plan(
            event="pull_request",
            changed_paths=(
                "README.md",
                "HANDOFF.md",
                "docs-site/src/content/docs/index.mdx",
                "docs/notes.md",
            ),
            dispatch_scope="full",
        )
        self.assertEqual(scope, "fast")
        self.assertEqual(reason, "documentation-only pull request")
        self.assertEqual(value, {"include": [{
            "name": "linux-x86_64", "runner": "ubuntu-24.04", "source_qualification": False,
        }]})

    def test_every_non_documentation_or_unclassified_pr_uses_all_native_hosts(self):
        for paths in (
            ("crates/aros-toolchain/src/lib.rs",),
            (".github/workflows/ci.yml",),
            ("contracts/aros-source-v1.toml",),
            ("LICENSE",),
            ("new-file-without-policy",),
        ):
            with self.subTest(paths=paths):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=paths, dispatch_scope="fast"
                )
                self.assertEqual(scope, "full")
                self.assertEqual([entry["name"] for entry in value["include"]], [
                    "linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64",
                ])
                self.assertTrue(value["include"][0]["source_qualification"])
                self.assertFalse(any(entry["source_qualification"] for entry in value["include"][1:]))

    def test_integrated_push_and_explicit_manual_selection_preserve_source_qualification(self):
        scope, _, value = planner.plan(event="push", changed_paths=None, dispatch_scope="full")
        self.assertEqual(scope, "fast")
        self.assertTrue(value["include"][0]["source_qualification"])
        for scope in ("fast", "full"):
            with self.subTest(scope=scope):
                actual_scope, _, actual = planner.plan(
                    event="workflow_dispatch", changed_paths=None, dispatch_scope=scope
                )
                self.assertEqual(actual_scope, scope)
                self.assertTrue(actual["include"][0]["source_qualification"])

    def test_scheduled_sweep_is_full(self):
        scope, reason, value = planner.plan(event="schedule", changed_paths=None, dispatch_scope="fast")
        self.assertEqual(scope, "full")
        self.assertEqual(reason, "scheduled native host sweep")
        self.assertEqual(len(value["include"]), 4)

    def test_empty_malformed_or_unsafe_path_input_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "paths"
            for payload in ("", "../outside\n", "docs-site/a\ndocs-site/a\n", "/absolute\n"):
                with self.subTest(payload=payload):
                    path.write_text(payload)
                    with self.assertRaises(planner.PolicyError):
                        planner.read_changed_paths(path)
            target = Path(directory) / "target"
            target.write_text("README.md\n")
            link = Path(directory) / "link"
            link.symlink_to(target)
            with self.assertRaises(planner.PolicyError):
                planner.read_changed_paths(link)

    def test_output_is_single_line_and_has_no_untrusted_values(self):
        value = planner.matrix("fast", source_qualification=False)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            summary = Path(directory) / "summary"
            planner.write_github_output(output, scope="fast", reason="documentation-only pull request", value=value)
            planner.write_github_summary(summary, scope="fast", reason="documentation-only pull request", value=value)
            self.assertEqual(output.read_text().splitlines()[0].split("=", 1)[0], "matrix")
            self.assertIn("linux-x86_64", summary.read_text())


if __name__ == "__main__":
    unittest.main()
