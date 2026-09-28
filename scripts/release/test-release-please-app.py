#!/usr/bin/env python3
"""Regression checks for repository-scoped, PR-only Release Please App auth."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = (ROOT / ".github/workflows/release-please.yml").read_text()


class ReleasePleaseApp(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-release-app-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.workflow = self.work / ".github/workflows/release-please.yml"
        self.workflow.parent.mkdir(parents=True)
        self.workflow.write_text(SOURCE)

    def policy(self, old=None, new=None, marker=None):
        if old:
            self.assertIn(old, SOURCE)
            self.workflow.write_text(SOURCE.replace(old, new))
        result = subprocess.run(["bash", str(ROOT / "scripts/release/check-actions-policy.sh"), str(self.work)],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1 if marker else 0, result.stderr)
        if marker:
            self.assertIn(marker, result.stderr)

    def test_valid_policy(self):
        self.policy()

    def test_builtin_token(self):
        self.policy("token: ${{ steps.app-token.outputs.token }}", "token: ${{ github.token }}",
                    "forbidden Release Please credential")

    def test_wrong_environment(self):
        self.policy("environment: release-please", "environment: release",
                    "missing Release Please App contract")

    def test_other_repository(self):
        self.policy("repositories: aros-tools", "repositories: aros-tools,other",
                    "App target must be exactly")

    def test_excess_permissions(self):
        self.policy("permission-contents: write", "permission-contents: write\n          permission-actions: write",
                    "must request only Contents and Pull requests write")

    def test_no_token_revocation(self):
        self.policy("permission-contents: write", "permission-contents: write\n          skip-token-revoke: true",
                    "forbidden Release Please credential")

    def test_no_automatic_release(self):
        self.policy("skip-github-release: true", "skip-github-release: false",
                    "missing Release Please App contract")

    def test_no_duplicate_dispatch(self):
        self.policy("set -euo pipefail", "set -euo pipefail\n          gh workflow run ci.yml",
                    "duplicate dispatch")

    def probe(self, repositories, *, slug="metaneutrons-release-please", code=None):
        # Exercise the actual workflow preflight, not a rewritten copy.
        step = SOURCE.split("      - name: Verify the Release Please App identity and token scope\n", 1)[1]
        run = textwrap.dedent(step.split("        run: |\n", 1)[1].split("      - name:", 1)[0])
        gh = self.work / "gh"
        gh.write_text("#!/usr/bin/env python3\nimport os\nprint(os.environ['API_FIXTURE'])\n")
        gh.chmod(0o755)
        env = dict(os.environ, PATH=f"{self.work}:{os.environ['PATH']}",
                   APP_SLUG=slug, INSTALLATION_ID="123", GITHUB_REPOSITORY="example/tools",
                   GH_TOKEN="synthetic-not-a-real-secret", API_FIXTURE=json.dumps(repositories))
        result = subprocess.run(["bash", "-c", run], env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1 if code else 0, result.stderr)
        self.assertNotIn(env["GH_TOKEN"], result.stdout + result.stderr)
        if code:
            self.assertIn(code, result.stdout + result.stderr)

    def test_real_preflight_valid(self):
        self.probe([{"total_count": 1, "repositories": [{"full_name": "example/tools"}]}])

    def test_real_preflight_wrong_app(self):
        self.probe([], slug="wrong-app", code="AP7140")

    def test_real_preflight_extra_repository(self):
        self.probe([{"total_count": 2, "repositories": [
            {"full_name": "example/tools"}, {"full_name": "other/repo"}]}], code="AP7141")

    def test_real_preflight_hidden_later_page(self):
        self.probe([{"total_count": 1, "repositories": [{"full_name": "example/tools"}]},
                    {"total_count": 1, "repositories": [{"full_name": "other/repo"}]}], code="AP7141")

    def test_real_preflight_missing_repository(self):
        self.probe([], code="AP7141")

    def probe_release_pr(self, heads, *, files=None, expected_success=True):
        # Exercise the workflow step with a stateful GitHub API fake. A freshly
        # updated App branch can precede the pull-request API's head refresh.
        step = SOURCE.split("      - name: Verify the exact App-authored release PR\n", 1)[1]
        run = textwrap.dedent(step.split("        run: |\n", 1)[1].split("      - name:", 1)[0])
        run = run.replace("sleep 2", ":")
        gh = self.work / "gh"
        gh.write_text(textwrap.dedent("""\
            #!/usr/bin/env python3
            import json
            import os
            from pathlib import Path
            import sys

            args = sys.argv[1:]
            if '/files?' in ' '.join(args):
                print('\\n'.join(json.loads(os.environ['API_FILES'])))
                raise SystemExit(0)
            counter = Path(os.environ['API_COUNTER'])
            index = int(counter.read_text()) if counter.exists() else 0
            heads = json.loads(os.environ['API_HEADS'])
            sha = heads[min(index, len(heads) - 1)]
            counter.write_text(str(index + 1))
            if '--jq' in args:
                print(sha)
            else:
                print(json.dumps({
                    'state': 'open', 'title': 'chore(main): release 0.3.14',
                    'base': {'ref': 'main'},
                    'head': {'sha': sha, 'ref': 'release-please--branches--main--components--aros-tools',
                             'repo': {'full_name': 'example/tools'}},
                }))
            """))
        gh.chmod(0o755)
        env = dict(os.environ, PATH=f"{self.work}:{os.environ['PATH']}",
                   GITHUB_REPOSITORY="example/tools", GH_TOKEN="synthetic-not-a-real-secret",
                   PR_JSON=json.dumps({'number': 123, 'headBranchName':
                                       'release-please--branches--main--components--aros-tools',
                                       'baseBranchName': 'main'}),
                   API_HEADS=json.dumps(heads), API_COUNTER=str(self.work / 'api-count'),
                   API_FILES=json.dumps(files or ['.release-please-manifest.json', 'CHANGELOG.md',
                                                  'Cargo.lock', 'Cargo.toml']))
        result = subprocess.run(["bash", "-c", run], env=env, capture_output=True,
                                text=True, timeout=10)
        self.assertEqual(result.returncode, 0 if expected_success else 1,
                         result.stdout + result.stderr)
        return result

    def test_release_pr_head_converges(self):
        old, new = 'a' * 40, 'b' * 40
        result = self.probe_release_pr([old, new, new, new])
        self.assertIn(new, result.stdout)

    def test_release_pr_head_never_stabilizes(self):
        old, new = 'a' * 40, 'b' * 40
        result = self.probe_release_pr([old, new] * 5, expected_success=False)
        self.assertIn('did not stabilize', result.stdout + result.stderr)

    def test_release_pr_rejects_unexpected_file_after_convergence(self):
        result = self.probe_release_pr(['b' * 40],
                                       files=['.release-please-manifest.json', 'CHANGELOG.md',
                                              'Cargo.lock', 'Cargo.toml', 'src/evil.rs'],
                                       expected_success=False)
        self.assertIn('unexpected files', result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
