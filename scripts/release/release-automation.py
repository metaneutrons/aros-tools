#!/usr/bin/env python3
"""Admit an exact Release Please merge, then start its immutable qualification.

Release Please selects the version/commit and creates a private draft. Its
native force-tag-creation option creates lightweight tags, so this repository
uses a narrow annotated-tag adapter instead. No existing ref or release is
replaced and no failed production mutation is automatically retried.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
OID = re.compile(r"[0-9a-f]{40}")
# Admission consumes merge_commit_sha from the reviewed PR response contract.
# GitHub removed that field in 2026-03-10; this supported version retains it.
REST_API_VERSION = "2022-11-28"
RULESET_BYPASS_QUERY = """query($nodeID: ID!) {
  node(id: $nodeID) {
    __typename
    ... on RepositoryRuleset {
      id
      databaseId
      bypassActors(first: 100) {
        totalCount
        nodes { __typename }
        pageInfo { hasNextPage }
      }
    }
  }
}"""
BOT = {"metaneutrons-release-please[bot]", "app/metaneutrons-release-please"}
FILES = {".release-please-manifest.json", "CHANGELOG.md", "Cargo.toml", "Cargo.lock"}
REQUIRED_JOBS = {
    "ci.yml": {"Plan host matrix", "Formatting, architecture and Clippy",
               "Tests (linux-x86_64)", "Tests (linux-aarch64)", "Tests (macos-aarch64)"},
    "codeql.yml": {"Analyze (actions)", "Analyze (rust)"},
}


def fail(message: str) -> "NoReturn":
    raise SystemExit(f"AP7150 {message}")


def api(path: str, *, method: str = "GET", payload: dict | None = None,
        pages: bool = False, absent: bool = False):
    args = ["gh", "api", "--method", method,
            "-H", f"X-GitHub-Api-Version: {REST_API_VERSION}", path]
    if pages:
        args.extend(["--paginate", "--slurp"])
    if payload is not None:
        args.extend(["--input", "-"])
    environment = dict(os.environ)
    # Ignore the retired privileged credential if an older workflow still
    # supplies it. All API requests use the job-scoped GH_TOKEN.
    environment.pop("GOVERNANCE_TOKEN", None)
    try:
        result = subprocess.run(args, input=json.dumps(payload) if payload is not None else None,
                                capture_output=True, text=True, timeout=45, check=False, env=environment)
    except (OSError, subprocess.TimeoutExpired):
        fail(f"GitHub {method} transport failed for {path}; no automatic mutation retry")
    if result.returncode:
        if absent and method == "GET" and "(HTTP 404)" in result.stderr:
            return None
        # Do not echo API response bodies or environment credentials on failure.
        status = re.search(r"\(HTTP ([0-9]{3})\)", result.stderr)
        detail = f" (HTTP {status[1]})" if status else ""
        fail(f"GitHub {method} failed{detail} for {path}; inspect the run before any recovery")
    if not result.stdout.strip():
        return None
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        fail(f"GitHub {method} returned invalid JSON for {path}")


def ruleset_bypass_connection(node_id: str) -> dict:
    """Read bypass actors through GraphQL using the ordinary job token."""
    if not isinstance(node_id, str) or not node_id or not os.environ.get("GH_TOKEN"):
        fail("tag ruleset GraphQL lookup lacks a REST node ID or job-scoped token")
    args = ["gh", "api", "graphql", "-H", f"X-GitHub-Api-Version: {REST_API_VERSION}",
            "-f", f"query={RULESET_BYPASS_QUERY}", "-f", f"nodeID={node_id}"]
    environment = dict(os.environ)
    environment.pop("GOVERNANCE_TOKEN", None)
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=45,
                                check=False, env=environment)
    except (OSError, subprocess.TimeoutExpired):
        fail("GitHub GraphQL transport failed for tag ruleset bypass lookup")
    if result.returncode:
        status = re.search(r"\(HTTP ([0-9]{3})\)", result.stderr)
        detail = f" (HTTP {status[1]})" if status else ""
        fail(f"GitHub GraphQL failed{detail} for tag ruleset bypass lookup")
    try:
        response = json.loads(result.stdout)
    except (json.JSONDecodeError, TypeError):
        fail("GitHub GraphQL returned invalid JSON for tag ruleset bypass lookup")
    # GitHub can report resolver errors in an HTTP 200 response. Never print
    # their potentially sensitive messages, and do not accept partial data.
    if not isinstance(response, dict) or "errors" in response:
        fail("GitHub GraphQL returned errors or a malformed response")
    data = response.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("node"), dict):
        fail("GitHub GraphQL returned a null or malformed tag ruleset node")
    return data["node"]


def has_no_ruleset_bypass(node_id: str, identifier: int) -> bool:
    node = ruleset_bypass_connection(node_id)
    if (not isinstance(node, dict) or node.get("id") != node_id
            or node.get("__typename") != "RepositoryRuleset"
            or type(node.get("databaseId")) is not int or node.get("databaseId") != identifier):
        fail("GraphQL tag ruleset identity does not match the REST ruleset")
    connection = node.get("bypassActors")
    if not isinstance(connection, dict):
        fail("GraphQL tag ruleset bypass connection is null or malformed")
    count = connection.get("totalCount")
    nodes = connection.get("nodes")
    page_info = connection.get("pageInfo")
    if type(count) is not int or not isinstance(nodes, list) or not isinstance(page_info, dict):
        fail("GraphQL tag ruleset bypass connection is malformed")
    if count != 0 or nodes != [] or page_info.get("hasNextPage") is not False:
        fail("tag ruleset has configured or incompletely reported bypass actors")
    return True


def context() -> tuple[str, str, str]:
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    if repo != "metaneutrons/aros-tools":
        fail("automation is restricted to metaneutrons/aros-tools")
    if os.environ.get("GITHUB_REF") != "refs/heads/main" or not os.environ.get("GH_TOKEN"):
        fail("automation requires main and a job-scoped token")
    environment = dict(os.environ)
    environment.pop("GOVERNANCE_TOKEN", None)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True,
                                   env=environment).strip()
    if OID.fullmatch(head) is None:
        fail("checkout commit is malformed")
    manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
    version = manifest.get(".", "")
    if not isinstance(version, str) or SEMVER.fullmatch(version) is None:
        fail("automatic promotion supports only canonical stable SemVer")
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text())
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    if cargo["workspace"]["package"]["version"] != version or any(
        package.get("version") != version for package in lock["package"] if "source" not in package
    ):
        fail("manifest, Cargo and lock versions differ")
    return repo, head, version


def event_allows(repo: str, head: str) -> bool:
    event_name = os.environ.get("GITHUB_EVENT_NAME")
    if event_name == "workflow_dispatch":
        return True
    if event_name != "workflow_run":
        fail("unsupported automation event")
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    run = event.get("workflow_run", {})
    return (run.get("event") == "push" and run.get("head_branch") == "main"
            and run.get("head_sha") == head and run.get("conclusion") == "success"
            and run.get("head_repository", {}).get("full_name") == repo
            and run.get("name") in {"Workspace CI", "CodeQL"})


def tag_policy(repo: str, receipt: dict | None = None) -> dict:
    if receipt is not None:
        if (not isinstance(receipt, dict) or set(receipt) != {"ruleset_id", "policy_sha256", "checked_at"}
                or type(receipt["ruleset_id"]) is not int or receipt["ruleset_id"] <= 0
                or not isinstance(receipt["policy_sha256"], str)
                or re.fullmatch(r"[0-9a-f]{64}", receipt["policy_sha256"]) is None
                or type(receipt["checked_at"]) is not int
                or not -60 <= int(time.time()) - receipt["checked_at"] <= 900):
            fail("governance receipt is malformed, expired or from the future")
    pages = api(f"repos/{repo}/rulesets?includes_parents=true&targets=tag&per_page=100",
                pages=True)
    if not isinstance(pages, list) or any(not isinstance(page, list) for page in pages):
        fail("tag ruleset list response is malformed")
    for item in (item for page in pages for item in page):
        if not isinstance(item, dict):
            fail("tag ruleset identity is malformed")
        identifier = item.get("id")
        if type(identifier) is not int or identifier <= 0:
            fail("tag ruleset identity is malformed")
        ruleset = api(f"repos/{repo}/rulesets/{identifier}")
        if not isinstance(ruleset, dict):
            fail("tag ruleset response is malformed")
        conditions = ruleset.get("conditions")
        rules = ruleset.get("rules")
        if not isinstance(conditions, dict) or not isinstance(rules, list) or any(
            not isinstance(rule, dict) for rule in rules
        ):
            fail("tag ruleset policy is malformed")
        refs = conditions.get("ref_name")
        if not isinstance(refs, dict):
            continue
        types = {rule.get("type") for rule in rules}
        if (type(ruleset.get("id")) is int and ruleset.get("id") == identifier
                and ruleset.get("target") == "tag"
                and ruleset.get("enforcement") == "active"
                and refs.get("include") == ["refs/tags/v*"] and refs.get("exclude") == []
                and {"update", "deletion"} <= types):
            node_id = ruleset.get("node_id")
            if not isinstance(node_id, str) or not node_id:
                fail("tag ruleset REST node identity is malformed")
            listed_node_id = item.get("node_id")
            if "node_id" in item and (not isinstance(listed_node_id, str) or listed_node_id != node_id):
                fail("tag ruleset REST node identity changed between list and detail")
            has_no_ruleset_bypass(node_id, identifier)
            if "bypass_actors" in ruleset and (
                type(ruleset["bypass_actors"]) is not list or ruleset["bypass_actors"] != []
            ):
                fail("REST tag ruleset reports malformed or nonempty bypass actors")
            public_policy = {key: ruleset[key] for key in ("id", "target", "enforcement", "conditions")}
            public_policy["rules"] = sorted(ruleset["rules"], key=lambda rule: json.dumps(rule, sort_keys=True))
            digest = hashlib.sha256(json.dumps(public_policy, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
            if receipt is None:
                return {"ruleset_id": identifier, "policy_sha256": digest, "checked_at": int(time.time())}
            if (receipt["ruleset_id"] == identifier
                    and receipt["policy_sha256"] == digest):
                return receipt
    fail("no active immutable v* tag ruleset with a verified empty bypass connection")


def admission(*, expected_draft: int | None = None, governance_receipt: dict | None = None) -> dict | None:
    repo, head, version = context()
    if not event_allows(repo, head):
        return None
    branch = api(f"repos/{repo}/branches/main")
    if branch.get("protected") is not True:
        fail("main is not protected")
    if branch.get("commit", {}).get("sha") != head:
        return None  # A newer main won the race; its own CI events will decide.
    # Exercise the protected environment credential on an ordinary main
    # checkpoint too. A permission defect must be visible before the human
    # merges a release candidate, without creating any draft or tag.
    governance = tag_policy(repo, governance_receipt)
    pulls = api(f"repos/{repo}/commits/{head}/pulls?per_page=100", pages=True)
    for pull in (pull for page in pulls for pull in page):
        if (pull.get("state") == "closed" and pull.get("merged_at")
                and pull.get("user", {}).get("login") in BOT
                and pull.get("title") == f"chore(main): release {version}"):
            merged_commit = pull.get("merge_commit_sha")
            if not isinstance(merged_commit, str) or OID.fullmatch(merged_commit) is None:
                fail("merged release PR lacks merge_commit_sha from the pinned REST API contract")
    matches = [p for page in pulls for p in page if
               p.get("state") == "closed" and p.get("merged_at")
               and p.get("merge_commit_sha") == head and p.get("base", {}).get("ref") == "main"
               and p.get("base", {}).get("repo", {}).get("full_name") == repo
               and p.get("head", {}).get("repo", {}).get("full_name") == repo
               and p.get("user", {}).get("login") in BOT
               and p.get("title") == f"chore(main): release {version}"
               and re.fullmatch(r"release-please--[A-Za-z0-9._-]+", p.get("head", {}).get("ref", ""))]
    if not matches:
        return None
    if len(matches) != 1 or type(matches[0].get("number")) is not int:
        fail("release merge has ambiguous PR identity")
    pull = matches[0]
    number = pull["number"]
    changed = api(f"repos/{repo}/pulls/{number}/files?per_page=100", pages=True)
    paths = [item.get("filename") for page in changed for item in page]
    if set(paths) != FILES or len(paths) != len(FILES) or any(
        item.get("status") not in {"modified", "added"} for page in changed for item in page
    ):
        fail("release PR is not the exact four-file version/changelog change")
    labels = {label.get("name") for label in pull.get("labels", [])}
    # Release Please consumes its pending label when it creates the draft.
    # Only this invocation's exact returned draft may cross that one-way phase;
    # fresh admission still rejects already tagged or retired candidates.
    release_labels = {label for label in labels if isinstance(label, str)
                      and label.startswith("autorelease: ")}
    expected_label = "autorelease: pending" if expected_draft is None else "autorelease: tagged"
    if release_labels != {expected_label}:
        return None  # Explicitly retired or already completed; never revive it.
    pending = api(f"repos/{repo}/pulls?state=closed&base=main&per_page=100", pages=True)
    pending_prs = [item["number"] for page in pending for item in page if item.get("merged_at")
                   and "autorelease: pending" in {label.get("name") for label in item.get("labels", [])}]
    expected_pending = [number] if expected_draft is None else []
    if pending_prs != expected_pending:
        fail("Release Please has other pending candidates; no release transition is permitted")
    runs = {}
    for workflow, required in REQUIRED_JOBS.items():
        pages = api(f"repos/{repo}/actions/workflows/{workflow}/runs?event=push&branch=main&head_sha={head}&per_page=100",
                    pages=True)
        candidates = [r for page in pages for r in page["workflow_runs"] if
                      r.get("event") == "push" and r.get("head_branch") == "main"
                      and r.get("head_sha") == head
                      and r.get("head_repository", {}).get("full_name") == repo]
        if not candidates:
            return None
        run = max(candidates, key=lambda r: r["id"])
        if run.get("status") != "completed" or run.get("conclusion") != "success":
            return None
        job_pages = api(f"repos/{repo}/actions/runs/{run['id']}/jobs?per_page=100", pages=True)
        jobs = [j for page in job_pages for j in page["jobs"]]
        success = {j.get("name") for j in jobs if j.get("conclusion") == "success"}
        if not required <= success or any(j.get("conclusion") not in {"success", "skipped"} for j in jobs):
            fail(f"{workflow} lacks the complete successful main qualification")
        runs[workflow] = {"id": run["id"], "attempt": run["run_attempt"]}
    tag = f"v{version}"
    if api(f"repos/{repo}/git/ref/tags/{tag}", absent=True) is not None:
        return None  # Consumed forever; never rerun the producer automatically.
    release_pages = api(f"repos/{repo}/releases?per_page=100", pages=True)
    releases = [release for page in release_pages for release in page if release.get("tag_name") == tag]
    if expected_draft is None:
        if releases:
            return None  # Partial/terminal state requires an explicit decision.
    else:
        # GitHub's tag/list index can lag a newly created draft. The numeric
        # ID returned by Release Please is authoritative before the tag exists.
        release = api(f"repos/{repo}/releases/{expected_draft}")
        if (release.get("id") != expected_draft or release.get("tag_name") != tag
                or any(item.get("id") != expected_draft for item in releases)):
            fail("Release Please draft identity changed or is ambiguous")
    return {"repository": repo, "commit": head, "version": version,
            "tag": tag, "pull_request": number, "runs": runs, "governance": governance}


def recheck(output: Path) -> None:
    try:
        saved = json.loads(os.environ.get("ADMISSION_JSON", ""))
        receipt = saved["governance"]
    except (json.JSONDecodeError, KeyError, TypeError):
        fail("trusted admission job returned no valid governance receipt")
    if saved != admission(governance_receipt=receipt):
        fail("read-only admission changed before draft creation")
    output.write_text(json.dumps(saved, sort_keys=True) + "\n")


def start(path: Path) -> None:
    saved = json.loads(path.read_text())
    if (os.environ.get("RP_CREATED") != "true" or os.environ.get("RP_TAG") != saved["tag"]
            or os.environ.get("RP_SHA") != saved["commit"]
            or os.environ.get("RP_VERSION") != saved["version"]
            or re.fullmatch(r"[1-9][0-9]*", os.environ.get("RP_ID", "")) is None):
        fail("Release Please did not create the exact admitted candidate")
    release_id = int(os.environ["RP_ID"])
    current = admission(expected_draft=release_id, governance_receipt=saved["governance"])
    if current != saved:
        fail("admission changed after draft creation; no tag or dispatch is permitted")
    repo, tag, head = saved["repository"], saved["tag"], saved["commit"]
    state = api(f"repos/{repo}/releases/{release_id}")
    if (state.get("id") != release_id or state.get("tag_name") != tag
            or state.get("target_commitish") != head or state.get("draft") is not True
            or state.get("prerelease") is not False or state.get("immutable", False) is not False
            or state.get("assets") != []):
        fail("candidate is not the exact fresh empty stable private draft")
    with tempfile.TemporaryDirectory(prefix="aros-release-notes-") as temporary:
        notes = Path(temporary) / "RELEASE_NOTES.md"
        subprocess.run(["python3", str(ROOT / "scripts/release/render-release-notes.py"),
                        "--changelog", str(ROOT / "CHANGELOG.md"), "--version", saved["version"],
                        "--output", str(notes)], check=True)
        body = notes.read_text()
    # Only the just-created empty private draft is normalized. The producer
    # later compares this body byte-for-byte with its signed release notes.
    normalized = api(f"repos/{repo}/releases/{release_id}", method="PATCH", payload={
        "tag_name": tag, "target_commitish": head,
        "name": f"aros-tools {tag}", "body": body, "draft": True, "prerelease": False})
    if (normalized.get("id") != release_id or normalized.get("body") != body
            or normalized.get("name") != f"aros-tools {tag}" or normalized.get("draft") is not True
            or normalized.get("tag_name") != tag or normalized.get("target_commitish") != head
            or normalized.get("prerelease") is not False or normalized.get("immutable", False) is not False
            or normalized.get("assets") != []):
        fail("private draft normalization did not preserve identity")
    annotated = api(f"repos/{repo}/git/tags", method="POST", payload={
        "tag": tag, "message": f"aros-tools {saved['version']}\n\nRelease Please PR #{saved['pull_request']}\nSource: {head}\n",
        "object": head, "type": "commit", "tagger": {
            "name": "github-actions[bot]", "email": "41898282+github-actions[bot]@users.noreply.github.com",
            "date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}})
    object_id = annotated.get("sha", "")
    if (OID.fullmatch(object_id) is None or annotated.get("tag") != tag
            or annotated.get("object", {}).get("type") != "commit"
            or annotated.get("object", {}).get("sha") != head):
        fail("annotated tag object differs from admitted source")
    api(f"repos/{repo}/git/refs", method="POST", payload={"ref": f"refs/tags/{tag}", "sha": object_id})
    ref = api(f"repos/{repo}/git/ref/tags/{tag}")
    if ref.get("object", {}).get("type") != "tag" or ref.get("object", {}).get("sha") != object_id:
        fail("remote annotated tag ref differs; do not retry or replace it")
    print(f"Created immutable candidate {tag} at {head} (tag object {object_id}, draft {release_id})", flush=True)
    api(f"repos/{repo}/actions/workflows/release.yml/dispatches", method="POST", payload={"ref": tag})
    print(f"Dispatched one qualification for {tag}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    admit = sub.add_parser("admit")
    admit.add_argument("--output", type=Path, required=True)
    admit.add_argument("--github-output", type=Path, required=True)
    restore = sub.add_parser("recheck")
    restore.add_argument("--output", type=Path, required=True)
    transition = sub.add_parser("start")
    transition.add_argument("--admission", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "start":
        start(args.admission)
    elif args.command == "recheck":
        recheck(args.output)
    else:
        candidate = admission()
        if candidate is not None:
            args.output.write_text(json.dumps(candidate, sort_keys=True) + "\n")
        with args.github_output.open("a") as output:
            output.write(f"ready={'true' if candidate is not None else 'false'}\n")
            if candidate is not None:
                output.write("candidate=" + json.dumps(candidate, sort_keys=True) + "\n")
        if candidate:
            # This closed, non-secret snapshot remains available even if the
            # later dispatch is interrupted or its response is lost.
            print("Release candidate admitted: " + json.dumps(candidate, sort_keys=True))
        else:
            print("No new fully qualified release candidate")


if __name__ == "__main__":
    main()
