#!/usr/bin/env python3
"""Offline contracts for the closed three-host Homebrew release matrix."""

import ast
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/release/homebrew-matrix.py"
spec = importlib.util.spec_from_file_location("homebrew_matrix", SCRIPT)
matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matrix)


def release_gate(job, coverage, homebrew="success", is_release="true"):
    """Evaluate the actual release coverage gate with a closed expression model."""
    workflow = (ROOT / ".github/workflows/release.yml").read_text()
    block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n    needs:", 1)[0]
    expression = block.split("    if: >-\n", 1)[1].strip()

    def value(match):
        key = match.group()
        if key == "needs.metadata.outputs.homebrew_coverage":
            return repr(coverage)
        if key == "needs.homebrew.result":
            return repr(homebrew)
        if key == "needs.metadata.outputs.is_release":
            return repr(is_release)
        if key == "needs.metadata.outputs.is_stable":
            return repr("true")
        if key.endswith(".result"):
            return repr("success")
        raise AssertionError(f"unmodelled gate field: {key}")

    expression = re.sub(r"needs\.[a-z-]+\.(?:outputs\.[a-z_]+|result)", value, expression)
    expression = " ".join(expression.replace("always()", "True").split())
    expression = expression.replace("&&", "and").replace("||", "or")
    tree = ast.parse(expression, mode="eval")
    allowed = (ast.Expression, ast.BoolOp, ast.And, ast.Or, ast.Compare,
               ast.Eq, ast.NotEq, ast.Constant)
    if any(not isinstance(node, allowed) for node in ast.walk(tree)):
        raise AssertionError("unmodelled workflow condition syntax")
    return eval(compile(tree, "workflow-condition", "eval"), {"__builtins__": {}})


