"""Tests for the bounded npm audit checker and its temporary exception."""

from __future__ import annotations

import copy
import datetime as dt
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts/check-docs-audit.py"
SPEC = importlib.util.spec_from_file_location("check_docs_audit", CHECKER)
assert SPEC is not None and SPEC.loader is not None
AUDIT = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = AUDIT
SPEC.loader.exec_module(AUDIT)

UTC = dt.timezone.utc
APPROVAL_START = dt.datetime(2026, 10, 3, 0, 0, tzinfo=UTC)
APPROVAL_ACTIVE = dt.datetime(2026, 10, 3, 6, 0, tzinfo=UTC)
EXPIRES = dt.datetime(2026, 10, 10, 22, 0, tzinfo=UTC)
LEAF = "http-cache-semantics"
ANCESTORS = (
    "astro",
    "@astrojs/mdx",
    "@astrojs/starlight",
    "astro-expressive-code",
)


def node_path(package: str) -> str:
    return f"node_modules/{package}"


def empty_report() -> dict:
    return {
        "auditReportVersion": 2,
        "vulnerabilities": {},
        "metadata": {
            "vulnerabilities": {
                "info": 0,
                "low": 0,
                "moderate": 0,
                "high": 0,
                "critical": 0,
                "total": 0,
            },
            "dependencies": {
                "prod": 0,
                "dev": 0,
                "optional": 0,
                "peer": 0,
                "peerOptional": 0,
                "total": 0,
            },
        },
    }


def add_vulnerability(report: dict, name: str, via, *, severity="high") -> None:
    report["vulnerabilities"][name] = {
        "name": name,
        "severity": severity,
        "isDirect": name in {"astro", "@astrojs/starlight"},
        "via": copy.deepcopy(via),
        "effects": [],
        "range": "*",
        "nodes": [node_path(name)],
        "fixAvailable": False,
    }


def exception_report() -> dict:
    report = empty_report()
    advisory = {
        "source": 1234567,
        "name": LEAF,
        "dependency": LEAF,
        "title": "Temporary test advisory",
        "url": AUDIT.ADVISORY_URL,
        "severity": "high",
        "range": AUDIT.LEAF_RANGE,
    }
    add_vulnerability(report, LEAF, [advisory])
    for package in ANCESTORS:
        add_vulnerability(report, package, AUDIT.ALLOWED_VIA[package])
    report["metadata"]["vulnerabilities"]["high"] = len(ANCESTORS) + 1
    report["metadata"]["vulnerabilities"]["total"] = len(ANCESTORS) + 1
    return report


def single_advisory_report(severity: str) -> dict:
    report = empty_report()
    advisory = {
        "source": 987654,
        "name": "ordinary-package",
        "dependency": "ordinary-package",
        "title": "Ordinary test advisory",
        "url": "https://github.com/advisories/GHSA-1111-2222-3333",
        "severity": severity,
        "range": "<2.0.0",
    }
    add_vulnerability(report, "ordinary-package", [advisory], severity=severity)
    report["metadata"]["vulnerabilities"][severity] = 1
    report["metadata"]["vulnerabilities"]["total"] = 1
    return report


def lock_fixture() -> dict:
    packages = {"": copy.deepcopy(AUDIT.LOCKED_ROOT)}
    for name, expected in AUDIT.LOCKED_PACKAGES.items():
        packages[node_path(name)] = dict(expected)
        packages[node_path(name)]["dependencies"] = {
            child: "*" for child in AUDIT.ALLOWED_VIA.get(name, [])
        }
    return {"name": "docs-site", "lockfileVersion": 3, "requires": True, "packages": packages}


def policy_fixture() -> dict:
    return AUDIT._read_json_file(AUDIT.POLICY)


