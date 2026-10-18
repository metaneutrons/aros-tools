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
REAL_GRAPHQL = AUTOMATION.ruleset_bypass_connection
REPO = "metaneutrons/aros-tools"
HEAD = "a" * 40
TAG = "v0.3.20"
RULESET_NODE_ID = "RRS_lACqUmVwb3NpdG9yeXJ1bGVzZXQ="


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
        self.ruleset = {"id": 42, "node_id": RULESET_NODE_ID, "target": "tag", "enforcement": "active",
                        "conditions": {"ref_name": {"include": ["refs/tags/v*"], "exclude": []}},
                        "rules": [{"type": "update"}, {"type": "deletion"}]}
        self.graphql_node = {
            "id": RULESET_NODE_ID, "databaseId": 42, "__typename": "RepositoryRuleset",
            "bypassActors": {"totalCount": 0, "nodes": [], "pageInfo": {"hasNextPage": False}},
        }
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
        # Ordinary REST reads can hide bypass_actors even when configured.
        self.hide_public_bypass = True
        self.graphql_calls = []

    def __call__(self, path, *, method="GET", payload=None, pages=False, absent=False):
        self.calls.append((method, path, payload))
        if method == "PATCH":
            self.release.update(payload)
            if "tag_name" not in payload:
                self.release["tag_name"] = "untagged-synthetic"
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
            return [[{"id": 42, "node_id": RULESET_NODE_ID}]]
        if path.endswith("/rulesets/42"):
            result = copy.deepcopy(self.ruleset)
            if self.hide_public_bypass:
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

    def ruleset_bypass_connection(self, node_id):
        self.graphql_calls.append(node_id)
        return copy.deepcopy(self.graphql_node)


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
                        patch.object(AUTOMATION, "ruleset_bypass_connection", self.gh.ruleset_bypass_connection),
                        patch.object(AUTOMATION.subprocess, "check_output", return_value=HEAD + "\n"),
                        patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO, "GITHUB_REF": "refs/heads/main",
                                                "GITHUB_EVENT_NAME": "workflow_dispatch",
                                                "GH_TOKEN": "synthetic-secret"})):
            context.start()
            self.addCleanup(context.stop)

    def mutations(self):
        return [call for call in self.gh.calls if call[0] != "GET"]

    def set_candidate_labels(self, *labels):
        label_objects = [{"name": label} for label in labels]
        self.gh.pull["labels"] = copy.deepcopy(label_objects)
        self.gh.pending = [[{"number": self.gh.pull["number"],
                             "merged_at": self.gh.pull["merged_at"],
                             "labels": copy.deepcopy(label_objects)}]]

    def prepare(self):
        candidate = AUTOMATION.admission()
        self.assertIsNotNone(candidate)
        admission = self.root / "admission.json"
        admission.write_text(json.dumps(candidate))
        self.set_candidate_labels("autorelease: tagged")
        self.gh.release = {"id": 123, "tag_name": TAG, "target_commitish": HEAD,
                           "draft": True, "prerelease": False, "immutable": False, "assets": []}
        os.environ.update(RP_CREATED="true", RP_TAG=TAG, RP_SHA=HEAD, RP_ID="123", RP_VERSION="0.3.20")
        return admission

    def assert_normalization_response_drift_rejected(self, field, value):
        path = self.prepare()
        self.gh.normalization_drift = {field: value}
        with self.assertRaisesRegex(SystemExit, "private draft normalization did not preserve identity"):
            AUTOMATION.start(path)
        writes = self.mutations()
        self.assertEqual(len(writes), 1)
        self.assertTrue(writes[0][1].endswith("/releases/123"))
        for suffix in ("/git/tags", "/git/refs", "/dispatches"):
            self.assertFalse(any(endpoint.endswith(suffix) for _, endpoint, _ in writes))

    def test_exact_green_merge_is_admitted_without_writes(self):
        self.assertEqual(AUTOMATION.admission()["commit"], HEAD)
        self.assertEqual(self.mutations(), [])

    def test_versioned_transport_preserves_the_merged_release_commit_contract(self):
        self.assertEqual(AUTOMATION.REST_API_VERSION, "2022-11-28")
        fixture_api = self.gh
        path = f"repos/{REPO}/commits/{HEAD}/pulls?per_page=100"

        def versioned_transport(arguments, **kwargs):
            self.assertEqual(kwargs["env"]["GH_TOKEN"], "synthetic-secret")
            self.assertNotIn("GOVERNANCE_TOKEN", kwargs["env"])
            self.assertEqual(arguments[arguments.index("--method") + 1], "GET")
            selected = arguments[arguments.index("-H") + 1]
            response = copy.deepcopy(self.gh.pulls)
            if selected == "X-GitHub-Api-Version: 2026-03-10":
                response[0][0].pop("merge_commit_sha")
            else:
                self.assertEqual(selected, "X-GitHub-Api-Version: 2022-11-28")
            return subprocess.CompletedProcess(arguments, 0, json.dumps(response), "")

        def routed_api(endpoint, **kwargs):
            if endpoint == path:
                return REAL_API(endpoint, **kwargs)
            return fixture_api(endpoint, **kwargs)

        with patch.object(AUTOMATION, "api", side_effect=routed_api), \
                patch.object(AUTOMATION.subprocess, "run", side_effect=versioned_transport):
            self.assertEqual(AUTOMATION.admission()["commit"], HEAD)
            with patch.object(AUTOMATION, "REST_API_VERSION", "2026-03-10"), \
                    self.assertRaisesRegex(SystemExit, "pinned REST API contract"):
                AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_missing_or_malformed_merge_sha_is_not_silently_a_nonrelease(self):
        for value in (None, True, "", "not-an-oid"):
            with self.subTest(value=value):
                self.gh.pulls = [[dict(self.gh.pull, merge_commit_sha=value)]]
                with self.assertRaisesRegex(SystemExit, "pinned REST API contract"):
                    AUTOMATION.admission()
        candidate = copy.deepcopy(self.gh.pull)
        candidate.pop("merge_commit_sha")
        self.gh.pulls = [[candidate]]
        with self.assertRaisesRegex(SystemExit, "pinned REST API contract"):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_admission_uses_the_job_token_without_a_governance_fallback(self):
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "synthetic-governance-secret"}):
            self.assertIsNotNone(AUTOMATION.admission())
        with patch.dict(os.environ, {"GH_TOKEN": "", "GOVERNANCE_TOKEN": "synthetic-governance-secret"}), \
                self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_nonrelease_main_probes_live_graphql_governance_before_a_release_is_merged(self):
        self.gh.pulls = [[]]
        self.assertIsNone(AUTOMATION.admission())
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])
        self.gh.graphql_node["bypassActors"] = {
            "totalCount": 1, "nodes": [{"__typename": "Team"}], "pageInfo": {"hasNextPage": False}}
        with self.assertRaisesRegex(SystemExit, "configured or incompletely reported"):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_hidden_rest_bypass_field_is_accepted_only_when_graphql_proves_zero(self):
        candidate = AUTOMATION.admission()
        self.assertIsNotNone(candidate)
        self.assertNotIn("bypass_actors", self.gh.ruleset)
        self.assertEqual(candidate["governance"]["ruleset_id"], 42)
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])

    def test_bypass_hidden_by_rest_is_rejected_when_graphql_reports_known_actor(self):
        self.gh.graphql_node["bypassActors"] = {
            "totalCount": 1, "nodes": [{"__typename": "Team"}], "pageInfo": {"hasNextPage": False}}
        with self.assertRaisesRegex(SystemExit, "configured or incompletely reported"):
            AUTOMATION.admission()
        self.assertNotIn("bypass_actors", self.gh.ruleset)
        self.assertEqual(self.mutations(), [])

    def test_explicit_rest_bypass_data_cannot_contradict_graphql_zero(self):
        self.gh.hide_public_bypass = False
        self.gh.ruleset["bypass_actors"] = [{"actor_id": 7}]
        with self.assertRaisesRegex(SystemExit, "REST tag ruleset reports"):
            AUTOMATION.admission()
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])
        self.assertEqual(self.mutations(), [])

    def test_graphql_bypass_connection_must_be_complete_and_well_typed(self):
        base = copy.deepcopy(self.gh.graphql_node)
        malformed = [
            None,
            dict(base, id="RRS_another"),
            dict(base, __typename="Repository"),
            dict(base, databaseId="42"),
            dict(base, databaseId=True),
            dict(base, databaseId=43),
            dict(base, bypassActors=None),
            dict(base, bypassActors={"totalCount": False, "nodes": [],
                                     "pageInfo": {"hasNextPage": False}}),
            dict(base, bypassActors={"totalCount": 1, "nodes": [],
                                     "pageInfo": {"hasNextPage": False}}),
            dict(base, bypassActors={"totalCount": 0, "nodes": [{"__typename": "Team"}],
                                     "pageInfo": {"hasNextPage": False}}),
            dict(base, bypassActors={"totalCount": 0, "nodes": [], "pageInfo": None}),
            dict(base, bypassActors={"totalCount": 0, "nodes": [],
                                     "pageInfo": {"hasNextPage": True}}),
            dict(base, bypassActors={"totalCount": 0, "nodes": []}),
        ]
        for node in malformed:
            with self.subTest(node=node), \
                    patch.object(AUTOMATION, "ruleset_bypass_connection", return_value=node), \
                    self.assertRaises(SystemExit):
                AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_writer_rechecks_receipt_without_governance_credential(self):
        candidate = AUTOMATION.admission()
        self.gh.graphql_calls.clear()
        output = self.root / "restored.json"
        with patch.dict(os.environ, {"ADMISSION_JSON": json.dumps(candidate)}):
            AUTOMATION.recheck(output)
        self.assertEqual(json.loads(output.read_text()), candidate)
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])
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
                    "ADMISSION_JSON": json.dumps(dict(candidate, governance=changed))
            }), self.assertRaises(SystemExit):
                AUTOMATION.recheck(self.root / "restored.json")
        for malformed in ("", "not json", "[]", "{}", "null"):
            with patch.dict(os.environ, {"ADMISSION_JSON": malformed}), self.assertRaises(SystemExit):
                AUTOMATION.recheck(self.root / "restored.json")
        self.assertEqual(self.mutations(), [])

    def test_public_policy_change_blocks_writer_before_draft(self):
        candidate = AUTOMATION.admission()
        self.gh.ruleset["rules"].append({"type": "non_fast_forward"})
        with patch.dict(os.environ, {"ADMISSION_JSON": json.dumps(candidate)}), \
                self.assertRaises(SystemExit):
            AUTOMATION.recheck(self.root / "restored.json")
        self.assertEqual(self.mutations(), [])

    def test_writer_rechecks_live_bypass_before_tag_mutation(self):
        path = self.prepare()
        self.gh.graphql_calls.clear()
        self.gh.graphql_node["bypassActors"] = {
            "totalCount": 1, "nodes": [{"__typename": "Team"}], "pageInfo": {"hasNextPage": False}}
        with self.assertRaisesRegex(SystemExit, "configured or incompletely reported"):
            AUTOMATION.start(path)
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])
        self.assertEqual(self.mutations(), [])

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

    def test_tagged_candidate_is_not_admitted_before_draft_creation(self):
        self.set_candidate_labels("autorelease: tagged")
        self.assertIsNone(AUTOMATION.admission())
        self.assertEqual(self.mutations(), [])

    def test_retired_candidate_existing_tag_or_draft_never_restarts(self):
        self.set_candidate_labels()
        self.assertIsNone(AUTOMATION.admission())
        self.set_candidate_labels("autorelease: pending")
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

    def test_tag_policy_must_be_active_and_closed_before_any_write(self):
        original = copy.deepcopy(self.gh.ruleset)
        for key, value in (("enforcement", "evaluate"), ("target", "branch"),
                           ("rules", [{"type": "update"}]),
                           ("conditions", {"ref_name": {"include": ["refs/tags/v*"], "exclude": [TAG]}}),
                           ("conditions", {"ref_name": {"include": ["refs/tags/v0.3.20"], "exclude": []}})):
            self.gh.ruleset = dict(original, **{key: value})
            with self.subTest(key=key, value=value), self.assertRaises(SystemExit):
                AUTOMATION.admission()
        self.gh.ruleset = dict(original)
        self.gh.graphql_node["bypassActors"] = {
            "totalCount": 1, "nodes": [{"__typename": "Team"}], "pageInfo": {"hasNextPage": False}}
        with self.assertRaises(SystemExit):
            AUTOMATION.admission()
        self.assertEqual(self.mutations(), [])

    def test_policy_drift_after_release_please_is_rejected_before_tag(self):
        path = self.prepare()
        self.gh.ruleset["enforcement"] = "disabled"
        with self.assertRaises(SystemExit):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_exact_tagged_transition_is_accepted_once(self):
        admission = self.prepare()
        self.assertEqual(self.gh.pull["labels"], [{"name": "autorelease: tagged"}])
        self.assertEqual(self.gh.pending[0][0]["labels"], [{"name": "autorelease: tagged"}])
        self.assertEqual([item["number"] for page in self.gh.pending for item in page
                          if "autorelease: pending" in {label["name"] for label in item["labels"]}], [])
        self.gh.graphql_calls.clear()
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "synthetic-governance-secret"}), \
                redirect_stdout(io.StringIO()) as output:
            AUTOMATION.start(admission)
        writes = self.mutations()
        self.assertEqual([p.split(f"repos/{REPO}/")[1] for _, p, _ in writes],
                         ["releases/123", "git/tags", "git/refs", "actions/workflows/release.yml/dispatches"])
        self.assertEqual({key: writes[0][2][key] for key in ("tag_name", "target_commitish")},
                         {"tag_name": TAG, "target_commitish": HEAD})
        self.assertEqual(writes[1][2]["object"], HEAD)
        self.assertEqual(writes[1][2]["type"], "commit")
        self.assertEqual(writes[2][2], {"ref": f"refs/tags/{TAG}", "sha": "b" * 40})
        self.assertEqual(writes[3][2], {"ref": TAG})
        self.assertEqual(self.gh.release["body"], "## 0.3.20\n\nA reviewed fix.\n")
        self.assertNotIn("synthetic-secret", output.getvalue())
        self.assertNotIn("synthetic-governance-secret", output.getvalue())
        self.assertEqual(self.gh.graphql_calls, [RULESET_NODE_ID])
        self.assertIsNone(AUTOMATION.admission())
        with self.assertRaisesRegex(SystemExit, "admission changed after draft creation"):
            AUTOMATION.start(admission)
        self.assertEqual(self.mutations(), writes)

    def test_postcreation_pending_label_is_rejected_before_writes(self):
        path = self.prepare()
        self.set_candidate_labels("autorelease: pending")
        with self.assertRaisesRegex(SystemExit, "admission changed after draft creation"):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_postcreation_missing_label_is_rejected_before_writes(self):
        path = self.prepare()
        self.set_candidate_labels()
        with self.assertRaisesRegex(SystemExit, "admission changed after draft creation"):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_postcreation_conflicting_labels_are_rejected_before_writes(self):
        path = self.prepare()
        self.set_candidate_labels("autorelease: pending", "autorelease: tagged")
        with self.assertRaisesRegex(SystemExit, "admission changed after draft creation"):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

    def test_competing_pending_candidate_blocks_postcreation_transition(self):
        path = self.prepare()
        self.gh.pending[0].append({"number": 100, "merged_at": "2026-10-08T10:00:00Z",
                                  "labels": [{"name": "autorelease: pending"}]})
        with self.assertRaisesRegex(SystemExit, "other pending candidates; no release transition"):
            AUTOMATION.start(path)
        self.assertEqual(self.mutations(), [])

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
        with self.assertRaisesRegex(SystemExit, "private draft normalization did not preserve identity"):
            AUTOMATION.start(path)
        self.assertEqual([endpoint.split(f"repos/{REPO}/")[1]
                          for _, endpoint, _ in self.mutations()], ["releases/123"])
        self.gh.calls.clear()
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
        self.assert_normalization_response_drift_rejected("prerelease", True)

    def test_tag_identity_drift_during_normalization_cannot_create_a_tag(self):
        self.assert_normalization_response_drift_rejected("tag_name", "v0.5.1")

    def test_source_identity_drift_during_normalization_cannot_create_a_tag(self):
        self.assert_normalization_response_drift_rejected("target_commitish", "main")

    def test_assets_drift_during_normalization_cannot_create_a_tag(self):
        self.assert_normalization_response_drift_rejected("assets", [{"id": 9}])

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

    def test_real_graphql_transport_uses_static_query_and_the_job_token(self):
        result = {"data": {"node": copy.deepcopy(self.gh.graphql_node)}}
        response = subprocess.CompletedProcess([], 0, json.dumps(result), "")
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "synthetic-governance-secret"}), \
                patch.object(AUTOMATION.subprocess, "run", return_value=response) as run:
            node = REAL_GRAPHQL(RULESET_NODE_ID)
        self.assertEqual(node, self.gh.graphql_node)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[:3], ["gh", "api", "graphql"])
        self.assertIn("query=" + AUTOMATION.RULESET_BYPASS_QUERY, arguments)
        self.assertIn("nodeID=" + RULESET_NODE_ID, arguments)
        self.assertEqual(run.call_args.kwargs["env"]["GH_TOKEN"], "synthetic-secret")
        self.assertNotIn("GOVERNANCE_TOKEN", run.call_args.kwargs["env"])

    def test_rest_transport_never_inherits_the_retired_governance_token(self):
        response = subprocess.CompletedProcess([], 0, "{}", "")
        with patch.dict(os.environ, {"GOVERNANCE_TOKEN": "synthetic-governance-secret"}), \
                patch.object(AUTOMATION.subprocess, "run", return_value=response) as run:
            for method, endpoint in (("GET", f"repos/{REPO}/rulesets/42"),
                                     ("GET", f"repos/{REPO}/branches/main"),
                                     ("POST", f"repos/{REPO}/git/tags")):
                REAL_API(endpoint, method=method)
                environment = run.call_args.kwargs["env"]
                self.assertEqual(environment["GH_TOKEN"], "synthetic-secret")
                self.assertNotIn("GOVERNANCE_TOKEN", environment)

    def test_real_graphql_transport_rejects_http_200_errors_null_and_malformed_json(self):
        successful_data = {"data": {"node": copy.deepcopy(self.gh.graphql_node)}}
        responses = (
            json.dumps({"data": successful_data["data"], "errors": [{"message": "private detail"}]}),
            json.dumps({"data": {"node": None}}),
            "not json",
        )
        for output in responses:
            response = subprocess.CompletedProcess([], 0, output, "")
            with self.subTest(output=output), \
                    patch.object(AUTOMATION.subprocess, "run", return_value=response), \
                    self.assertRaises(SystemExit) as raised:
                REAL_GRAPHQL(RULESET_NODE_ID)
            self.assertNotIn("private detail", str(raised.exception))
        with patch.object(AUTOMATION.subprocess, "run", return_value=
                          subprocess.CompletedProcess([], 1, "", "forbidden (HTTP 403)")), \
                self.assertRaisesRegex(SystemExit, r"GraphQL failed \(HTTP 403\)"):
            REAL_GRAPHQL(RULESET_NODE_ID)


if __name__ == "__main__":
    unittest.main()
