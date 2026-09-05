#!/usr/bin/env python3
"""Offline positive/negative contracts for Homebrew App publication."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
TOKEN = "synthetic-secret-must-not-appear"
SLUG = "metaneutrons-homebrew"
TAP = "metaneutrons/homebrew-tap"

MOCK_GH = """#!/usr/bin/env python3
import json, os, sys
fixture = json.loads(os.environ["GH_FIXTURE"])
key = sys.argv[2]
assert sys.argv[1] == "api" and not any(x in sys.argv for x in ["POST", "PUT", "PATCH", "DELETE"])
if key.startswith("installation/"):
    assert sys.argv[3:] == ["--paginate", "--slurp"]
value = fixture[key]
if value == "API_ERROR":
    print(os.environ["GH_TOKEN"], file=sys.stderr)  # simulated noisy upstream error
    sys.exit(1)
if value == "BAD_JSON":
    print("{")
else:
    print(json.dumps(value))
"""


class AppPreflight(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-test-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        gh = self.work / "gh"
        gh.write_text(MOCK_GH)
        gh.chmod(0o755)
        self.fixture = {
            "installation/repositories?per_page=100": [
                {"total_count": 1, "repositories": [{"full_name": TAP}]}
            ],
            f"repos/{TAP}": {"full_name": TAP, "permissions": {"push": False}},
            f"users/{SLUG}%5Bbot%5D": {"login": f"{SLUG}[bot]", "type": "Bot", "id": 123},
        }
        self.env = dict(os.environ, PATH=f"{self.work}:{os.environ['PATH']}",
                        GH_TOKEN=TOKEN, HOMEBREW_APP_SLUG=SLUG,
                        HOMEBREW_INSTALLATION_ID="123", GITHUB_OUTPUT=str(self.work / "output"))

    def check(self, error=None):
        self.env["GH_FIXTURE"] = json.dumps(self.fixture)
        result = subprocess.run(["bash", str(ROOT / "scripts/release/verify-homebrew-app.sh")],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertNotIn(TOKEN, result.stdout + result.stderr)
        if error:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(error, result.stderr)
            self.assertFalse((self.work / "output").exists())
        else:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((self.work / "output").read_text(),
                             f"bot-name={SLUG}[bot]\nbot-email=123+{SLUG}[bot]@users.noreply.github.com\n")

    def test_valid(self):
        self.check()

    def test_no_token(self):
        self.env["GH_TOKEN"] = ""
        self.check("AP7110")

    def test_wrong_app(self):
        self.env["HOMEBREW_APP_SLUG"] = "different-app"
        self.check("AP7111")

    def test_invalid_installation(self):
        self.env["HOMEBREW_INSTALLATION_ID"] = "0"
        self.check("AP7111")

    def test_pat_rejected_and_error_body_not_logged(self):
        self.fixture["installation/repositories?per_page=100"] = "API_ERROR"
        self.check("AP7112")

    def test_malformed_json(self):
        self.fixture["installation/repositories?per_page=100"] = "BAD_JSON"
        self.check("AP7112")

    def test_missing_page(self):
        self.fixture["installation/repositories?per_page=100"] = []
        self.check("AP7113")

    def test_non_integer_count(self):
        self.fixture["installation/repositories?per_page=100"][0]["total_count"] = True
        self.check("AP7113")

    def test_wrong_repository(self):
        self.fixture["installation/repositories?per_page=100"][0]["repositories"][0]["full_name"] = "other/tap"
        self.check("AP7113")

    def test_additional_repository_on_later_page(self):
        self.fixture["installation/repositories?per_page=100"].append(
            {"total_count": 1, "repositories": [{"full_name": "other/tap"}]})
        self.check("AP7113")

    def test_repository_identity_mismatch(self):
        self.fixture[f"repos/{TAP}"]["full_name"] = "other/tap"
        self.check("AP7114")

    def test_personal_identity(self):
        self.fixture[f"users/{SLUG}%5Bbot%5D"]["type"] = "User"
        self.check("AP7115")

    def test_bad_bot_id(self):
        self.fixture[f"users/{SLUG}%5Bbot%5D"]["id"] = True
        self.check("AP7115")

    def test_output_unwritable(self):
        self.env["GITHUB_OUTPUT"] = str(self.work / "missing" / "output")
        self.check("AP7116")


class WorkflowPolicy(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-policy-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        shutil.copytree(ROOT / ".github", self.work / ".github")
        self.action = self.work / ".github/actions/homebrew-token/action.yml"
        self.publish = self.work / ".github/workflows/publish-ecosystem.yml"

    def check(self, marker=None):
        result = subprocess.run(["bash", str(ROOT / "scripts/release/check-actions-policy.sh"), str(self.work)],
                                capture_output=True, text=True, timeout=10)
        if marker:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(marker, result.stderr)
        else:
            self.assertEqual(result.returncode, 0, result.stderr)

    def replace(self, path, old, new):
        source = path.read_text()
        self.assertIn(old, source)
        path.write_text(source.replace(old, new))

    def test_valid(self):
        self.check()

    def test_legacy_pat(self):
        self.replace(self.publish, "secrets.HOMEBREW_APP_PRIVATE_KEY", "secrets.HOMEBREW_TAP_TOKEN")
        self.check("legacy Homebrew PAT")

    def test_admin_write(self):
        self.replace(self.action, "permission-administration: read", "permission-administration: write")
        self.check("unexpected or duplicate permission grant")

    def test_missing_write_permission(self):
        self.replace(self.action, "permission-contents: write", "permission-contents: read")
        self.check("unexpected or duplicate permission grant")

    def test_overbroad_repository_scope(self):
        self.replace(self.action, "repositories: homebrew-tap", "repositories: homebrew-tap,other")
        self.check("credential factory target")

    def test_no_revocation(self):
        self.replace(self.action, "permission-actions: read", "permission-actions: read\n        skip-token-revoke: true")
        self.check("forbidden credential-factory override")

    def test_unpinned_nested_action(self):
        self.replace(self.action, "@bcd2ba49218906704ab6c1aa796996da409d3eb1", "@v3")
        self.check("external action is not pinned")

    def test_no_renewal(self):
        self.replace(self.publish, "id: homebrew-merge-token", "id: obsolete-token")
        self.check("must renew")

    def test_stale_token_for_merge(self):
        self.replace(self.publish, "steps.homebrew-merge-token.outputs.token", "steps.homebrew-token.outputs.token")
        self.check("newly verified App token")

    def test_wait_unbounded(self):
        self.replace(self.publish, "timeout-minutes: 35", "timeout-minutes: 90")
        self.check("must be bounded")

    def test_wrong_environment(self):
        self.replace(self.publish, "environment: homebrew-publication", "environment: release")
        self.check("must stay inside homebrew-publication")


if __name__ == "__main__":
    unittest.main()
