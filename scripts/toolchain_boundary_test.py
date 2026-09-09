#!/usr/bin/env python3
"""Maintain the producer library boundary, including renamed/target dependencies."""

from __future__ import annotations

from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
# This is a closed architectural allow-list, not an unrestricted crates.io
# exception. M4 additionally permits the statically linked XZ codec and the
# existing SHA-2 primitive required for bounded archive/content verification;
# producer lifecycle/publication owners remain forbidden below.
ALLOWED = {
    "aros-cmake-engine",
    "aros-common",
    "aros-fetch",
    "flate2",
    "fs2",
    "rustix",
    "semver",
    "serde",
    "serde_json",
    "sha2",
    "tar",
    "tempfile",
    "thiserror",
    "tokio",
    "toml",
    "tracing",
    "url",
    "which",
    "xz2",
}


def dependency_names(manifest: dict, workspace: dict) -> set[str]:
    """Resolve all dependency classes and aliases, including platform tables."""
    names = set()
    tables = [manifest, *manifest.get("target", {}).values()]
    for table in tables:
        for section in ("dependencies", "build-dependencies", "dev-dependencies"):
            for alias, declaration in table.get(section, {}).items():
                if isinstance(declaration, dict) and declaration.get("workspace") is True:
                    declaration = workspace[alias]
                name = declaration.get("package", alias) if isinstance(declaration, dict) else alias
                names.add(name)
    return names


class ToolchainBoundaryTests(unittest.TestCase):
    def test_live_library_has_only_the_reviewed_lower_level_dependencies(self) -> None:
        workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
        directory = ROOT / "crates/aros-toolchain"
        manifest = tomllib.loads((directory / "Cargo.toml").read_text())
        names = dependency_names(manifest, workspace["dependencies"])
        self.assertEqual(names - ALLOWED, set())
        self.assertIn("aros-common", names)
        self.assertFalse(manifest["package"]["publish"])
        self.assertNotIn("bin", manifest)
        self.assertNotIn("build", manifest["package"])
        self.assertFalse((directory / "src/main.rs").exists())
        self.assertFalse((directory / "src/bin").exists())
        self.assertFalse((directory / "build.rs").exists())
        self.assertIn("crates/aros-toolchain", workspace["members"])

    def test_alias_target_and_build_dependencies_cannot_hide_a_forbidden_owner(self) -> None:
        for section in ("dependencies", "build-dependencies", "dev-dependencies"):
            for forbidden in ("aros-cli", "aros-release", "aros-board", "reqwest"):
                declaration = {section: {"alias": {"package": forbidden, "path": "../hidden"}}}
                for manifest in (declaration, {"target": {"cfg(unix)": declaration}}):
                    with self.subTest(section=section, forbidden=forbidden, manifest=manifest):
                        self.assertEqual(dependency_names(manifest, {}) - ALLOWED, {forbidden})

    def test_workspace_aliases_are_resolved_before_validation(self) -> None:
        manifest = {"dependencies": {"shared": {"workspace": True}}}
        self.assertEqual(dependency_names(manifest, {"shared": {"package": "aros-common"}}) - ALLOWED, set())
        self.assertEqual(dependency_names(manifest, {"shared": {"package": "aros-release"}}) - ALLOWED, {"aros-release"})
        with self.assertRaises(KeyError):
            dependency_names(manifest, {})


if __name__ == "__main__":
    unittest.main()
