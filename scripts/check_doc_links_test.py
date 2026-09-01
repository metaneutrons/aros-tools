"""Regression tests for the generated documentation link gate."""

from __future__ import annotations

import importlib.util
import pathlib
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("check-doc-links.py")
SPEC = importlib.util.spec_from_file_location("check_doc_links", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class DocumentationLinkGateTests(unittest.TestCase):
    def test_accepts_contained_assets_pages_and_anchors(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "guide").mkdir()
            (root / "asset.svg").write_text("<svg/>", encoding="utf-8")
            (root / "guide/index.html").write_text(
                '<h2 id="ready">Ready</h2>', encoding="utf-8"
            )
            (root / "index.html").write_text(
                '<a href="/aros-tools/guide/#ready">Guide</a>'
                '<img src="/aros-tools/asset.svg">',
                encoding="utf-8",
            )

            checked, failures = CHECKER.check(root, "/aros-tools/")

            self.assertEqual(checked, 2)
            self.assertEqual(failures, [])

    def test_reports_missing_targets_and_anchors(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "index.html").write_text(
                '<a href="/aros-tools/missing/">Missing</a>'
                '<a href="#absent">Anchor</a>',
                encoding="utf-8",
            )

            _, failures = CHECKER.check(root, "/aros-tools/")

            self.assertEqual(len(failures), 2)
            self.assertTrue(any("target is missing" in failure for failure in failures))
            self.assertTrue(any("anchor is missing" in failure for failure in failures))

    def test_rejects_root_relative_links_outside_the_site_base(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            (root / "index.html").write_text(
                '<a href="/other-site/">Escape</a>', encoding="utf-8"
            )

            _, failures = CHECKER.check(root, "/aros-tools/")

            self.assertEqual(len(failures), 1)
            self.assertIn("escapes configured base", failures[0])


if __name__ == "__main__":
    unittest.main()
