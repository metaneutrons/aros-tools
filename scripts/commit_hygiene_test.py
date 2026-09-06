"""Exercise the actual CI hygiene script, including normal branch refreshes."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class CommitHygieneTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.env = {
            **os.environ,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "Fixture",
            "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_NAME": "Fixture",
            "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
            "PR_TITLE": "docs: management design",
            "PR_BODY": "Reviewed design only.",
        }
        self.git("init", "--bare", ".")
        self.tree = self.git("mktree", input="")
        self.base = self.commit("chore: base")
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        step = workflow.split("      - name: Check the title, the body and every commit\n", 1)[1]
        body = step.split("        run: |\n", 1)[1].split("      - name:", 1)[0]
        self.script = "\n".join(line[10:] for line in body.splitlines())

    def git(self, *args, input=None):
        return subprocess.run(
            ["git", *args], cwd=self.root, env=self.env, input=input,
            check=True, text=True, capture_output=True,
        ).stdout.strip()

    def commit(self, message, *parents):
        arguments = ["commit-tree", self.tree]
        for parent in parents:
            arguments.extend(["-p", parent])
        return self.git(*arguments, input=message + "\n")

    def check(self, head, success):
        result = subprocess.run(
            ["bash", "-c", self.script], cwd=self.root,
            env={**self.env, "BASE_SHA": self.base, "HEAD_SHA": head},
            text=True, capture_output=True,
        )
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)

    def test_real_generated_merge_passes(self):
        left = self.commit("docs: left", self.base)
        right = self.commit("docs: right", self.base)
        self.check(self.commit("Merge branch 'main'", left, right), True)

    def test_fake_merge_and_plain_invalid_subject_fail(self):
        for subject in ("Merge branch 'main'", "unclassified change"):
            self.check(self.commit(subject, self.base), False)

    def test_merge_does_not_bypass_attribution_or_ancestor_checks(self):
        right = self.commit("docs: right", self.base)
        left = self.commit("docs: left", self.base)
        attribution = "\n\nCo-Authored-By: " + "Claude <fixture@example.invalid>"
        self.check(self.commit("Merge branch 'main'" + attribution, left, right), False)
        invalid = self.commit("unclassified change", self.base)
        self.check(self.commit("Merge branch 'main'", invalid, right), False)

    def test_pr_title_still_requires_conventional_commit(self):
        self.env["PR_TITLE"] = "Merge branch 'main'"
        self.check(self.commit("docs: valid", self.base), False)


if __name__ == "__main__":
    unittest.main()
