#!/usr/bin/env python3
"""Offline tests of the exact-run archive dispatch and inert manifest boundary."""

from __future__ import annotations

import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("central_contract", SCRIPTS / "central-apt-contract.py")
assert SPEC is not None and SPEC.loader is not None
contract_module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(contract_module)
SHA = "a" * 40
REPOSITORY = "metaneutrons/apt-archive"
RUN_ID = 12345
WORKFLOW_ID = 456
RUN_URL = f"https://github.com/{REPOSITORY}/actions/runs/{RUN_ID}"

# Inert public input, intentionally independent of the publication implementation.
MANIFEST = '''[domain]
host = "deb.metaneutrons.cc"
base_url = "https://deb.metaneutrons.cc"
origin = "metaneutrons"
keyring_package = "metaneutrons-archive-keyring"
keyring_file = "/usr/share/keyrings/metaneutrons-archive-keyring.pgp"
[signing]
primary_fingerprint = "1B7B79417383648BBFBE282E01AB8296EF0FCD76"
signing_subkey = "A0C21782FC507CCBD666F3ED242072FEC8BE54A4" # gitleaks:allow -- public OpenPGP fingerprint, not a credential
[release]
suite = "rolling"
codename = "rolling"
components = ["main"]
architectures = ["amd64", "arm64"]
acquire_by_hash = true
valid_until_days = 180
[[projects]]
name = "aros-tools"
prefix = "/aros-tools"
source_repo = "metaneutrons/aros-tools"
packages = ["aros-tools"]
keep_versions = 5
'''


def fake_gh() -> None:
    root = Path(os.environ["AROS_REQUEST_FIXTURE"])
    scenario = json.loads((root / "scenario.json").read_text())
    args = sys.argv[2:]
    with (root / "calls.jsonl").open("a") as stream:
        stream.write(json.dumps(args) + "\n")
    if not args or args[0] != "api":
        raise SystemExit("unexpected gh command")
    endpoint = next((part for part in args if part.startswith("repos/")), "")
    base = f"repos/{REPOSITORY}"
    if endpoint == base + "/branches/main":
        response = {"name": "main", "protected": scenario.get("protected", True), "commit": {"sha": SHA}}
    elif endpoint == base + f"/contents/domains/metaneutrons.cc/manifest.toml?ref={SHA}":
        if "Accept: application/vnd.github.raw+json" not in args:
            raise SystemExit("manifest must be read as inert raw data")
        print(MANIFEST.replace('suite = "rolling"', 'suite = "wrong"') if scenario.get("manifest_drift") else MANIFEST)
        return
    elif endpoint == base + "/actions/workflows/publish.yml":
        response = {"id": WORKFLOW_ID, "state": scenario.get("workflow_state", "active"),
                    "path": scenario.get("workflow_path", ".github/workflows/publish.yml")}
    elif endpoint == base + "/commits/main":
        if args[-2:] != ["--jq", ".sha"]:
            raise SystemExit("unexpected source recheck")
        print(scenario.get("recheck_sha", SHA))
        return
    elif endpoint == base + "/actions/workflows/publish.yml/dispatches":
        for expected in ("POST", "ref=main", "return_run_details=true", "inputs[domain]=metaneutrons.cc",
                         "inputs[project]=aros-tools", "X-GitHub-Api-Version: 2022-11-28"):
            if expected not in args:
                raise SystemExit("incorrect dispatch protocol")
        if scenario.get("dispatch_error"):
            print("simulated ambiguous HTTP failure", file=sys.stderr)
            raise SystemExit(1)
        response = scenario.get("dispatch_response", {"workflow_run_id": RUN_ID, "html_url": RUN_URL})
    elif endpoint == base + f"/actions/runs/{RUN_ID}":
        response = {"id": RUN_ID, "workflow_id": WORKFLOW_ID, "head_sha": SHA, "head_branch": "main",
                    "event": "workflow_dispatch", "repository": {"full_name": REPOSITORY},
                    "status": "completed", "conclusion": "success"}
        response.update(scenario.get("run", {}))
        if scenario.get("queued_once") and not (root / "observed").exists():
            (root / "observed").touch()
            response.update(status="queued", conclusion=None)
    else:
        raise SystemExit(f"unexpected endpoint: {endpoint}")
    print(json.dumps(response))


class ArchiveRequestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="aros-archive-request-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        binary = self.root / "bin"
        binary.mkdir()
        (binary / "gh").write_text(
            f"#!{sys.executable}\nimport runpy, sys\nsys.argv.insert(1, '--fake-gh')\n"
            f"runpy.run_path({str(Path(__file__).resolve())!r}, run_name='__main__')\n")
        (binary / "gh").chmod(0o755)
        (binary / "sleep").write_text("#!/bin/sh\nexit 0\n")
        (binary / "sleep").chmod(0o755)
        self.environment = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"],
                                GH_TOKEN="disposable-test-only", AROS_REQUEST_FIXTURE=str(self.root),
                                GITHUB_STEP_SUMMARY=str(self.root / "summary.md"))

    def request(self, mode="dispatch", **scenario):
        (self.root / "scenario.json").write_text(json.dumps(scenario))
        return subprocess.run(["bash", str(SCRIPTS / "request-central-apt.sh"), mode],
                              env=self.environment, capture_output=True, text=True, timeout=15)

    def calls(self):
        path = self.root / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def assert_failed(self, result, *, dispatches):
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("AP7252", result.stderr)
        self.assertEqual(sum("POST" in call for call in self.calls()), dispatches, result.stderr)
        self.assertNotIn("disposable-test-only", result.stdout + result.stderr)

    def test_preflight_is_read_only(self):
        result = self.request("preflight")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any("POST" in call for call in self.calls()))
        self.assertFalse((self.root / "summary.md").exists())

    def test_exact_success_and_receipt(self):
        result = self.request()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(RUN_ID))
        self.assertIn(RUN_URL, (self.root / "summary.md").read_text())
        self.assertEqual(sum("POST" in call for call in self.calls()), 1)

    def test_queue_follows_only_the_dispatched_run(self):
        result = self.request(queued_once=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = [call for call in self.calls() if f"repos/{REPOSITORY}/actions/runs/{RUN_ID}" in call]
        self.assertEqual(len(observed), 2)

    def test_unprotected_branch_is_rejected(self):
        self.assert_failed(self.request(protected=False), dispatches=0)

    def test_manifest_drift_is_rejected_before_dispatch(self):
        self.assert_failed(self.request(manifest_drift=True), dispatches=0)

    def test_source_race_is_rejected_before_dispatch(self):
        self.assert_failed(self.request(recheck_sha="b" * 40), dispatches=0)

    def test_disabled_workflow_is_rejected(self):
        self.assert_failed(self.request(workflow_state="disabled_manually"), dispatches=0)

    def test_wrong_workflow_path_is_rejected(self):
        self.assert_failed(self.request(workflow_path=".github/workflows/other.yml"), dispatches=0)

    def test_ambiguous_submission_is_never_retried(self):
        self.assert_failed(self.request(dispatch_error=True), dispatches=1)

    def test_missing_run_id_is_not_guessed(self):
        self.assert_failed(self.request(dispatch_response={}), dispatches=1)

    def test_wrong_run_url_is_rejected(self):
        self.assert_failed(self.request(dispatch_response={"workflow_run_id": RUN_ID,
                                                          "html_url": "https://example.invalid/run"}), dispatches=1)

    def test_changed_run_identity_is_rejected(self):
        for changed in ({"id": 99}, {"workflow_id": 99}, {"head_sha": "b" * 40},
                        {"head_branch": "other"}, {"event": "push"},
                        {"repository": {"full_name": "metaneutrons/other"}}):
            with self.subTest(changed=changed):
                (self.root / "calls.jsonl").unlink(missing_ok=True)
                self.assert_failed(self.request(run=changed), dispatches=1)

    def test_failed_cancelled_and_timed_out_runs_fail_closed(self):
        for conclusion in ("failure", "cancelled", "timed_out", "skipped", None):
            with self.subTest(conclusion=conclusion):
                (self.root / "calls.jsonl").unlink(missing_ok=True)
                self.assert_failed(self.request(run={"conclusion": conclusion}), dispatches=1)

    def test_missing_token_fails_before_api_access(self):
        self.environment.pop("GH_TOKEN")
        self.assert_failed(self.request(), dispatches=0)
        self.assertEqual(self.calls(), [])


class CentralManifestTests(unittest.TestCase):
    def setUp(self):
        self.manifest = contract_module.tomllib.loads(MANIFEST)
        self.contract = contract_module.apt.load_contract()

    def test_canonical_manifest(self):
        contract_module.validate(self.manifest, self.contract)

    def test_malformed_table_types_are_rejected(self):
        for section in ("domain", "signing", "release", "projects"):
            with self.subTest(section=section):
                changed = copy.deepcopy(self.manifest)
                changed[section] = "not a table"
                with self.assertRaises(contract_module.apt.VerificationError):
                    contract_module.validate(changed, self.contract)

    def test_each_identity_field_is_enforced(self):
        for section in ("domain", "signing", "release"):
            for field in self.manifest[section]:
                with self.subTest(section=section, field=field):
                    changed = copy.deepcopy(self.manifest)
                    changed[section][field] = None
                    with self.assertRaises(contract_module.apt.VerificationError):
                        contract_module.validate(changed, self.contract)

    def test_project_selection_and_retention_are_enforced(self):
        for field in self.manifest["projects"][0]:
            with self.subTest(field=field):
                changed = copy.deepcopy(self.manifest)
                changed["projects"][0][field] = None
                with self.assertRaises(contract_module.apt.VerificationError):
                    contract_module.validate(changed, self.contract)
        self.manifest["projects"] *= 2
        with self.assertRaises(contract_module.apt.VerificationError):
            contract_module.validate(self.manifest, self.contract)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--fake-gh":
        fake_gh()
    else:
        unittest.main()
