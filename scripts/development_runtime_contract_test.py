#!/usr/bin/env python3
"""Regression tests for friendly development-runtime failures."""

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path


class DevelopmentRuntimeContractTest(unittest.TestCase):
    def test_python_310_is_rejected_without_traceback(self) -> None:
        helper = Path(__file__).with_name("check-development-runtimes.py")
        completed = subprocess.run(
            [sys.executable, str(helper), "--python-version-for-test", "3.10"],
            capture_output=True,
            text=True,
            env=dict(os.environ, AROS_RUNTIME_CONTRACT_FIXTURE="1"),
            check=False,
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("Python >= 3.11 with tomllib is required", completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)


if __name__ == "__main__":
    unittest.main()
