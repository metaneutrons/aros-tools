"""Regression tests for the documentation publication boundary."""

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("check-docs-publication.py")
SPEC = importlib.util.spec_from_file_location("check_docs_publication", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


def valid_configuration() -> dict[str, object]:
    return {
        "$schema": "./node_modules/wrangler/config-schema.json",
        "name": "aros-tools-docs",
        "account_id": "9122b44fa4c05b23985d6a0b779caa01",
        "compatibility_date": "2026-09-04",
        "workers_dev": False,
        "preview_urls": False,
        "routes": CHECKER.EXPECTED_ROUTES,
        "assets": {
            "directory": "./worker-dist",
            "html_handling": "auto-trailing-slash",
            "not_found_handling": "404-page",
        },
    }


class DocumentationPublicationTests(unittest.TestCase):
    def test_accepts_exact_static_worker_boundary(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            configuration = root / "wrangler.jsonc"
            configuration.write_text(json.dumps(valid_configuration()), encoding="utf-8")
            assets = root / "worker-dist/aros-tools"
            assets.mkdir(parents=True)
            (assets / "index.html").write_text("index", encoding="utf-8")
            (assets / "404.html").write_text("missing", encoding="utf-8")

            CHECKER.validate_configuration(configuration)
            CHECKER.validate_assets(root / "worker-dist")

    def test_rejects_broader_route(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            configuration = pathlib.Path(temporary) / "wrangler.jsonc"
            payload = valid_configuration()
            payload["routes"] = [
                {"pattern": "aros.metaneutrons.cc/*", "zone_name": "metaneutrons.cc"}
            ]
            configuration.write_text(json.dumps(payload), encoding="utf-8")

            with self.assertRaisesRegex(SystemExit, "exactly own"):
                CHECKER.validate_configuration(configuration)

    def test_rejects_additional_binding(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            configuration = pathlib.Path(temporary) / "wrangler.jsonc"
            payload = valid_configuration()
            payload["r2_buckets"] = [{"binding": "BUCKET", "bucket_name": "unsafe"}]
            configuration.write_text(json.dumps(payload), encoding="utf-8")

            with self.assertRaisesRegex(SystemExit, "static-only contract"):
                CHECKER.validate_configuration(configuration)

    def test_rejects_symlink_in_asset_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            assets = root / "worker-dist/aros-tools"
            assets.mkdir(parents=True)
            (assets / "index.html").write_text("index", encoding="utf-8")
            (assets / "404.html").write_text("missing", encoding="utf-8")
            (assets / "escape").symlink_to(root)

            with self.assertRaisesRegex(SystemExit, "symbolic link"):
                CHECKER.validate_assets(root / "worker-dist")


if __name__ == "__main__":
    unittest.main()
