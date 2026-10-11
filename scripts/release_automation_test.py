"""Exercise the real release admission and one-way transition without GitHub writes."""

from contextlib import redirect_stdout
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("release_automation", ROOT / "scripts/release/release-automation.py")
AUTOMATION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUTOMATION)
REAL_API = AUTOMATION.api
REPO = "metaneutrons/aros-tools"
HEAD = "a" * 40
TAG = "v0.3.20"


class GitHub:
    def __init__(self):
        self.branch = {"protected": True, "commit": {"sha": HEAD}}
        self.pull = {"number": 328, "state": "closed", "merged_at": "2026-10-09T10:00:00Z",
                     "merge_commit_sha": HEAD, "base": {"ref": "main", "repo": {"full_name": REPO}},
                     "head": {"ref": "release-please--branches--main", "repo": {"full_name": REPO}},
                     "title": "chore(main): release 0.3.20",
                     "user": {"login": "metaneutrons-release-please[bot]"},
                     "labels": [{"name": "autorelease: pending"}]}
        self.pulls = [[self.pull]]
        self.pending = [[{"number": 328, "merged_at": "2026-10-09T10:00:00Z",
                          "labels": [{"name": "autorelease: pending"}]}]]
        self.ruleset = {"id": 42, "target": "tag", "enforcement": "active", "bypass_actors": [],
                        "conditions": {"ref_name": {"include": ["refs/tags/v*"], "exclude": []}},
                        "rules": [{"type": "update"}, {"type": "deletion"}]}
        self.files = [[{"filename": p, "status": "modified"} for p in AUTOMATION.FILES]]
        self.runs = {}
        self.jobs = {}
        for number, (workflow, required) in enumerate(AUTOMATION.REQUIRED_JOBS.items(), 1):
            self.runs[workflow] = [{"id": number, "run_attempt": 1, "event": "push",
                                    "head_branch": "main", "head_sha": HEAD,
                                    "head_repository": {"full_name": REPO},
                                    "status": "completed", "conclusion": "success"}]
            self.jobs[number] = [{"name": name, "conclusion": "success"} for name in required]
        self.ref = None
        self.release = None
        self.hidden_draft_index = False
        self.extra_release = None
        self.calls = []
        self.bad_normalization = False
        self.normalization_drift = {}
        self.bad_object = False
        self.bad_ref = False
        self.dispatch_failure = False
        self.hide_public_bypass = False
        self.credential_calls = []

    def __call__(self, path, *, method="GET", payload=None, pages=False, absent=False, governance=False):
        self.calls.append((method, path, payload))
        self.credential_calls.append((method, path, governance))
        if method == "PATCH":
            self.release.update(payload)
            if self.bad_normalization:
                self.release["body"] = "not the canonical notes"
            self.release.update(self.normalization_drift)
            return copy.deepcopy(self.release)
        if method == "POST":
            if path.endswith("/git/tags"):
                return {"sha": "b" * 40, "tag": TAG,
                        "object": {"type": "commit", "sha": "c" * 40 if self.bad_object else HEAD}}
            if path.endswith("/git/refs"):
                self.ref = {"object": {"type": "commit" if self.bad_ref else "tag", "sha": payload["sha"]}}
                return self.ref
            if path.endswith("/dispatches"):
                if self.dispatch_failure:
                    AUTOMATION.fail("synthetic dispatch failure")
                return None
            raise AssertionError(path)
        if path.endswith("/branches/main"):
            return copy.deepcopy(self.branch)
        if "/rulesets?" in path:
            return [[{"id": 42}]]
        if path.endswith("/rulesets/42"):
            result = copy.deepcopy(self.ruleset)
            if self.hide_public_bypass and not governance:
                result.pop("bypass_actors", None)
            return result
        if f"/commits/{HEAD}/pulls?" in path:
            return copy.deepcopy(self.pulls)
        if "/pulls?state=closed" in path:
            return self.pending
        if "/files?" in path:
            return self.files
        if "/actions/workflows/" in path and "/runs?" in path:
            workflow = path.split("/workflows/")[1].split("/")[0]
            return [{"workflow_runs": self.runs[workflow]}]
        if "/jobs?" in path:
            number = int(path.split("/runs/")[1].split("/")[0])
            return [{"jobs": self.jobs[number]}]
        if "/git/ref/tags/" in path:
            return self.ref
        if "/releases?" in path:
            return [[copy.deepcopy(item) for item in (self.release, self.extra_release)
                     if item is not None and not self.hidden_draft_index]]
        if "/releases/" in path:
            return copy.deepcopy(self.release)
        raise AssertionError(path)


