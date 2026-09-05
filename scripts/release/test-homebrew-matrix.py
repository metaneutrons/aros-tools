#!/usr/bin/env python3
"""Offline scope, expiry and actual workflow-condition counter-probes."""

import ast
from datetime import date, datetime, timedelta, timezone
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
EXCEPTION = dict(id="HB-2026-09-05", host="macos-x86_64",
                 starts_on="2026-09-05", expires_on="2026-10-05")


def release_gate(job, coverage, homebrew="success", is_release="true"):
    """Evaluate only boolean/string comparisons from the real Actions gate.

    This models the declared if-expression, not a fabricated publisher and not
    a substitute for a tag run. Unknown syntax fails instead of being ignored.
    """
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
        # The dated fixture survives removal of the real exception; it must
        # continue proving that a future exception cannot waive a release.
        self.policy["pr_exception"] = dict(EXCEPTION)

    def plan(self, event="pull_request", ref_type="branch", ref="refs/pull/39/merge",
             today=date(2026, 9, 5)):
        return matrix.plan_matrix(self.policy, event, ref_type, ref, today)

    def test_current_repository_policy_and_documentation(self):
        policy = matrix.load_policy(matrix.POLICY)
        matrix.validate_policy(policy)
        if policy["pr_exception"] is not None:
            documentation = (ROOT / matrix.DOCUMENTATION).read_text()
            self.assertIn(policy["pr_exception"]["id"], documentation)
            self.assertIn(policy["pr_exception"]["expires_on"], documentation)

    def test_pr_omits_exactly_intel_and_marks_incomplete(self):
        plan = self.plan()
        self.assertEqual(plan["coverage"], "three-hosts-pr-exception")
        self.assertEqual(plan["matrix"]["include"],
                         [row for row in self.policy["include"] if row["name"] != "macos-x86_64"])
        self.assertIn("temporarily UNQUALIFIED", plan["message"])
        self.assertIn("NOT release qualification", plan["message"])
        self.assertIn("2026-10-05 00:00 UTC", plan["message"])

    def test_last_valid_day_and_revised_deadline(self):
        for today in (date(2026, 9, 19), date(2026, 10, 4)):
            with self.subTest(today=today):
                self.assertEqual(len(self.plan(today=today)["matrix"]["include"]), 3)

    def test_before_start_at_expiry_and_after_expiry_fail(self):
        for today in (date(2026, 9, 4), date(2026, 10, 5), date(2027, 1, 1)):
            with self.subTest(today=today), self.assertRaisesRegex(matrix.PolicyError, "AP7331"):
                self.plan(today=today)

    def test_tags_prereleases_and_manual_runs_always_require_four(self):
        for event, ref_type, ref in (
            ("push", "tag", "refs/tags/v0.1.1"),
            ("push", "tag", "refs/tags/v0.1.1-rc.1"),
            ("workflow_dispatch", "tag", "refs/tags/v0.1.1"),
            ("workflow_dispatch", "tag", "refs/tags/v0.1.1-rc.1"),
            ("workflow_dispatch", "branch", "refs/heads/fix/initial-release-closure"),
        ):
            for today in (date(2026, 9, 5), date(2026, 10, 5)):
                with self.subTest(event=event, ref=ref, today=today):
                    plan = self.plan(event, ref_type, ref, today)
                    self.assertEqual(plan["coverage"], "four-hosts")
                    self.assertEqual(plan["matrix"]["include"], self.policy["include"])
                    self.assertNotIn("temporarily", plan["message"])

    def test_removing_exception_restores_pr_lane_even_after_expiry(self):
        self.policy["pr_exception"] = None
        plan = self.plan(today=date(2027, 1, 1))
        self.assertEqual(plan["coverage"], "four-hosts")
        self.assertEqual(plan["matrix"]["include"], self.policy["include"])

    def test_contradictory_and_unknown_context_cannot_claim_exception(self):
        for event, ref_type, ref in (
            ("pull_request", "tag", "refs/tags/v0.1.1"),
            ("pull_request", "branch", "refs/heads/main"),
            ("pull_request", "branch", "refs/pull/0/merge"),
            ("pull_request_target", "branch", "refs/heads/main"),
            ("push", "branch", "refs/heads/main"),
            ("workflow_dispatch", "branch", "refs/pull/39/merge"),
            ("", "", ""),
        ):
            with self.subTest(event=event, ref=ref), self.assertRaisesRegex(matrix.PolicyError, "AP7332"):
                self.plan(event, ref_type, ref)

    def test_invalid_or_broader_exceptions_are_rejected(self):
        invalid = [False, [], {}, dict(EXCEPTION, host="linux-x86_64"),
                   dict(EXCEPTION, id="unreviewed"), dict(EXCEPTION, event="workflow_dispatch"),
                   dict(EXCEPTION, expires_on="2026-10-06"),
                   dict(EXCEPTION, expires_on="2026-09-05"),
                   dict(EXCEPTION, starts_on="2026-13-01"),
                   dict(EXCEPTION, expires_on=None),
                   dict(EXCEPTION, expires_on="2026-1005")]
        for exception in invalid:
            with self.subTest(exception=exception), self.assertRaisesRegex(matrix.PolicyError, "AP7330"):
                self.policy["pr_exception"] = exception
                self.plan()

    def test_matrix_cannot_drop_duplicate_or_relabel_a_host(self):
        rows = self.policy["include"]
        invalid = [None, [], rows[:-1], rows + rows[:1],
                   rows[:2] + [dict(rows[2], runner="macos-15")] + rows[3:],
                   [dict(rows[0], **{"continue-on-error": True})] + rows[1:]]
        for candidate in invalid:
            with self.subTest(candidate=candidate), self.assertRaisesRegex(matrix.PolicyError, "four genuine native hosts"):
                self.policy["include"] = candidate
                self.plan(event="push", ref_type="tag", ref="refs/tags/v0.1.1")

    def test_unknown_schema_or_extra_fields_fail_closed(self):
        for policy in (None, [], dict(self.policy, schema=True), dict(self.policy, schema=2),
                       dict(self.policy, release_exception=True)):
            with self.subTest(policy=policy), self.assertRaisesRegex(matrix.PolicyError, "AP7330"):
                matrix.validate_policy(policy)

    def test_actual_publication_gates_need_full_coverage_and_all_success(self):
        for job in ("release-config-preflight", "channel-preflight", "publish"):
            with self.subTest(job=job):
                self.assertTrue(release_gate(job, "four-hosts"))
                for coverage in ("three-hosts-pr-exception", "", "four-hosts\n", "unknown"):
                    self.assertFalse(release_gate(job, coverage))
                for result in ("skipped", "cancelled", "failure", ""):
                    self.assertFalse(release_gate(job, "four-hosts", homebrew=result))
                # Successful non-release qualification never publishes either.
                self.assertFalse(release_gate(job, "four-hosts", is_release="false"))

    def test_other_intel_builds_stay_in_the_workflows(self):
        release = (ROOT / ".github/workflows/release.yml").read_text()
        native = release.split("\n  native:\n")[1].split("\n  native-ab:\n")[0]
        self.assertIn("runner: macos-15-intel", native)
        ci = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("runner: macos-15-intel", ci)