class MatrixPolicy(unittest.TestCase):
    def setUp(self):
        self.policy = matrix.load_policy(matrix.POLICY)

    def plan(self, event="pull_request", ref_type="branch", ref="refs/pull/39/merge"):
        return matrix.plan_matrix(self.policy, event, ref_type, ref)

    def test_current_policy_is_exact_and_permanent(self):
        matrix.validate_policy(self.policy)
        self.assertEqual(len(self.policy["include"]), 3)
        self.assertNotIn("macos-x86_64", [row["name"] for row in self.policy["include"]])

    def test_every_valid_context_gets_the_same_release_hosts(self):
        for event, ref_type, ref in (
            ("pull_request", "branch", "refs/pull/39/merge"),
            ("push", "tag", "refs/tags/v0.2.0"),
            ("push", "tag", "refs/tags/v0.2.0-rc.1"),
            ("workflow_dispatch", "tag", "refs/tags/v0.2.0"),
            ("workflow_dispatch", "branch", "refs/heads/main"),
        ):
            with self.subTest(event=event, ref=ref):
                plan = self.plan(event, ref_type, ref)
                self.assertEqual(plan["coverage"], "release-hosts")
                self.assertEqual(plan["matrix"]["include"], self.policy["include"])
                self.assertIn("not a release target", plan["message"])

    def test_contradictory_and_unknown_contexts_fail_closed(self):
        for event, ref_type, ref in (
            ("pull_request", "tag", "refs/tags/v0.2.0"),
            ("pull_request", "branch", "refs/heads/main"),
            ("pull_request", "branch", "refs/pull/0/merge"),
            ("pull_request_target", "branch", "refs/heads/main"),
            ("push", "branch", "refs/heads/main"),
            ("workflow_dispatch", "branch", "refs/pull/39/merge"),
            ("", "", ""),
        ):
            with self.subTest(event=event, ref=ref), self.assertRaisesRegex(matrix.PolicyError, "AP7332"):
                self.plan(event, ref_type, ref)

    def test_matrix_cannot_drop_duplicate_or_relabel_a_release_host(self):
        rows = self.policy["include"]
        invalid = [None, [], rows[:-1], rows + rows[:1],
                   rows[:2] + [dict(rows[2], runner="macos-14")],
                   [dict(rows[0], **{"continue-on-error": True})] + rows[1:]]
        for candidate in invalid:
            with self.subTest(candidate=candidate), self.assertRaisesRegex(matrix.PolicyError, "three genuine native release hosts"):
                self.policy["include"] = candidate
                self.plan("push", "tag", "refs/tags/v0.2.0")

    def test_unknown_schema_and_legacy_exception_fail_closed(self):
        for policy in (None, [], dict(self.policy, schema=True), dict(self.policy, schema=2),
                       dict(self.policy, pr_exception=None)):
            with self.subTest(policy=policy), self.assertRaisesRegex(matrix.PolicyError, "AP7330"):
                matrix.validate_policy(policy)

    def test_publication_gates_require_complete_release_host_coverage(self):
        for job in ("release-config-preflight", "channel-preflight", "publish"):
            with self.subTest(job=job):
                self.assertTrue(release_gate(job, "release-hosts"))
                for coverage in ("", "release-hosts\n", "three-hosts-pr-exception", "unknown"):
                    self.assertFalse(release_gate(job, coverage))
                for result in ("skipped", "cancelled", "failure", ""):
                    self.assertFalse(release_gate(job, "release-hosts", homebrew=result))
                self.assertFalse(release_gate(job, "release-hosts", is_release="false"))

    def test_tools_release_and_ci_do_not_schedule_intel_macos(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertNotIn("macos-15-intel", release)
        self.assertNotIn("x86_64-apple-darwin", release)
        ci_planner = (ROOT / "scripts/plan_ci_platform_matrix.py").read_text()
        self.assertNotIn('"runner": "macos-15-intel"', ci_planner)

    def test_channel_publisher_requires_exactly_the_three_release_payload_urls(self):
        ecosystem = (ROOT / ".github/workflows/publish-ecosystem.yml").read_text()
        homebrew = ecosystem.split("  homebrew:\n", 1)[1].split("\n  aur-publish:\n", 1)[0]
        self.assertIn('grep -Fc "/releases/download/${TAG}/" "$source_formula") != 3', homebrew)
        self.assertIn('formula does not reference three exact staged-release URLs', homebrew)
        self.assertNotIn('grep -Fc "/releases/download/${TAG}/" "$source_formula") != 4', homebrew)
        self.assertNotIn('formula does not reference four exact staged-release URLs', homebrew)


class CommandLine(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-matrix-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.path = self.work / "policy.json"
        self.path.write_text(json.dumps(matrix.load_policy(matrix.POLICY)))
        self.output = self.work / "outputs"
        self.summary = self.work / "summary"

    def run_cli(self, ref_type="branch", ref="refs/pull/39/merge", event="pull_request"):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--policy", str(self.path), "--event", event,
             "--ref-type", ref_type, "--ref", ref, "--github-output", str(self.output),
             "--github-summary", str(self.summary)],
            env=dict(os.environ, SOURCE_DATE_EPOCH="0"), capture_output=True, text=True, timeout=10,
        )

    def rejected(self, marker):
        result = self.run_cli()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(marker, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertFalse(self.output.exists())

    def test_outputs_and_summary_use_closed_release_policy(self):
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        plan = json.loads(result.stdout)
        outputs = dict(line.split("=", 1) for line in self.output.read_text().splitlines())
        self.assertEqual(json.loads(outputs["matrix"]), plan["matrix"])
        self.assertEqual(outputs["coverage"], "release-hosts")
        self.assertEqual(len(plan["matrix"]["include"]), 3)
        self.assertIn("not a release target", self.summary.read_text())

    def test_tag_emits_the_same_three_hosts(self):
        result = self.run_cli("tag", "refs/tags/v0.2.0", "push")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(json.loads(result.stdout)["matrix"]["include"]), 3)
        self.assertIn("coverage=release-hosts\n", self.output.read_text())

    def test_duplicate_json_field_is_not_a_hidden_override(self):
        self.path.write_text('{"schema":1,"schema":2}')
        self.rejected("AP7330 duplicate")

    def test_symlink_and_oversized_policy_are_rejected(self):
        real = self.work / "real.json"
        self.path.rename(real)
        self.path.symlink_to(real)
        self.rejected("AP7330 policy must be a regular file")
        self.path.unlink()
        self.path.write_text(" " * 16_385)
        self.rejected("AP7330 policy must be a regular file")

    def test_missing_summary_destination_cannot_grant_coverage(self):
        self.summary = self.work / "missing" / "summary"
        self.rejected("AP7333")

    def test_malformed_json_and_missing_policy_emit_no_outputs(self):
        self.path.write_text("{")
        self.rejected("AP7333")
        self.path.unlink()
        self.rejected("AP7333")


if __name__ == "__main__":
    unittest.main()
