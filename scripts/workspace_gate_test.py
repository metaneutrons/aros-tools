"""Exercise canonical gate routing with inert tools, not product qualification."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class WorkspaceGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repo = self.root / "tools"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.log = self.root / "calls.jsonl"
        self.source = self.root / "source"
        self.source.mkdir()
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("AROS_", "GIT_"))}
        self.env.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
        self.git("init", "--quiet")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "core.hooksPath=/dev/null", "commit", "--allow-empty", "-qm", "fixture")
        self.commit = self.git("rev-parse", "HEAD").stdout.strip()
        self.write("contracts/aros-source-v1.toml", f'[source]\ncommit = "{self.commit}"\n')
        self.gate = self.write("scripts/check-workspace.sh",
                               (ROOT / "scripts/check-workspace.sh").read_text())
        self.fixtures = self.repo / "crates/aros-cmake-engine/engine/tests"
        self.fixtures.mkdir(parents=True)
        for name in ("AhiBuildTest.cmake", "GrubBuildTest.cmake", "NewTest.cmake"):
            (self.fixtures / name).write_text("# inert routing fixture\n")
        # Quality policy is independently tested by its actual fixtures. These
        # inert commands prove mode routing without invoking compilers/network.
        for name in ("check-development-runtimes.py", "check-environment-contract.py"):
            self.write("scripts/" + name, "pass\n")
        self.write("scripts/inert_test.py", "import unittest\nclass Fixture(unittest.TestCase):\n    def test_inert(self):\n        self.assertTrue(True)\n")
        for name in ("check-architecture.sh", "release/check-actions-policy.sh",
                     "release/verify-apt-workflow-contract.sh", "release/test-governance-policy.sh",
                     "release/test-release-policy.sh"):
            self.write("scripts/" + name, "#!/bin/sh\nexit 0\n", executable=True)
        for name in ("test-homebrew-app.py", "test-homebrew-matrix.py", "test-release-please-app.py"):
            self.write("scripts/release/" + name,
                       f"#!{sys.executable}\n", executable=True)
        (self.repo / "docs-site").mkdir()
        self.write("scripts/check-doc-links.py", "pass\n")
        program = f'''#!{sys.executable}
import json
from pathlib import Path
import sys
root = Path({str(self.root)!r})
name = Path(sys.argv[0]).name
with (root / "calls.jsonl").open("a") as stream:
    stream.write(json.dumps([name, *sys.argv[1:]]) + "\\n")
if name == "uname":
    linux = (root / "linux").exists()
    print(("Linux" if linux else "Darwin") if sys.argv[1:] == ["-s"] else ("x86_64" if linux else "arm64"))
if name == "cargo" and sys.argv[1:2] == ["test"] and (root / "fail-rust").exists():
    sys.exit(23)
if name == "cmake" and (root / "fail-engine").exists():
    sys.exit(29)
'''
        # Real git validates the clean source identity. Real Python executes
        # the gate's TOML parser and unittest discovery; only external tooling
        # and the platform selector are inert here.
        for name in ("cargo", "cmake", "clang", "ninja", "uname", "node", "npm",
                     "actionlint", "shellcheck", "ar", "curl", "dpkg-deb", "gpg",
                     "gpgconf", "gpgv", "jq", "rustc", "sha256sum"):
            path = self.bin / name
            path.write_text(program)
            path.chmod(0o755)
        self.env["PATH"] = str(self.bin) + os.pathsep + os.environ["PATH"]

    def write(self, name, text, executable=False):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        if executable:
            path.chmod(0o755)
        return path

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.source), *args],
                              env=self.env, check=True, capture_output=True, text=True, timeout=30)

    def run_gate(self, *args, source=False):
        self.log.unlink(missing_ok=True)
        env = {**self.env}
        if source:
            env["AROS_TEST_SOURCE_ROOT"] = str(self.source)
        return subprocess.run(["bash", str(self.gate), *args], cwd=self.root,
                              env=env, capture_output=True, text=True, timeout=30)

    def calls(self, name):
        records = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [record[1:] for record in records if record[0] == name]

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_default_runs_quality_and_portable_not_products_or_docs(self):
        result = self.run_gate()
        self.assert_ok(result)
        self.assertIn(["fmt", "--all", "--", "--check"], self.calls("cargo"))
        self.assertTrue(any("--exclude" in args for args in self.calls("cargo")))
        self.assertFalse(self.calls("cmake"))
        self.assertFalse(self.calls("npm"))
        self.assertIn("were not executed", result.stdout)

    def test_source_stage_keeps_exact_rust_suite_without_cmake(self):
        result = self.run_gate("source-test", source=True)
        self.assert_ok(result)
        self.assertEqual(self.calls("cargo"), [["test", "--workspace", "--all-features", "--locked"]])
        self.assertFalse(self.calls("cmake"))
        self.assertIn("explicit test/all", result.stdout)

    def test_explicit_integration_discovers_all_fixtures_including_grub(self):
        result = self.run_gate("test", source=True)
        self.assert_ok(result)
        self.assertEqual(len(self.calls("cmake")), 3)
        self.assertIn("GrubBuildTest.cmake", json.dumps(self.calls("cmake")))
        self.assertIn("3 executed, 0 host-qualified omission(s), 3 discovered", result.stdout)
        self.assertFalse(self.calls("npm"))

    def test_explicit_all_keeps_quality_docs_and_complete_integration(self):
        self.assert_ok(self.run_gate("all", source=True))
        self.assertIn(["fmt", "--all", "--", "--check"], self.calls("cargo"))
        self.assertIn(["run", "build"], self.calls("npm"))
        self.assertEqual(len(self.calls("cmake")), 3)

    def test_linux_reports_grub_omission_instead_of_full_host_coverage(self):
        (self.root / "linux").touch()
        result = self.run_gate("test", source=True)
        self.assert_ok(result)
        self.assertEqual(len(self.calls("cmake")), 2)
        self.assertNotIn("GrubBuildTest.cmake", json.dumps(self.calls("cmake")))
        self.assertIn("2 executed, 1 host-qualified omission(s), 3 discovered", result.stdout)

    def test_empty_engine_inventory_cannot_pass_integration(self):
        for fixture in self.fixtures.iterdir():
            fixture.unlink()
        result = self.run_gate("test", source=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no CMake engine contract tests", result.stderr)

    def test_source_mismatch_dirty_and_missing_source_fail_before_test_tools(self):
        self.assertNotEqual(self.run_gate("source-test").returncode, 0)
        self.assertFalse(self.calls("cargo"))
        self.write("contracts/aros-source-v1.toml", '[source]\ncommit = "' + "0" * 40 + '"\n')
        self.assertNotEqual(self.run_gate("test", source=True).returncode, 0)
        self.assertFalse(self.calls("cargo"))
        self.write("contracts/aros-source-v1.toml", f'[source]\ncommit = "{self.commit}"\n')
        (self.source / "untracked").write_text("not qualified\n")
        self.assertNotEqual(self.run_gate("source-test", source=True).returncode, 0)
        self.assertFalse(self.calls("cargo"))

    def test_conflicting_source_and_invalid_modes_fail_before_any_tools(self):
        for mode in ((), ("check",), ("portable-test",), ("unknown",), ("test", "extra")):
            result = self.run_gate(*mode, source=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(self.calls("cargo"))
            self.assertFalse(self.calls("cmake"))

    def test_rust_and_engine_failures_are_not_success_or_omissions(self):
        (self.root / "fail-rust").touch()
        self.assertEqual(self.run_gate("test", source=True).returncode, 23)
        self.assertFalse(self.calls("cmake"))
        (self.root / "fail-rust").unlink()
        (self.root / "fail-engine").touch()
        result = self.run_gate("test", source=True)
        self.assertEqual(result.returncode, 29)
        self.assertNotIn("engine tests passed", result.stdout)

    def test_ci_uses_a_fail_closed_host_planner_and_routes_only_the_source_lane(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        planner = (ROOT / "scripts/plan_ci_platform_matrix.py").read_text()
        self.assertIn("name: Tests (${{ matrix.name }})", workflow)
        self.assertIn("name: Plan host matrix", workflow)
        self.assertIn("needs: platform-plan", workflow)
        self.assertIn("matrix: ${{ fromJSON(needs.platform-plan.outputs.matrix) }}", workflow)
        self.assertIn("scripts/plan_ci_platform_matrix.py", workflow)
        self.assertIn("- cron: '41 3 * * 1'", workflow)
        self.assertIn("options: [fast, full]", workflow)
        self.assertIn("run: scripts/check-workspace.sh portable-test", workflow)
        for name in ("linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64"):
            self.assertIn('"name": "' + name + '"', planner)
        self.assertIn("documentation-only pull request", planner)
        self.assertIn("pull request changes executable or unclassified inputs", planner)
        for event, gate in (("==", "source-test"), ("!=", "test")):
            condition = f"if: matrix.source_qualification && github.event_name {event} 'pull_request'"
            block = workflow.split(condition, 1)[1].split("      - name:", 1)[0]
            self.assertIn("run: scripts/check-workspace.sh " + gate + "\n", block)
            self.assertIn("AROS_TEST_SOURCE_ROOT: ${{ github.workspace }}/aros-source", block)
        self.assertNotIn("continue-on-error", workflow)

    def test_release_qualification_is_explicit_or_tag_addressed(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        trigger = workflow.split("permissions:", 1)[0]
        self.assertNotIn("pull_request:", trigger)
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("tags:", trigger)


if __name__ == "__main__":
    unittest.main()
