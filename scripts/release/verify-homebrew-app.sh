#!/usr/bin/env bash
# Read-only preflight. Never echo credentials or raw API error bodies.
set -euo pipefail

python3 - <<'PY'
import json
import os
import re
import subprocess
import sys

TAP = "metaneutrons/homebrew-tap"
SLUG = "metaneutrons-homebrew"

def fail(code, message):
    print(f"::error::{code} {message}", file=sys.stderr)
    raise SystemExit(1)

if not os.environ.get("GH_TOKEN"):
    fail("AP7110", "Homebrew App token is missing; configure the protected environment")
if (os.environ.get("HOMEBREW_APP_SLUG") != SLUG
        or not re.fullmatch(r"[1-9][0-9]*", os.environ.get("HOMEBREW_INSTALLATION_ID", ""))):
    fail("AP7111", "Homebrew credential was not issued by the expected installed App")

def api(endpoint, *, paginate=False):
    args = ["gh", "api", endpoint]
    if paginate:
        args.extend(["--paginate", "--slurp"])
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=45, check=False)
    except (OSError, subprocess.TimeoutExpired):
        fail("AP7112", "Homebrew App API probe unavailable or timed out; retry the preflight")
    if result.returncode:
        fail("AP7112", "Homebrew App API probe rejected; check installation and accepted permissions")
    try:
        return json.loads(result.stdout)
    except (ValueError, TypeError):
        fail("AP7112", "Homebrew App API probe returned invalid JSON")

# This endpoint rejects PATs. Enumerate every page, not just the first match.
pages = api("installation/repositories?per_page=100", paginate=True)
if (not isinstance(pages, list) or not pages
        or any(not isinstance(page, dict) or type(page.get("total_count")) is not int
               or page["total_count"] != 1 or not isinstance(page.get("repositories"), list)
               for page in pages)):
    fail("AP7113", "Homebrew installation token must grant exactly one repository")
repositories = [repo for page in pages for repo in page["repositories"]]
if (len(repositories) != 1 or not isinstance(repositories[0], dict)
        or repositories[0].get("full_name") != TAP):
    fail("AP7113", "Homebrew installation token reaches an unexpected repository")
repo = api(f"repos/{TAP}")
if not isinstance(repo, dict) or repo.get("full_name") != TAP:
    fail("AP7114", "Homebrew App repository identity is inconsistent")
# GET /repos returns user-role flags (observed all false for this App), not
# the installation grant. Write permissions are required by the pinned token
# factory; GitHub rejects issuance if that installation cannot grant them.

# Installation tokens have no personal /user identity.
bot = api(f"users/{SLUG}%5Bbot%5D")
login = f"{SLUG}[bot]"
if (not isinstance(bot, dict) or bot.get("login") != login or bot.get("type") != "Bot"
        or type(bot.get("id")) is not int or bot["id"] <= 0):
    fail("AP7115", "Homebrew App bot identity is missing or inconsistent")
output = os.environ.get("GITHUB_OUTPUT")
if output:
    try:
        with open(output, "a", encoding="utf-8") as stream:
            stream.write(f"bot-name={login}\nbot-email={bot['id']}+{login}@users.noreply.github.com\n")
    except OSError:
        fail("AP7116", "Cannot record the verified Homebrew bot identity")
print("Homebrew App preflight passed: one tap, verified repository and bot identity")
PY
