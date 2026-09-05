#!/usr/bin/env python3
"""Prove the closed governance contract using classic protection or a ruleset.

Only an authenticated classic-protection 404 permits the ruleset path. Effective
branch rules identify the applicable source; its full definition must expose an
empty bypass list. Unknown/overlapping policies require review, never a guessed
union of guarantees. No repository setting is changed by this verifier.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[2]
BOOL_FIELDS = {
    "dismiss_stale_reviews", "require_code_owner_reviews", "require_last_push_approval",
    "required_conversation_resolution", "enforce_admins", "required_linear_history",
    "allow_force_pushes", "allow_deletions",
}
RULE_TYPES = {"deletion", "non_fast_forward", "required_linear_history", "pull_request",
              "required_status_checks"}


class PolicyError(ValueError):
    """Protection could not be proved; the caller must not mutate the ref."""


def require(condition, message):
    if not condition:
        raise PolicyError(message)


def unique(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, "duplicate API JSON field")
        value[key] = item
    return value


class GitHub:
    def get(self, endpoint, *, allow_absent=False):
        result = subprocess.run(
            ["gh", "api", endpoint, "--include", "--method", "GET",
             "-H", "X-GitHub-Api-Version: 2022-11-28"],
            capture_output=True, timeout=30, check=False,
        )
        # Do not copy stderr or error bodies into a publication log. A denied
        # Administration read must not look like missing classic protection.
        require(len(result.stdout) <= 2 * 1024 * 1024, "protection API response exceeds its bound")
        parts = re.split(rb"\r?\n\r?\n", result.stdout, maxsplit=1)
        require(len(parts) == 2, "protection API returned no HTTP envelope")
        status = re.match(rb"HTTP/[0-9.]+ ([0-9]{3})(?:\s|$)", parts[0])
        require(status is not None, "protection API returned no HTTP status")
        code = int(status[1])
        if allow_absent and code == 404 and result.returncode != 0:
            return None
        require(code == 200 and result.returncode == 0,
                f"cannot read protection API {endpoint} (HTTP {code})")
        # Complete single resources/arrays, not the paginated ruleset listing.
        require(not re.search(rb"(?im)^link:.*rel=\"next\"", parts[0]),
                "effective branch rules unexpectedly require pagination")
        value = json.loads(parts[1], object_pairs_hook=unique)
        require(isinstance(value, (dict, list)), "protection API returned no structured resource")
        return value


def checks(value, field):
    require(isinstance(value, list) and value, "required checks are missing")
    result = []
    for entry in value:
        require(isinstance(entry, dict) and isinstance(entry.get("context"), str)
                and entry["context"] and type(entry.get(field)) is int and entry[field] > 0,
                "every required check needs a context and positive App binding")
        result.append((entry["context"], entry[field]))
    require(len({name for name, _ in result}) == len(result), "duplicate required check context")
    return sorted(result)


def load_policy(path, repository):
    with path.open("rb") as stream:
        contract = tomllib.load(stream)
    require(type(contract.get("schema_version")) is int and contract["schema_version"] == 1,
            "unsupported repository governance schema")
    policy = contract["repositories"][repository]
    require(set(policy) == BOOL_FIELDS | {"branch", "required_approving_review_count",
                                         "required_status_checks"}, "governance policy fields differ")
    require(all(type(policy[field]) is bool for field in BOOL_FIELDS), "policy boolean is malformed")
    require(isinstance(policy["branch"], str) and
            re.fullmatch(r"[A-Za-z0-9._/-]+", policy["branch"]), "governance branch is malformed")
    count = policy["required_approving_review_count"]
    require(type(count) is int and 0 <= count <= 6, "approval count must be an integer from zero through six")
    require(count != 0 or not policy["require_last_push_approval"],
            "last-push approval cannot be required when approvals are disabled")
    checks(policy["required_status_checks"], "app_id")
    require(all(set(row) == {"context", "app_id"} for row in policy["required_status_checks"]),
            "governance check fields differ")
    return policy


def same(actual, expected, label):
    require(type(actual) is type(expected) and actual == expected, label + " differs from governance contract")


def verify_classic(actual, policy):
    same(actual["required_status_checks"]["strict"], True, "strict checks")
    same(checks(actual["required_status_checks"]["checks"], "app_id"),
         checks(policy["required_status_checks"], "app_id"), "required checks")
    reviews = actual["required_pull_request_reviews"]
    for key in ("required_approving_review_count", "dismiss_stale_reviews",
                "require_code_owner_reviews", "require_last_push_approval"):
        same(reviews[key], policy[key], key)
    bypass = reviews.get("bypass_pull_request_allowances", {})
    require(isinstance(bypass, dict) and all(not value for value in bypass.values()),
            "classic pull-request bypass is not permitted")
    for key in ("required_conversation_resolution", "enforce_admins", "required_linear_history",
                "allow_force_pushes", "allow_deletions"):
        same(actual[key]["enabled"], policy[key], key)


def rule_map(rules):
    require(isinstance(rules, list) and len(rules) == len(RULE_TYPES),
            "effective rules are missing or include an unreviewed overlay")
    require(all(isinstance(row, dict) and row.get("type") in RULE_TYPES for row in rules),
            "unknown effective rule requires governance review")
    require({row["type"] for row in rules} == RULE_TYPES, "duplicate or missing effective rule")
    return {row["type"]: row for row in rules}


def verify_ruleset(api, repository, branch, rules, policy):
    effective = rule_map(rules)
    ids = {row.get("ruleset_id") for row in rules}
    require(len(ids) == 1 and all(type(value) is int and value > 0 for value in ids),
            "overlapping or unidentified rulesets require governance review")
    identity = next(iter(ids))
    require(all(row.get("ruleset_source_type") == "Repository" and
                row.get("ruleset_source") == repository for row in rules),
            "inherited/foreign rulesets require governance review")
    definition = api.get(f"repos/{repository}/rulesets/{identity}")
    same(definition["id"], identity, "ruleset identity")
    same(definition["source_type"], "Repository", "ruleset source type")
    same(definition["source"], repository, "ruleset source")
    same(definition["target"], "branch", "ruleset target")
    same(definition["enforcement"], "active", "ruleset enforcement")
    # GitHub omits bypass_actors without the required read permission. Missing
    # is not empty: it is an unproved privilege boundary.
    same(definition["bypass_actors"], [], "ruleset bypass actors")
    same(policy["enforce_admins"], True, "ruleset administrator enforcement")
    conditions = definition["conditions"]
    require(set(conditions) == {"ref_name"}, "unexpected ruleset condition")
    ref = conditions["ref_name"]
    same(ref["exclude"], [], "ruleset exclusions")
    include = ref["include"]
    require(isinstance(include, list) and include and len(include) == len(set(include)) and
            set(include) <= {f"refs/heads/{branch}", "~DEFAULT_BRANCH"},
            "ruleset scope must explicitly cover the contracted branch")
    if "~DEFAULT_BRANCH" in include:
        same(api.get(f"repos/{repository}")["default_branch"], branch, "default branch")
    full = rule_map(definition["rules"])
    for kind, row in effective.items():
        same(row.get("parameters", {}), full[kind].get("parameters", {}), "effective " + kind)
    for kind, field, expected in (("deletion", "allow_deletions", False),
                                  ("non_fast_forward", "allow_force_pushes", False),
                                  ("required_linear_history", "required_linear_history", True)):
        same(policy[field], expected, field)
        same(full[kind].get("parameters", {}), {}, kind + " parameters")
    status = full["required_status_checks"]["parameters"]
    same(status["strict_required_status_checks_policy"], True, "strict checks")
    same(status["do_not_enforce_on_create"], False, "check enforcement on creation")
    same(checks(status["required_status_checks"], "integration_id"),
         checks(policy["required_status_checks"], "app_id"), "required checks")
    reviews = full["pull_request"]["parameters"]
    for field, key in (("required_approving_review_count", "required_approving_review_count"),
                       ("dismiss_stale_reviews_on_push", "dismiss_stale_reviews"),
                       ("require_code_owner_review", "require_code_owner_reviews"),
                       ("require_last_push_approval", "require_last_push_approval"),
                       ("required_review_thread_resolution", "required_conversation_resolution")):
        same(reviews[field], policy[key], key)
    same(reviews.get("required_reviewers", []), [], "extra required reviewers")
    same(api.get(f"repos/{repository}/rules/branches/{quote(branch, safe='')}"), rules,
         "effective rules during verification")


def verify(api, repository, policy):
    branch = policy["branch"]
    endpoint = f"repos/{repository}/branches/{quote(branch, safe='')}"
    state = api.get(endpoint)
    same(state["protected"], True, "branch protection")
    sha = state["commit"]["sha"]
    require(isinstance(sha, str) and re.fullmatch(r"[0-9a-f]{40}", sha), "branch SHA is malformed")
    classic = api.get(endpoint + "/protection", allow_absent=True)
    rules = api.get(f"repos/{repository}/rules/branches/{quote(branch, safe='')}")
    require(isinstance(rules, list), "effective rules API did not return an array")
    if classic is not None:
        require(not rules, "classic protection and ruleset overlay require governance review")
        verify_classic(classic, policy)
    else:
        verify_ruleset(api, repository, branch, rules, policy)
    return sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--contract", type=Path, default=ROOT / "contracts/repository-governance-v1.toml")
    args = parser.parse_args()
    require(re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repository), "repository must be owner/repository")
    require(os.environ.get("GH_TOKEN"), "GH_TOKEN is required for protection checks")
    print(verify(GitHub(), args.repository, load_policy(args.contract, args.repository)))


if __name__ == "__main__":
    try:
        main()
    except (PolicyError, OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
        print(f"::error::AP7030 protection verification failed: {error}", file=sys.stderr)
        sys.exit(1)