class DocsAuditValidationTests(unittest.TestCase):
    def validate(self, report, *, code=1, lock=None, now=APPROVAL_ACTIVE, configs=None):
        AUDIT.validate_report(
            report,
            exit_code=code,
            lock=lock if lock is not None else lock_fixture(),
            policy=policy_fixture(),
            configuration_sha256=(
                configs if configs is not None else AUDIT.CONFIGURATION_SHA256
            ),
            now=now,
        )

    def test_clean_npm_v2_report_passes(self):
        self.validate(empty_report(), code=0, lock={"packages": {}}, now=EXPIRES + dt.timedelta(days=1))

    def test_ordinary_low_and_moderate_advisories_remain_allowed(self):
        for severity in ("low", "moderate"):
            with self.subTest(severity=severity):
                self.validate(single_advisory_report(severity), code=0)

    def test_exact_leaf_and_all_propagation_pass(self):
        self.validate(exception_report())

    def test_false_duplicate_or_missing_propagation_edges_fail(self):
        for edges in ([LEAF], ["astro", "astro"], ["astro"]):
            with self.subTest(edges=edges):
                report = exception_report()
                report["vulnerabilities"]["@astrojs/starlight"]["via"] = edges
                with self.assertRaises(AUDIT.AuditValidationError):
                    self.validate(report)
        report = exception_report()
        report["vulnerabilities"]["@astrojs/starlight"]["via"].reverse()
        self.validate(report)

    def test_changed_lock_dependency_edges_and_incomplete_graph_fail(self):
        for edges in ({LEAF: "*"}, {}, {"astro": "*", LEAF: "*"}):
            with self.subTest(edges=edges):
                lock = lock_fixture()
                lock["packages"][node_path("@astrojs/mdx")]["dependencies"] = edges
                with self.assertRaises(AUDIT.AuditValidationError):
                    self.validate(exception_report(), lock=lock)
        report = exception_report()
        del report["vulnerabilities"]["@astrojs/starlight"]
        report["metadata"]["vulnerabilities"].update({"high": 4, "total": 4})
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report)

    def test_new_direct_dependency_or_forged_direct_status_fails(self):
        lock = lock_fixture()
        lock["packages"][""]["dependencies"][LEAF] = AUDIT.LEAF_VERSION
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(exception_report(), lock=lock)
        for package in AUDIT.LOCKED_PACKAGES:
            with self.subTest(package=package):
                report = exception_report()
                record = report["vulnerabilities"][package]
                record["isDirect"] = not record["isDirect"]
                with self.assertRaises(AUDIT.AuditValidationError):
                    self.validate(report)

    def test_other_advisory_on_leaf_fails(self):
        report = exception_report()
        report["vulnerabilities"][LEAF]["via"][0]["url"] = (
            "https://github.com/advisories/GHSA-aaaa-bbbb-cccc"
        )
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report)

    def test_new_high_package_fails(self):
        report = exception_report()
        add_vulnerability(report, "new-high-package", [{
            "source": 1,
            "name": "new-high-package",
            "dependency": "new-high-package",
            "title": "Other finding",
            "url": "https://github.com/advisories/GHSA-aaaa-bbbb-cccc",
            "severity": "high",
            "range": "<1.0.0",
        }])
        report["metadata"]["vulnerabilities"]["high"] += 1
        report["metadata"]["vulnerabilities"]["total"] += 1
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report)

    def test_critical_escalation_fails(self):
        report = exception_report()
        for record in report["vulnerabilities"].values():
            record["severity"] = "critical"
            for via in record["via"]:
                if isinstance(via, dict):
                    via["severity"] = "critical"
        report["metadata"]["vulnerabilities"].update(
            {"high": 0, "critical": len(report["vulnerabilities"])}
        )
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report)

    def test_exception_fails_before_approval_at_expiry_and_after(self):
        for current in (
            APPROVAL_START - dt.timedelta(microseconds=1),
            EXPIRES,
            EXPIRES + dt.timedelta(seconds=1),
        ):
            with self.subTest(now=current), self.assertRaises(AUDIT.AuditValidationError):
                self.validate(exception_report(), now=current)

    def test_changed_configuration_binding_fails(self):
        configs = dict(AUDIT.CONFIGURATION_SHA256)
        configs["docs-site/astro.config.mjs"] = "0" * 64
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(exception_report(), configs=configs)

    def test_changed_leaf_or_ancestor_lock_fails(self):
        for package in (LEAF, "@astrojs/mdx"):
            for field, changed in (("integrity", "sha512-changed"), ("version", "0.0.0")):
                with self.subTest(package=package, field=field):
                    lock = lock_fixture()
                    lock["packages"][node_path(package)][field] = changed
                    with self.assertRaises(AUDIT.AuditValidationError):
                        self.validate(exception_report(), lock=lock)

    def test_dangling_and_cyclic_via_chains_fail(self):
        dangling = exception_report()
        dangling["vulnerabilities"]["astro"]["via"] = ["missing-package"]
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(dangling)

        cyclic = exception_report()
        cyclic["vulnerabilities"]["astro"]["via"] = ["@astrojs/mdx"]
        cyclic["vulnerabilities"]["@astrojs/mdx"]["via"] = ["astro"]
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(cyclic)

    def test_malformed_propagation_or_extra_parent_advisory_fails(self):
        report = exception_report()
        report["vulnerabilities"]["astro"]["via"].append(
            copy.deepcopy(report["vulnerabilities"][LEAF]["via"][0])
        )
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report)

    def test_low_record_cannot_hide_high_or_critical_via_causes(self):
        for child_severity in ("high", "critical"):
            with self.subTest(child_severity=child_severity):
                report = empty_report()
                high_child = {
                    "source": 777,
                    "name": "ordinary-package",
                    "dependency": "ordinary-package",
                    "title": "Higher severity cause",
                    "url": "https://github.com/advisories/GHSA-1111-2222-3333",
                    "severity": child_severity,
                    "range": "<2.0.0",
                }
                add_vulnerability(report, "ordinary-package", [high_child], severity="low")
                report["metadata"]["vulnerabilities"].update({"low": 1, "total": 1})
                with self.assertRaises(AUDIT.AuditValidationError):
                    self.validate(report, code=0)

        report = empty_report()
        report["vulnerabilities"][LEAF] = exception_report()["vulnerabilities"][LEAF]
        add_vulnerability(report, "ordinary-package", [LEAF], severity="low")
        report["metadata"]["vulnerabilities"].update({"low": 1, "high": 1, "total": 2})
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(report, code=1)

    def test_metadata_counts_must_match_each_record_severity(self):
        cases = (("low", single_advisory_report("low")), ("high", exception_report()))
        for severity, report in cases:
            with self.subTest(severity=severity):
                report["metadata"]["vulnerabilities"]["total"] -= 1
                report["metadata"]["vulnerabilities"][severity] -= 1
                with self.assertRaises(AUDIT.AuditValidationError):
                    self.validate(report, code=1 if severity == "high" else 0)

    def test_dangling_and_cyclic_low_severity_chains_fail(self):
        dangling = empty_report()
        add_vulnerability(dangling, "ordinary-package", ["missing-package"], severity="low")
        dangling["metadata"]["vulnerabilities"].update({"low": 1, "total": 1})
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(dangling, code=0)

        cyclic = empty_report()
        add_vulnerability(cyclic, "low-package-a", ["low-package-b"], severity="low")
        add_vulnerability(cyclic, "low-package-b", ["low-package-a"], severity="low")
        cyclic["metadata"]["vulnerabilities"].update({"low": 2, "total": 2})
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(cyclic, code=0)

    def test_clean_report_after_expiry_does_not_need_exception_bindings(self):
        changed_lock = {"packages": {"node_modules/http-cache-semantics": {"version": "4.2.1"}}}
        changed_configs = {"docs-site/astro.config.mjs": "changed"}
        self.validate(
            empty_report(),
            code=0,
            lock=changed_lock,
            now=EXPIRES + dt.timedelta(seconds=1),
            configs=changed_configs,
        )

    def test_report_shape_status_and_policy_are_closed(self):
        malformed = empty_report()
        malformed["error"] = {"code": "EAUDIT"}
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(malformed, code=1)
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(empty_report(), code=2)
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(empty_report(), code=1)
        policy = policy_fixture()
        policy["unexpected"] = True
        with self.assertRaises(AUDIT.AuditValidationError):
            AUDIT.validate_policy(policy)

        malformed_severity = single_advisory_report("low")
        malformed_severity["vulnerabilities"]["ordinary-package"]["severity"] = []
        with self.assertRaises(AUDIT.AuditValidationError):
            self.validate(malformed_severity, code=0)


class DocsAuditCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.bin = root / "bin"
        self.bin.mkdir()
        self.report_file = root / "audit-report.json"
        self.stderr_file = root / "audit-stderr.txt"
        self.call_log = root / "npm-call.json"
        self.npm = self.bin / "npm"
        self.npm.write_text(
            "#!" + sys.executable + "\n"
            "import json, os, pathlib, sys\n"
            "pathlib.Path(os.environ['FAKE_AUDIT_CALL_LOG']).write_text(json.dumps({"
            "'cwd': os.getcwd(), 'args': sys.argv[1:]}))\n"
            "sys.stdout.buffer.write(pathlib.Path(os.environ['FAKE_AUDIT_REPORT']).read_bytes())\n"
            "stderr = pathlib.Path(os.environ['FAKE_AUDIT_STDERR']).read_bytes()\n"
            "sys.stderr.buffer.write(stderr)\n"
            "raise SystemExit(int(os.environ['FAKE_AUDIT_STATUS']))\n"
        )
        self.npm.chmod(0o755)

    def test_continuous_oversized_output_is_stopped_immediately(self):
        self.npm.write_text(
            "#!" + sys.executable + "\n"
            "import os, time\n"
            "while True:\n"
            "    os.write(1, b'x' * 65536)\n"
            "    time.sleep(0.001)\n"
        )
        started = time.monotonic()
        result = self.run_checker(b"{}")
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"output exceeded its size limit", result.stderr)
        self.assertLess(time.monotonic() - started, 10)

    def run_checker(self, raw: bytes, *, status=0, stderr=b"", args=()):
        self.report_file.write_bytes(raw)
        self.stderr_file.write_bytes(stderr)
        env = os.environ.copy()
        env.update({
            "PATH": str(self.bin) + os.pathsep + env.get("PATH", ""),
            "FAKE_AUDIT_CALL_LOG": str(self.call_log),
            "FAKE_AUDIT_REPORT": str(self.report_file),
            "FAKE_AUDIT_STDERR": str(self.stderr_file),
            "FAKE_AUDIT_STATUS": str(status),
        })
        return subprocess.run(
            [sys.executable, str(CHECKER), *args],
            cwd=self.temporary.name,
            env=env,
            capture_output=True,
            timeout=10,
        )

    def assert_audit_ran(self):
        self.assertTrue(self.call_log.exists(), "the checker did not invoke npm audit")
        call = json.loads(self.call_log.read_text())
        self.assertEqual(call["cwd"], str(ROOT / "docs-site"))
        self.assertEqual(call["args"], ["audit", "--json", "--audit-level=high"])

    def test_real_cli_accepts_clean_json_report_and_runs_audit(self):
        result = self.run_checker(json.dumps(empty_report()).encode(), status=0)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(b"documentation npm audit clean", result.stdout)
        self.assert_audit_ran()

    def test_real_cli_labels_subthreshold_findings_as_such(self):
        result = self.run_checker(
            json.dumps(single_advisory_report("moderate")).encode(), status=0
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(b"passed below the high severity threshold", result.stdout)
        self.assertNotIn(b"GHSA-ch52-4w7c-c8xp", result.stdout)
        self.assert_audit_ran()

    def test_real_cli_uses_current_time_for_temporary_exception(self):
        result = self.run_checker(json.dumps(exception_report()).encode(), status=1)
        self.assert_audit_ran()
        current = dt.datetime.now(UTC)
        active = APPROVAL_START <= current < EXPIRES
        self.assertEqual(result.returncode == 0, active, result.stdout + result.stderr)
        if active:
            self.assertIn(b"GHSA-ch52-4w7c-c8xp", result.stdout)
            self.assertIn(b"2026-10-10T22:00:00Z", result.stdout)

    def test_real_cli_rejects_npm_error_malformed_json_and_duplicate_fields(self):
        cases = (
            (b'{"error":{"code":"EAUDIT"}}', 1),
            (b'{not json', 0),
            (b'{"auditReportVersion":2,"auditReportVersion":2}', 0),
            (
                json.dumps({
                    **single_advisory_report("low"),
                    "vulnerabilities": {
                        "ordinary-package": {
                            **single_advisory_report("low")["vulnerabilities"]["ordinary-package"],
                            "severity": [],
                        }
                    },
                }).encode(),
                0,
            ),
        )
        for raw, status in cases:
            with self.subTest(raw=raw):
                result = self.run_checker(raw, status=status)
                self.assertNotEqual(result.returncode, 0)
            self.assert_audit_ran()

    def test_real_cli_rejects_unexpected_exit_and_hides_npm_diagnostics(self):
        report = self.run_checker(json.dumps(empty_report()).encode(), status=7)
        self.assertNotEqual(report.returncode, 0)
        self.assert_audit_ran()
        diagnostic = b"PRIVATE_NPM_TRANSPORT_DETAIL"
        result = self.run_checker(
            json.dumps(empty_report()).encode(), status=0, stderr=diagnostic
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(diagnostic, result.stdout + result.stderr)
        self.assert_audit_ran()

    def test_cli_options_cannot_bypass_the_audit(self):
        result = self.run_checker(
            json.dumps(empty_report()).encode(), status=0, args=("--skip",)
        )
        self.assertNotEqual(result.returncode, 0)
        self.assert_audit_ran()

    def test_real_cli_rejects_output_over_limit_without_printing_it(self):
        oversized = b" " * (AUDIT_MAX_OUTPUT + 32)
        result = self.run_checker(oversized, status=0)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn(oversized[:128], result.stdout + result.stderr)
        self.assert_audit_ran()


AUDIT_MAX_OUTPUT = 1_048_576


if __name__ == "__main__":
    unittest.main()