class ReleaseAutomation(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="aros-release-admission-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / ".release-please-manifest.json").write_text('{".":"0.3.20"}')
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion="0.3.20"\n')
        (self.root / "Cargo.lock").write_text('[[package]]\nname="aros-cli"\nversion="0.3.20"\n')
        (self.root / "CHANGELOG.md").write_text('## 0.3.20\n\nA reviewed fix.\n\n## 0.3.19\n\nPrevious release.\n')
        scripts = self.root / "scripts/release"
        scripts.mkdir(parents=True)
        shutil.copyfile(ROOT / "scripts/release/render-release-notes.py", scripts / "render-release-notes.py")
        self.gh = GitHub()
        for context in (patch.object(AUTOMATION, "ROOT", self.root),
                        patch.object(AUTOMATION, "api", self.gh),
                        patch.object(AUTOMATION.subprocess, "check_output", return_value=HEAD + "\n"),
                        patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO, "GITHUB_REF": "refs/heads/main",
                                                "GITHUB_EVENT_NAME": "workflow_dispatch",
                                                "GH_TOKEN": "synthetic-secret",
                                                "GOVERNANCE_TOKEN": "synthetic-governance-secret"})):
            context.start()
            self.addCleanup(context.stop)

    def mutations(self):
        return [call for call in self.gh.calls if call[0] != "GET"]

    def prepare(self):
        candidate = AUTOMATION.admission()
        self.assertIsNotNone(candidate)
        admission = self.root / "admission.json"
        admission.write_text(json.dumps(candidate))
        self.gh.release = {"id": 123, "tag_name": TAG, "target_commitish": HEAD,
                           "draft": True, "prerelease": False, "immutable": False, "assets": []}
        os.environ.update(RP_CREATED="true", RP_TAG=TAG, RP_SHA=HEAD, RP_ID="123", RP_VERSION="0.3.20")
        return admission

    def test_exact_green_merge_is_admitted_without_writes(self):
        self.assertEqual(AUTOMATION.admission()["commit"], HEAD)
        self.assertEqual(self.mutations(), [])

    def test_admission_requires_explicit_governance_credential(self):
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": ""}), self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_nonrelease_main_probes_governance_before_a_release_is_merged(self):
        self.gh.pulls = [[]]
        self.assertIsNone(AUTOMATION.admission())
        self.assertTrue(any(privileged for _, _, privileged in self.gh.credential_calls))
        self.gh.ruleset.pop("bypass_actors")
        with self.assertRaisesRegex(SystemExit, "explicit empty bypass list"):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_writer_rechecks_receipt_without_governance_credential(self):
        candidate = AUTOMATION.admission()
        self.gh.hide_public_bypass = True
        self.gh.credential_calls.clear()
        output = self.root / "restored.json"
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "", "ADMISSION_JSON": json.dumps(candidate)}):
            AUTOMATION.recheck(output)
        self.assertEqual(json.loads(output.read_text()), candidate)
        self.assertFalse(any(privileged for _, _, privileged in self.gh.credential_calls))
        self.assertEqual(self.mutations(), [])

    def test_stale_malformed_or_changed_governance_receipt_rejected(self):
        candidate = AUTOMATION.admission()
        receipt = candidate["governance"]
        invalid = [dict(receipt, checked_at=int(time.time()) - 901),
                   dict(receipt, checked_at=int(time.time()) + 120),
                   dict(receipt, checked_at=True), dict(receipt, ruleset_id=True),
                   dict(receipt, policy_sha256="bad"), dict(receipt, extra="unexpected"),
                   dict(receipt, policy_sha256="0" * 64), None]
        for changed in invalid:
            with self.subTest(receipt=changed), patch.dict(os.environ, {
                    "GOVERNANCE_TOKEN": "", "ADMISSION_JSON": json.dumps(dict(candidate, governance=changed))
            }), self.assertRaises(SystemExit):
                AUTOMATION.recheck(self.root / "restored.json")
        for malformed in ("", "not json", "[]", "{}", "null"):
            with patch.dict(os.environ, {"ADMISSION_JSON": malformed}), self.assertRaises(SystemExit):
                AUTOMATION.recheck(self.root / "restored.json")
        self.assertEqual(self.mutations(), [])

    def test_public_policy_change_blocks_writer_before_draft(self):
        candidate = AUTOMATION.admission()
        self.gh.hide_public_bypass = True
        self.gh.ruleset["rules"].append({"type": "non_fast_forward"})
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "", "ADMISSION_JSON": json.dumps(candidate)}), \
                self.assertRaises(SystemExit):
            AUTOMATION.recheck(self.root / "restored.json")
        self.assertEqual(self.mutations(), [])

    def test_admission_governance_credential_is_used_only_for_ruleset_reads(self):
        AUTOMATION.admission()
        self.assertNotIn("GOVERNANCE_TOKEN", AUTOMATION.subprocess.check_output.call_args.kwargs["env"])
        privileged = [(method, path) for method, path, governance in self.gh.credential_calls if governance]
        self.assertEqual(len(privileged), 2)
        self.assertTrue(all(method == "GET" and "/rulesets" in path for method, path in privileged))

    def test_context_requires_own_protected_main(self):
        for key, value in (("GITHUB_REPOSITORY", "foreign/tools"), ("GITHUB_REF", "refs/heads/feature"),
                           ("GH_TOKEN", "")):
            with self.subTest(key=key), patch.dict(os.environ, {key: value}), self.assertRaises(SystemExit):
                AUTOMATION.admission()
        self.gh.branch["protected"] = False
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_stale_checkout_is_not_admitted(self):
        self.gh.branch["commit"]["sha"] = "c" * 40
        self.assertIsNone(AUTOMATION.admission())

    def test_workflow_run_only_trusts_exact_own_main_push(self):
        event = {"workflow_run": {"name": "Workspace CI", "event": "push", "head_branch": "main",
                                  "head_sha": HEAD, "conclusion": "success",
                                  "head_repository": {"full_name": REPO}}}
        path = self.root / "event.json"
        with patch.dict(os.environ, {"GITHUB_EVENT_NAME": "workflow_run", "GITHUB_EVENT_PATH": str(path)}):
            path.write_text(json.dumps(event))
            self.assertIsNotNone(AUTOMATION.admission())
            for key, value in (("event", "pull_request"), ("head_branch", "foreign"),
                               ("head_sha", "c" * 40), ("conclusion", "failure"),
                               ("name", "untrusted workflow"), ("head_repository", {"full_name": "fork/tools"})):
                mutated = copy.deepcopy(event)
                mutated["workflow_run"][key] = value
                path.write_text(json.dumps(mutated))
                self.assertIsNone(AUTOMATION.admission(), key)

    def test_nonrelease_or_wrong_author_never_promotes(self):
        for field, value in (("title", "chore: ordinary change"), ("user", {"login": "foreign"}),
                             ("merged_at", None), ("merge_commit_sha", "c" * 40),
                             ("head", {"ref": "release-please--main", "repo": {"full_name": "fork/tools"}})):
            original = copy.deepcopy(self.gh.pull)
            self.gh.pull[field] = value
            self.assertIsNone(AUTOMATION.admission(), field)
            self.gh.pull.clear()
            self.gh.pull.update(original)

    def test_ambiguous_release_or_unexpected_files_rejected(self):
        self.gh.pulls[0].append(copy.deepcopy(self.gh.pull))
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.gh.pulls[0].pop()
        self.gh.files[0].append({"filename": "src/unreviewed.rs", "status": "modified"})
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()

    def test_other_pending_version_is_rejected_before_release_please(self):
        self.gh.pending[0].append({"number": 100, "merged_at": "2026-10-08T10:00:00Z",
                                  "labels": [{"name": "autorelease: pending"}]})
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_retired_candidate_existing_tag_or_draft_never_restarts(self):
        self.gh.pull["labels"] = []
        self.assertIsNone(AUTOMATION.admission())
        self.gh.pull["labels"] = [{"name": "autorelease: pending"}]
        self.gh.ref = {"object": {"type": "tag", "sha": "b" * 40}}
        self.assertIsNone(AUTOMATION.admission())
        self.gh.ref = None
        self.gh.release = {"id": 100, "tag_name": TAG, "draft": True}
        self.assertIsNone(AUTOMATION.admission())
        self.assertEqual(self.mutations(), [])

    def test_waits_for_both_full_main_workflows(self):
        for conclusion in ("failure", None, "cancelled"):
            self.gh.runs["codeql.yml"][0]["conclusion"] = conclusion
            self.assertIsNone(AUTOMATION.admission())
        self.gh.runs["codeql.yml"][0]["conclusion"] = "success"
        self.gh.jobs[1] = [job for job in self.gh.jobs[1] if job["name"] != "Tests (macos-aarch64)"]
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()

    def test_latest_failed_run_cannot_use_older_green_evidence(self):
        newer = dict(self.gh.runs["ci.yml"][0], id=3, conclusion="failure")
        self.gh.runs["ci.yml"].append(newer)
        self.assertIsNone(AUTOMATION.admission())

    def test_version_drift_rejected(self):
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion="0.3.19"\n')
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()

    def test_tag_policy_must_be_active_closed_and_without_bypass_before_any_write(self):
        original = copy.deepcopy(self.gh.ruleset)
        for key, value in (("enforcement", "evaluate"), ("target", "branch"),
                           ("bypass_actors", [{"actor_id": 1}]), ("bypass_actors", None),
                           ("rules", [{"type": "update"}]),
                           ("conditions", {"ref_name": {"include": ["refs/tags/v*"], "exclude": [TAG]}}),
                           ("conditions", {"ref_name": {"include": ["refs/tags/v0.3.20"], "exclude": []}})):
            self.gh.ruleset = dict(original, **{key: value})
            with self.subTest(key=key, value=value), self.assertRaises(SystemExit):
                AUTOMATION.admission()
        self.gh.ruleset = dict(original)
        del self.gh.ruleset["bypass_actors"]
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_policy_drift_after_release_please_is_rejected_before_tag(self):
        path = self.prepare()
        self.gh.ruleset["enforcement"] = "disabled"
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_exact_candidate_creates_annotated_ref_and_one_tag_dispatch(self):
        admission = self.prepare()
        self.gh.hide_public_bypass = True
        self.gh.credential_calls.clear()
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": ""}), redirect_stdout(io.StringIO()) as output:
            AUTOMATION.start(admission)
        writes = self.mutations()
        self.assertEqual([p.split(f"repos/{REPO}/")[1] for _, p, _ in writes],
                         ["releases/123", "git/tags", "git/refs", "actions/workflows/release.yml/dispatches"])
        self.assertEqual(writes[1][2]["object"], HEAD)
        self.assertEqual(writes[1][2]["type"], "commit")
        self.assertEqual(writes[2][2], {"ref": f"refs/tags/{TAG}", "sha": "b" * 40})
        self.assertEqual(writes[3][2], {"ref": TAG})
        self.assertEqual(self.gh.release["body"], "## 0.3.20\n\nA reviewed fix.\n")
        self.assertNotIn("synthetic-secret", output.getvalue())
        self.assertNotIn("synthetic-governance-secret", output.getvalue())
        self.assertFalse(any(privileged for _, _, privileged in self.gh.credential_calls))
        self.assertIsNone(AUTOMATION.admission())

    def test_release_please_output_mismatch_never_writes(self):
        path = self.prepare()
        for key, value in (("RP_CREATED", "false"), ("RP_TAG", "v0.3.19"), ("RP_SHA", "c" * 40),
                           ("RP_VERSION", "0.3.19"), ("RP_ID", "malformed")):
            with patch.dict(os.environ, {key: value}), self.assertRaises(SystemExit):
                AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_fresh_admission_is_required_after_release_please(self):
        path = self.prepare()
        self.gh.runs["ci.yml"][0]["run_attempt"] = 2
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_new_draft_index_lag_uses_only_exact_numeric_id(self):
        path = self.prepare()
        self.gh.hidden_draft_index = True
        with redirect_stdout(io.StringIO()):
            AUTOMATION.start(path)
        self.assertEqual(sum(p.endswith("/dispatches") for _, p, _ in self.mutations()), 1)

    def test_ambiguous_draft_is_rejected_before_mutation(self):
        path = self.prepare()
        self.gh.extra_release = dict(self.gh.release, id=124)
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_foreign_public_partial_or_changed_draft_never_writes(self):
        path = self.prepare()
        original = copy.deepcopy(self.gh.release)
        for key, value in (("id", 124), ("tag_name", "v0.3.19"), ("target_commitish", "main"),
                           ("draft", False), ("prerelease", True), ("immutable", True),
                           ("assets", [{"id": 1}])):
            self.gh.release = dict(original, **{key: value})
            with self.subTest(key=key), self.assertRaises(SystemExit):
                AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_bad_notes_or_annotation_cannot_create_a_ref(self):
        path = self.prepare()
        self.gh.bad_normalization = True
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertFalse(any(p.endswith("/git/refs") for _, p, _ in self.mutations()))
        self.gh.bad_normalization = False
        self.gh.bad_object = True
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertFalse(any(p.endswith("/git/refs") for _, p, _ in self.mutations()))

    def test_dispatch_failure_consumes_tag_without_automatic_retry(self):
        path = self.prepare()
        self.gh.dispatch_failure = True
        with redirect_stdout(io.StringIO()), self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertIsNotNone(self.gh.ref)
        self.assertIsNone(AUTOMATION.admission())
        self.assertEqual(sum(p.endswith("/dispatches") for _, p, _ in self.mutations()), 1)

    def test_draft_channel_drift_during_normalization_cannot_create_a_tag(self):
        path = self.prepare()
        self.gh.normalization_drift = {"prerelease": True}
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertFalse(any(p.endswith("/git/tags") for _, p, _ in self.mutations()))

    def test_ref_response_must_remain_annotated_before_dispatch(self):
        path = self.prepare()
        self.gh.bad_ref = True
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertFalse(any(p.endswith("/dispatches") for _, p, _ in self.mutations()))

    def test_http_404_is_absent_but_403_and_transport_errors_fail_closed(self):
        # Exercise the real transport wrapper rather than the fixture API.
        with patch.object(AUTOMATION, "api", REAL_API):
            for code, stderr, absent in ((1, "gh: Not Found (HTTP 404)", True),
                                         (1, "gh: Forbidden (HTTP 403)", False),
                                         (1, "transport unavailable", False)):
                response = subprocess.CompletedProcess([], code, "", stderr)
                with patch.object(AUTOMATION.subprocess, "run", return_value=response):
                    if absent:
                        self.assertIsNone(AUTOMATION.api("repos/example/releases/tags/v1", absent=True))
                    else:
                        with self.assertRaises(SystemExit):
                            AUTOMATION.api("repos/example/releases/tags/v1", absent=True)

    def test_real_transport_isolates_governance_token_from_other_calls(self):
        response = subprocess.CompletedProcess([], 0, "{}", "")
        with patch.object(AUTOMATION.subprocess, "run", return_value=response) as run:
            REAL_API(f"repos/{REPO}/rulesets/42", governance=True)
            environment = run.call_args.kwargs["env"]
            self.assertEqual(environment["GH_TOKEN"], "synthetic-governance-secret")
            self.assertNotIn("GOVERNANCE_TOKEN", environment)
            for method, path in (("GET", f"repos/{REPO}/branches/main"),
                                 ("POST", f"repos/{REPO}/git/tags")):
                REAL_API(path, method=method)
                environment = run.call_args.kwargs["env"]
                self.assertEqual(environment["GH_TOKEN"], "synthetic-secret")
                self.assertNotIn("GOVERNANCE_TOKEN", environment)
            for method, path in (("POST", f"repos/{REPO}/rulesets"),
                                 ("GET", f"repos/{REPO}/branches/main"),
                                 ("GET", "repos/foreign/project/rulesets/42"),
                                 ("GET", f"repos/{REPO}/rulesets/42/foreign-endpoint")):
                run.reset_mock()
                with self.assertRaises(SystemExit):
                    REAL_API(path, method=method, governance=True)
                run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
