#!/usr/bin/env python3
"""Positive and fail-closed counter-probes for the protection preflight."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("protection", HERE / "verify-branch-protection.py")
protection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protection)
SHA = "0123456789abcdef0123456789abcdef01234567"
CONTRACT = HERE.parents[1] / "contracts/repository-governance-v1.toml"


class FixtureAPI:
    def __init__(self, mode="ruleset"):
        self.repository = "metaneutrons/aros-tools" if mode == "ruleset" else "metaneutrons/homebrew-tap"
        self.policy = protection.load_policy(CONTRACT, self.repository)
        p = self.policy
        self.branch = {"protected": True, "commit": {"sha": SHA}}
        self.repo = {"default_branch": "main"}
        self.classic = None
        self.definition = {
            "id": 42, "source_type": "Repository", "source": self.repository,
            "target": "branch", "enforcement": "active", "bypass_actors": [],
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
            "rules": [{"type": "deletion"}, {"type": "non_fast_forward"},
                      {"type": "required_linear_history"},
                      {"type": "pull_request", "parameters": {
                          "required_approving_review_count": p["required_approving_review_count"],
                          "dismiss_stale_reviews_on_push": p["dismiss_stale_reviews"],
                          "require_code_owner_review": p["require_code_owner_reviews"],
                          "require_last_push_approval": p["require_last_push_approval"],
                          "required_review_thread_resolution": p["required_conversation_resolution"],
                          "required_reviewers": [], "allowed_merge_methods": ["squash"],
                          "require_extra_approval_for_unattributed_changes": True}},
                      {"type": "required_status_checks", "parameters": {
                          "strict_required_status_checks_policy": True, "do_not_enforce_on_create": False,
                          "required_status_checks": [{"context": c["context"], "integration_id": c["app_id"]}
                                                     for c in p["required_status_checks"]]}}],
        }
        self.refresh_rules()
        if mode == "classic":
            self.classic = {
                "required_status_checks": {"strict": True, "checks": p["required_status_checks"]},
                "required_pull_request_reviews": {key: p[key] for key in (
                    "required_approving_review_count", "dismiss_stale_reviews",
                    "require_code_owner_reviews", "require_last_push_approval")},
                **{key: {"enabled": p[key]} for key in (
                    "required_conversation_resolution", "enforce_admins", "required_linear_history",
                    "allow_force_pushes", "allow_deletions")},
            }
            self.rules = []
        self.calls = []

    def refresh_rules(self):
        self.rules = [{**copy.deepcopy(row), "ruleset_id": 42,
                       "ruleset_source_type": "Repository", "ruleset_source": self.repository}
                      for row in self.definition["rules"]]

    def get(self, endpoint, *, allow_absent=False):
        self.calls.append(endpoint)
        base = "repos/" + self.repository
        if endpoint == base + "/branches/main/protection":
            if not allow_absent:
                raise AssertionError("classic absence must be explicitly requested")
            return copy.deepcopy(self.classic)
        return copy.deepcopy({base: self.repo, base + "/branches/main": self.branch,
                              base + "/rules/branches/main": self.rules,
                              base + "/rulesets/42": self.definition}[endpoint])


class Governance(unittest.TestCase):
    def run_fixture(self, fixture):
        return protection.verify(fixture, fixture.repository, fixture.policy)

    def test_valid_ruleset_and_classic(self):
        for mode in ("ruleset", "classic"):
            with self.subTest(mode=mode):
                self.assertEqual(self.run_fixture(FixtureAPI(mode)), SHA)

    def test_explicit_branch_scope(self):
        api = FixtureAPI()
        api.definition["conditions"]["ref_name"]["include"] = ["refs/heads/main"]
        self.assertEqual(self.run_fixture(api), SHA)

    def test_definition_boundaries(self):
        cases = (("enforcement", "disabled"), ("enforcement", "evaluate"), ("target", "tag"),
                 ("source", "other/repository"), ("source_type", "Organization"), ("id", 43),
                 ("bypass_actors", [{"actor_type": "RepositoryRole", "actor_id": 5}]),
                 ("conditions", {"ref_name": {"include": ["refs/heads/other"], "exclude": []}}),
                 ("conditions", {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": ["refs/heads/main"]}}),
                 ("conditions", {"ref_name": {"include": ["refs/heads/*"], "exclude": []}}))
        for key, value in cases:
            with self.subTest(key=key, value=value):
                api = FixtureAPI(); api.definition[key] = value
                with self.assertRaises(protection.PolicyError):
                    self.run_fixture(api)

    def test_missing_bypass_is_not_empty(self):
        api = FixtureAPI(); del api.definition["bypass_actors"]
        with self.assertRaises(KeyError):
            self.run_fixture(api)

    def test_effective_scope_and_overlay(self):
        for field, value in (("ruleset_id", 43), ("ruleset_id", True),
                             ("ruleset_source", "other/repo"), ("ruleset_source_type", "Organization")):
            with self.subTest(field=field, value=value):
                api = FixtureAPI(); api.rules[0][field] = value
                with self.assertRaises(protection.PolicyError):
                    self.run_fixture(api)
        api = FixtureAPI(); api.rules.append({"type": "update", "ruleset_id": 42})
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)
        api = FixtureAPI(); api.rules[0]["type"] = "required_signatures"
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)
        api = FixtureAPI(); api.rules[0]["type"] = "non_fast_forward"
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)
        api = FixtureAPI("classic"); api.refresh_rules()
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)

    def test_ruleset_check_policy(self):
        for mode in ("missing-build", "missing-app", "wrong-app", "boolean-app", "duplicate", "extra"):
            with self.subTest(mode=mode):
                api = FixtureAPI()
                checks = api.definition["rules"][-1]["parameters"]["required_status_checks"]
                if mode == "missing-build":
                    checks[:] = [c for c in checks if c["context"] != "build"]
                elif mode == "missing-app":
                    del checks[0]["integration_id"]
                elif mode == "wrong-app":
                    checks[0]["integration_id"] = 1
                elif mode == "boolean-app":
                    checks[0]["integration_id"] = True
                elif mode == "duplicate":
                    checks.append(copy.deepcopy(checks[0]))
                else:
                    checks.append({"context": "unreviewed", "integration_id": 15368})
                api.refresh_rules()
                with self.assertRaises(protection.PolicyError):
                    self.run_fixture(api)
        for key in ("strict_required_status_checks_policy", "do_not_enforce_on_create"):
            api = FixtureAPI()
            value = api.definition["rules"][-1]["parameters"]
            value[key] = not value[key]; api.refresh_rules()
            with self.assertRaises(protection.PolicyError):
                self.run_fixture(api)

    def test_reviews_and_stale_api(self):
        for key in ("required_approving_review_count", "dismiss_stale_reviews_on_push",
                    "require_code_owner_review", "require_last_push_approval", "required_review_thread_resolution"):
            with self.subTest(key=key):
                api = FixtureAPI(); reviews = api.definition["rules"][3]["parameters"]
                reviews[key] = 1 if key == "required_approving_review_count" else not reviews[key]
                api.refresh_rules()
                with self.assertRaises(protection.PolicyError):
                    self.run_fixture(api)
        api = FixtureAPI(); api.repo["default_branch"] = "other"
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)
        api = FixtureAPI(); api.definition["rules"][-1]["parameters"]["do_not_enforce_on_create"] = True
        with self.assertRaisesRegex(protection.PolicyError, "effective"):
            self.run_fixture(api)
        api = FixtureAPI()
        original = api.get
        def drift(endpoint, **kwargs):
            result = original(endpoint, **kwargs)
            if endpoint.endswith("rules/branches/main") and api.calls.count(endpoint) == 2:
                return []
            return result
        api.get = drift
        with self.assertRaisesRegex(protection.PolicyError, "during verification"):
            self.run_fixture(api)

    def test_classic_counter_probes(self):
        for field in ("enforce_admins", "allow_force_pushes", "allow_deletions",
                      "required_linear_history", "required_conversation_resolution"):
            api = FixtureAPI("classic"); api.classic[field]["enabled"] = not api.classic[field]["enabled"]
            with self.assertRaises(protection.PolicyError):
                self.run_fixture(api)
        for key in ("required_approving_review_count", "require_last_push_approval", "dismiss_stale_reviews"):
            api = FixtureAPI("classic")
            value = api.classic["required_pull_request_reviews"][key]
            api.classic["required_pull_request_reviews"][key] = 1 if type(value) is int else not value
            with self.assertRaises(protection.PolicyError):
                self.run_fixture(api)
        api = FixtureAPI("classic")
        api.classic["required_pull_request_reviews"]["bypass_pull_request_allowances"] = {"apps": [1]}
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)

    def test_unprotected_and_malformed_branch(self):
        for key, value in (("protected", False), ("commit", {"sha": "not-a-commit"})):
            api = FixtureAPI(); api.branch[key] = value
            with self.assertRaises(protection.PolicyError):
                self.run_fixture(api)
        api = FixtureAPI(); api.rules = []
        with self.assertRaises(protection.PolicyError):
            self.run_fixture(api)

    def test_malformed_contract(self):
        source = CONTRACT.read_text()
        for old, new in (("app_id = 15368", "app_id = true"), ("enforce_admins = true", "enforce_admins = 1"),
                         ("schema_version = 1", "schema_version = true")):
            with self.subTest(new=new), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "policy.toml"
                self.assertIn(old, source)
                path.write_text(source.replace(old, new))
                with self.assertRaises(protection.PolicyError):
                    protection.load_policy(path, "metaneutrons/aros-tools")


class HTTP(unittest.TestCase):
    def request(self, status=200, body=b'{}', *, headers=b'', code=0, allow_absent=False):
        response = subprocess.CompletedProcess([], code,
            f'HTTP/2.0 {status} response\r\n'.encode() + headers + b'\r\n' + body, b'private diagnostic')
        with patch.object(protection.subprocess, 'run', return_value=response) as run:
            result = protection.GitHub().get('repos/test/repo/rules/branches/main', allow_absent=allow_absent)
            self.assertEqual(run.call_args.kwargs['timeout'], 30)
            return result

    def test_success_and_only_explicit_404_fallback(self):
        self.assertEqual(self.request(body=b'[]'), [])
        self.assertIsNone(self.request(404, code=1, allow_absent=True))
        for status in (401, 403, 429, 500):
            with self.subTest(status=status), self.assertRaises(protection.PolicyError):
                self.request(status, code=1, allow_absent=True)
        with self.assertRaises(protection.PolicyError):
            self.request(404, code=1)
        with self.assertRaises(protection.PolicyError):
            self.request(200, code=1)

    def test_malformed_duplicate_and_paginated_responses(self):
        with self.assertRaises(protection.PolicyError):
            self.request(body=b'null', allow_absent=True)
        with self.assertRaises(protection.PolicyError):
            self.request(body=b'{"field":1,"field":2}')
        with self.assertRaises(ValueError):
            self.request(body=b'not json')
        with self.assertRaises(protection.PolicyError):
            self.request(headers=b'Link: <https://api.github.com/example>; rel="next"\r\n')
        with patch.object(protection.subprocess, 'run', side_effect=subprocess.TimeoutExpired('gh', 30)):
            with self.assertRaises(subprocess.TimeoutExpired):
                protection.GitHub().get('test')
        with patch.object(protection.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, b'', b'private')):
            with self.assertRaisesRegex(protection.PolicyError, 'no HTTP envelope'):
                protection.GitHub().get('test', allow_absent=True)


if __name__ == '__main__':
    unittest.main()
