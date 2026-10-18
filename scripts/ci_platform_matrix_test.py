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
    def assert_hosts(self, value, scope, *, source_qualification):
        names = [entry["name"] for entry in value["include"]]
        expected = ["linux-x86_64"] if scope == "fast" else [
            "linux-x86_64", "linux-aarch64", "macos-aarch64",
        ]
        self.assertEqual(names, expected)
        self.assertEqual(value["include"][0]["source_qualification"], source_qualification)
        self.assertFalse(any(entry["source_qualification"] for entry in value["include"][1:]))

    def test_documentation_only_pr_is_linux_only_and_source_independent(self):
        scope, reason, value = planner.plan(
            event="pull_request",
            changed_paths=(
                "README.md",
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

    def test_policy_only_pr_paths_are_linux_only_without_source_qualification(self):
        paths = (
            ".github/workflows/ci.yml",
            ".github/workflows/release.yml",
            ".github/workflows/codeql.yml",
            "scripts/release/test-release-policy.sh",
            "scripts/release-please/package.json",
            "scripts/plan_ci_platform_matrix.py",
            "scripts/ci_platform_matrix_test.py",
            "scripts/workspace_gate_test.py",
        )
        for path in paths:
            with self.subTest(path=path):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=(path,), dispatch_scope="full"
                )
                self.assertEqual(scope, "fast")
                self.assert_hosts(value, scope, source_qualification=False)

    def test_rust_source_pr_paths_use_all_native_hosts(self):
        for crate in ("aros-transpiler", "aros-verify", "aros-genmodule", "aros-romtool"):
            path = f"crates/{crate}/src/lib.rs"
            with self.subTest(path=path):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=(path,), dispatch_scope="full"
                )
                self.assertEqual(scope, "full")
                self.assert_hosts(value, scope, source_qualification=True)

    def test_documentation_and_policy_mixtures_remain_fast_without_source_qualification(self):
        for paths in (
            ("README.md", ".github/workflows/ci.yml"),
            ("docs/notes.md", "scripts/release/test-release-policy.sh"),
            (".github/workflows/ci.yml", "scripts/workspace_gate_test.py"),
        ):
            with self.subTest(paths=paths):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=paths, dispatch_scope="full"
                )
                self.assertEqual(scope, "fast")
                self.assert_hosts(value, scope, source_qualification=False)

    def test_mixing_rust_with_documentation_or_policy_uses_all_native_hosts(self):
        for paths in (
            ("README.md", "crates/aros-verify/src/lib.rs"),
            ("scripts/plan_ci_platform_matrix.py", "crates/aros-romtool/src/main.rs"),
            ("scripts/workspace_gate_test.py", "crates/aros-toolchain/src/lib.rs"),
            ("scripts/workspace_gate_test.py", "scripts/check-workspace.sh"),
        ):
            with self.subTest(paths=paths):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=paths, dispatch_scope="full"
                )
                self.assertEqual(scope, "full")
                self.assert_hosts(value, scope, source_qualification=True)

    def test_native_shared_and_unclassified_pr_paths_use_all_native_hosts(self):
        for paths in (
            ("crates/aros-toolchain/src/lib.rs",),
            ("crates/aros-collect/src/lib.rs",),
            ("crates/aros-cmake-engine/src/lib.rs",),
            ("crates/aros-common/src/lib.rs",),
            ("crates/aros-board/src/lib.rs",),
            ("crates/aros-macos-disk-claim/src/lib.rs",),
            ("Cargo.toml",),
            ("Cargo.lock",),
            ("rust-toolchain.toml",),
            ("scripts/check-workspace.sh",),
            ("scripts/workspace_gate_test_helper.py",),
            ("scripts/workspace_gate_test.py.bak",),
            (".github/actions/native-build/action.yml",),
            ("contracts/aros-source-v1.toml",),
            ("crates/aros-transpiler/Cargo.toml",),
            ("LICENSE",),
            ("new-file-without-policy",),
            ("README.md", "crates/aros-toolchain/src/lib.rs"),
            ("crates/aros-verify/src/lib.rs", "new-file-without-policy"),
        ):
            with self.subTest(paths=paths):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=paths, dispatch_scope="fast"
                )
                self.assertEqual(scope, "full")
                self.assert_hosts(value, scope, source_qualification=True)

    def test_empty_or_release_version_pr_uses_all_native_hosts(self):
        for paths in ((), (".release-please-manifest.json",)):
            with self.subTest(paths=paths):
                scope, _, value = planner.plan(
                    event="pull_request", changed_paths=paths, dispatch_scope="fast"
                )
                self.assertEqual(scope, "full")
                self.assert_hosts(value, scope, source_qualification=True)

    def test_integrated_push_and_explicit_manual_selection_preserve_source_qualification(self):
        scope, _, value = planner.plan(event="push", changed_paths=("crates/aros-cli/src/main.rs",), dispatch_scope="full")
        self.assertEqual(scope, "fast")
        self.assert_hosts(value, scope, source_qualification=True)
        for scope in ("fast", "full"):
            with self.subTest(scope=scope):
                actual_scope, _, actual = planner.plan(
                    event="workflow_dispatch", changed_paths=None, dispatch_scope=scope
                )
                self.assertEqual(actual_scope, scope)
                self.assert_hosts(actual, actual_scope, source_qualification=True)

    def test_release_merge_push_satisfies_release_admission_host_contract(self):
        spec = importlib.util.spec_from_file_location(
            "release_automation", ROOT / "scripts/release/release-automation.py"
        )
        automation = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(automation)
        scope, _, value = planner.plan(
            event="push", changed_paths=tuple(automation.FILES), dispatch_scope="fast"
        )
        self.assertEqual(scope, "full")
        produced = {f"Tests ({host['name']})" for host in value["include"]}
        required = {name for name in automation.REQUIRED_JOBS["ci.yml"] if name.startswith("Tests (")}
        self.assertEqual(produced, required)
        self.assert_hosts(value, scope, source_qualification=True)

    def test_push_without_paths_fails_closed_and_empty_push_uses_all_native_hosts(self):
        with self.assertRaises(planner.PolicyError):
            planner.plan(event="push", changed_paths=None, dispatch_scope="fast")
        scope, _, value = planner.plan(event="push", changed_paths=(), dispatch_scope="fast")
        self.assertEqual(scope, "full")
        self.assert_hosts(value, scope, source_qualification=True)

    def test_scheduled_sweep_is_full(self):
        scope, reason, value = planner.plan(event="schedule", changed_paths=None, dispatch_scope="fast")
        self.assertEqual(scope, "full")
        self.assertEqual(reason, "scheduled native host sweep")
        self.assert_hosts(value, scope, source_qualification=True)

    def test_empty_malformed_or_unsafe_path_input_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "paths"
            for payload in (
                "",
                "../outside\n",
                "./README.md\n",
                "docs//notes.md\n",
                "docs/./notes.md\n",
                "//README.md\n",
                "docs-site/a\ndocs-site/a\n",
                "/absolute\n",
            ):
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