class CommandLine(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-matrix-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.policy = matrix.load_policy(matrix.POLICY)
        today = datetime.now(timezone.utc).date()
        self.policy["pr_exception"] = dict(EXCEPTION,
                                           starts_on=str(today - timedelta(days=1)),
                                           expires_on=str(today + timedelta(days=29)))
        self.path = self.work / "policy.json"
        self.path.write_text(json.dumps(self.policy))
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

    def test_outputs_and_visible_notice_use_real_clock_not_source_epoch(self):
        result = self.run_cli()
        self.assertEqual(result.returncode, 0, result.stderr)
        plan = json.loads(result.stdout)
        outputs = dict(line.split("=", 1) for line in self.output.read_text().splitlines())
        self.assertEqual(json.loads(outputs["matrix"]), plan["matrix"])
        self.assertEqual(outputs["coverage"], "three-hosts-pr-exception")
        self.assertIn("::notice::", result.stderr)
        self.assertIn("NOT release qualification", self.summary.read_text())

    def test_real_tag_emits_four_hosts_without_exception_notice(self):
        result = self.run_cli("tag", "refs/tags/v0.1.1", "push")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(json.loads(result.stdout)["matrix"]["include"]), 4)
        self.assertIn("coverage=four-hosts\n", self.output.read_text())
        self.assertEqual(result.stderr, "")

    def test_expiry_emits_no_matrix_or_coverage(self):
        self.policy["pr_exception"] = dict(EXCEPTION, starts_on="2000-01-01", expires_on="2000-01-31")
        self.path.write_text(json.dumps(self.policy))
        self.rejected("AP7331")
        self.assertFalse(self.summary.exists())

    def test_duplicate_json_field_is_not_a_hidden_override(self):
        self.path.write_text('{"schema":1,"schema":2}')
        self.rejected("AP7330 duplicate")

    def test_symlink_and_oversized_policy_are_rejected(self):
        real = self.work / "real.json"
        self.path.rename(real)
        self.path.symlink_to(real)
        self.rejected("AP7330 policy must be a regular file")
        self.path.unlink()
        self.path.write_text(" " * 16385)
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
