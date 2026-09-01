#!/usr/bin/env python3
"""Regression fixtures for the public environment contract checker."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


CHECKER = Path(__file__).with_name("check-environment-contract.py")
SPEC = importlib.util.spec_from_file_location("environment_contract", CHECKER)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load environment contract checker")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def write_fixture(root: Path) -> Path:
    source = root / "crates/example/src/main.rs"
    source.parent.mkdir(parents=True)
    source.write_text(
        'let _ = std::env::var("AROS_PUBLIC");\n'
        '#[arg(long, env = "AROS_TEST_ONLY")]\n',
        encoding="utf-8",
    )
    docs = root / "docs/configuration.md"
    docs.parent.mkdir(parents=True)
    docs.write_text("`AROS_PUBLIC` `AROS_TEST_ONLY` `HOME`\n", encoding="utf-8")
    contract = root / "contracts/environment.toml"
    contract.parent.mkdir(parents=True)
    contract.write_text(
        'schema = 1\ndocumentation = "docs/configuration.md"\n'
        'public = ["AROS_PUBLIC"]\nambient = ["HOME"]\n'
        'test_internal = ["AROS_TEST_ONLY"]\n',
        encoding="utf-8",
    )
    ambient = root / "crates/example/tests/ambient.rs"
    ambient.parent.mkdir(parents=True)
    ambient.write_text('let _ = std::env::var_os("HOME");\n', encoding="utf-8")
    return contract


class EnvironmentContractTests(unittest.TestCase):
    def test_exact_contract_and_documentation_are_enforced(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            contract = write_fixture(repository)
            self.assertEqual(MODULE.validate(repository, contract), [])

            source = repository / "crates/example/src/main.rs"
            source.write_text(
                source.read_text(encoding="utf-8")
                + 'let _ = std::env::var("AROS_UNDECLARED");\n',
                encoding="utf-8",
            )
            self.assertIn(
                "absent from contract", "\n".join(MODULE.validate(repository, contract))
            )

            source.write_text(
                source.read_text(encoding="utf-8").replace(
                    'let _ = std::env::var("AROS_UNDECLARED");\n', ""
                ),
                encoding="utf-8",
            )
            (repository / "docs/configuration.md").write_text(
                "`AROS_PUBLIC` `HOME`\n", encoding="utf-8"
            )
            self.assertIn(
                "absent from documentation",
                "\n".join(MODULE.validate(repository, contract)),
            )


if __name__ == "__main__":
    unittest.main()
