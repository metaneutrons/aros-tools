"""Keep development integration identity separate from immutable producer pins."""

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class SourceContractTests(unittest.TestCase):
    def validate(self, integration="", workflow_commit="a" * 40):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            contract = root / "source.toml"
            contract.write_text(
                'schema_version = 1\n[source]\nrepository = "owner/source"\n'
                f'commit = "{"a" * 40}"\n[producer]\nrepository = "owner/producer"\n'
                f'commit = "{"b" * 40}"\nworkflow = ".github/workflows/build.yml"\n'
                + integration
            )
            workflow = root / "build.yml"
            workflow.write_text('env:\n  AROS_SOURCE_REPOSITORY: owner/source\n'
                                f'  AROS_SOURCE_COMMIT: {workflow_commit}\n')
            return subprocess.run(
                [sys.executable, str(ROOT / "scripts/validate-source-contract.py"),
                 "--contract", str(contract), "--producer-workflow", str(workflow)],
                capture_output=True, text=True, timeout=30,
            )

    def test_original_contract_remains_valid(self):
        result = self.validate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_independent_integration_identity_preserves_producer_check(self):
        integration = ('[integration]\nrepository = "owner/source"\n'
                       f'commit = "{"c" * 40}"\n')
        result = self.validate(integration)
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.validate(integration, workflow_commit="c" * 40)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer commit mismatch", result.stderr)

    def test_wrong_integration_repository_is_rejected(self):
        result = self.validate('[integration]\nrepository = "other/source"\n'
                               f'commit = "{"c" * 40}"\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("repositories must agree", result.stderr)

    def test_malformed_integration_identity_is_rejected(self):
        result = self.validate('[integration]\nrepository = "owner/source"\ncommit = "main"\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("integration.commit", result.stderr)

    def test_unknown_integration_field_is_rejected(self):
        result = self.validate('[integration]\nrepository = "owner/source"\n'
                               f'commit = "{"c" * 40}"\nunchecked = true\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("keys differ", result.stderr)


if __name__ == "__main__":
    unittest.main()
