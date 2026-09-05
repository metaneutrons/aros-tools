#!/usr/bin/env python3
"""Maintain M0 contract/golden inputs; this is not a native producer validator.

Runs in the existing offline workspace quality gate. Actual producer/parser
conformance and four-host byte parity are M1/M2/M4 implementation gates.
"""

from __future__ import annotations

import base64
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "scripts/fixtures/toolchain-producer"


def read_json(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode()


class ToolchainProducerContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.contract = tomllib.loads((ROOT / "contracts/toolchain-producer-v1.toml").read_text())
        cls.golden = read_json("package-v1.json")
        cls.tree_path = ROOT / cls.contract["artifacts"]["tree_fixture"]
        cls.tree = json.loads(cls.tree_path.read_text(encoding="utf-8"))

    def assert_fields(self, document: dict, record: str) -> None:
        self.assertEqual(set(document), set(self.contract["documents"][record + "_fields"]))

    def test_scope_does_not_advertise_a_runtime_implementation(self) -> None:
        self.assertEqual(self.contract["schema_version"], 1)
        self.assertEqual(self.contract["status"], "specified-not-implemented")
        self.assertEqual(self.contract["new_executables"], [])
        self.assertEqual(self.contract["dependencies"], ["aros-common", "aros-fetch"])
        self.assertEqual(self.contract["forbidden_dependencies"], ["aros-cli", "aros-release"])
        self.assertFalse(self.contract["documents"]["release_asset"])
        self.assertFalse(self.contract["lifecycle"]["automatic_publish"])
        self.assertFalse(self.contract["lifecycle"]["shared_compiled_cache"])
        self.assertTrue(self.contract["lifecycle"]["fresh_release_builds"])

    def test_existing_format_versions_and_explicit_backend_are_preserved(self) -> None:
        inputs = self.contract["inputs"]
        self.assertEqual(inputs["recipe_schema"], "aros-toolchain-recipe-v2")
        self.assertEqual(inputs["source_lock_schema"], "aros-toolchain-source-lock-v2")
        self.assertEqual(inputs["source_target"], "crosstools-release")
        self.assertTrue(inputs["legacy_preview_requires_explicit_selection"])
        self.assertEqual(self.contract["commands"]["preserved"], ["install", "list", "verify", "path"])
        self.assertEqual(self.contract["commands"]["default_backend"], "native")
        self.assertFalse(self.contract["artifacts"]["index_is_consumer_lock"])
        self.assertEqual(self.contract["artifacts"]["manifest_schema"], 1)
        self.assertEqual(self.contract["artifacts"]["consumer_lock_schema"], 1)

    def test_release_inventory_has_no_self_hash_cycle(self) -> None:
        artifacts = self.contract["artifacts"]
        count = sum(artifacts[key] for key in (
            "archive_count", "manifest_count", "sidecar_count", "sbom_count", "support_count"
        ))
        self.assertEqual(count, 56)
        self.assertEqual(count, artifacts["total_assets"])
        self.assertEqual(artifacts["checksummed_assets"], count - 1)
        self.assertEqual(artifacts["attested_subjects"], count - 2)
        self.assertEqual(len(set(artifacts["support_files"])), artifacts["support_count"])
        self.assertEqual(artifacts["outer_types"], ["regular-file"])

    def test_shared_tree_vector_and_canonical_encoding_match_capture(self) -> None:
        self.assertEqual(sha256(self.tree_path.read_bytes()), self.golden["tree_fixture_sha256"])
        self.assertEqual(self.tree["entries"], self.golden["manifest"]["files"])
        self.assertEqual(self.tree["tree_sha256"], self.golden["manifest"]["tree_sha256"])
        encoded = canonical(self.tree["entries"][2])
        self.assertEqual(encoded.decode(), self.golden["canonical_entry_utf8"])
        self.assertEqual(sha256(encoded), self.golden["canonical_entry_sha256"])
        self.assertTrue(encoded.endswith(b"\n"))
        self.assertIn("Größe".encode(), encoded)
        self.assertNotIn(b"\\u00", encoded)
        self.assertEqual(sha256(b"".join(canonical(entry) for entry in self.tree["entries"])), self.tree["tree_sha256"])

    def test_captured_archive_digest_headers_inventory_and_contents(self) -> None:
        payload = base64.b64decode(self.golden["archive_base64"], validate=True)
        self.assertEqual(len(payload), self.golden["archive_size"])
        self.assertEqual(sha256(payload), self.golden["archive_sha256"])
        self.assertTrue(payload.startswith(b"\xfd7zXZ\x00"))
        entries = {"toolchain/" + entry["path"]: entry for entry in self.tree["entries"]}
        entries["toolchain"] = {"type": "directory", "mode": "0755"}
        entries["toolchain/toolchain-manifest.json"] = {"type": "file", "mode": "0644"}
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:xz") as archive:
            members = archive.getmembers()
            self.assertEqual([member.name for member in members], sorted(entries))
            for member in members:
                with self.subTest(path=member.name):
                    entry = entries[member.name]
                    self.assertEqual((member.uid, member.gid, member.uname, member.gname), (0, 0, "", ""))
                    self.assertEqual(member.mtime, self.golden["manifest"]["source_date_epoch"])
                    self.assertEqual(member.mode, int(entry["mode"], 8))
                    if entry["type"] == "directory":
                        self.assertTrue(member.isdir())
                    elif entry["type"] == "symlink":
                        self.assertTrue(member.issym())
                        self.assertEqual(member.linkname, entry["target"])
                    else:
                        self.assertTrue(member.isfile())
                        stream = archive.extractfile(member)
                        self.assertIsNotNone(stream)
                        with stream:
                            data = stream.read()
                        if member.name.endswith("/toolchain-manifest.json"):
                            self.assertEqual(json.loads(data), self.golden["manifest"])
                            self.assertEqual(sha256(data), self.golden["manifest_sha256"])
                        else:
                            self.assertEqual(data, self.tree["file_content_utf8"].encode())
                            self.assertEqual(len(data), entry["size"])
                            self.assertEqual(sha256(data), entry["sha256"])

    def test_negative_cases_keep_legacy_observations_distinct_from_native_requirements(self) -> None:
        cases = read_json("manifest-cases.json")["cases"]
        self.assertEqual(len(cases), self.golden["legacy_manifest_cases"])
        self.assertEqual(len({case["name"] for case in cases}), len(cases))
        stricter = set()
        for case in cases:
            self.assertIn(case["legacy"], {"accept", "reject"})
            self.assertIn(case["native"], {"accept", "reject"})
            if case["legacy"] == "reject":
                self.assertEqual(case["native"], "reject", case["name"])
            if case["legacy"] != case["native"]:
                stricter.add(case["name"])
            if case["name"] != "valid":
                self.assertTrue(case["changes"])
                self.assertEqual(case["native"], "reject")
        self.assertEqual(stricter, {
            "unknown-top-level-field", "whitespace-capability", "padded-capability", "escaping-release-id"
        })

    def test_versioned_wire_examples_have_the_exact_declared_fields(self) -> None:
        for name in ("plan", "result", "receipt"):
            with self.subTest(document=name):
                example = read_json(name + "-v1.json")
                self.assert_fields(example, name)
                self.assertEqual(example["schema"], self.contract["documents"][name + "_schema"])
                self.assert_fields(example["identity"], "identity")
                self.assert_fields(example["identity"]["executor"], "executor")
                self.assertIsNone(example["identity"]["executor"]["origin_evidence_sha256"])
                self.assertEqual(example["identity"]["tools_commit"], example["identity"]["executor"]["tools_commit"])
        plan = read_json("plan-v1.json")
        self.assert_fields(plan["paths"], "paths")
        self.assert_fields(plan["resources"], "resources")
        self.assertEqual(plan["steps"], self.contract["lifecycle"]["phases"])
        self.assertEqual(plan["readiness"], "incomplete")
        self.assertTrue(plan["findings"])
        self.assertIsNone(plan["resources"]["jobs"])
        self.assertIsNone(plan["resources"]["timeout_seconds"])
        result = read_json("result-v1.json")
        for output in result["outputs"]:
            self.assert_fields(output, "output")
            self.assertEqual(output["sha256"], self.golden["archive_sha256"])
            self.assertEqual(output["size"], self.golden["archive_size"])
        for evidence in result["evidence"]:
            self.assert_fields(evidence, "evidence")
        self.assertEqual(result["qualification"], "local-only")
        self.assertEqual(result["commit_state"], "committed")

    def test_receipt_self_digest_binds_the_exact_complete_record(self) -> None:
        receipt = read_json("receipt-v1.json")
        measured = receipt.pop("receipt_sha256")
        self.assertEqual(sha256(canonical(receipt)), measured)
        self.assertIsNone(receipt["previous_receipt_sha256"])
        self.assertEqual(receipt["phase"], "preflight")
        receipt["identity"]["executor"]["binary_sha256"] = "0" * 64
        self.assertNotEqual(sha256(canonical(receipt)), measured)

    def test_every_baseline_python_function_has_an_owner(self) -> None:
        document = (ROOT / self.contract["specification"]).read_text(encoding="utf-8")
        for name in self.golden["producer_functions"]:
            self.assertRegex(document, rf"\b{re.escape(name)}\b")

    def test_diagnostic_reservation_is_unique_and_does_not_claim_registration(self) -> None:
        diagnostics = self.contract["diagnostics"]
        self.assertEqual(diagnostics["envelope"], "aros-tool-diagnostics-v1")
        self.assertEqual(diagnostics["registration"], "reserved-until-M1")
        self.assertEqual(diagnostics["failure_exit"], 1)
        self.assertEqual(diagnostics["json_failure_stderr_documents"], 1)
        self.assertEqual(len(set(diagnostics["codes"])), len(diagnostics["codes"]))
        source = (ROOT / "crates/aros-common/src/diagnostic.rs").read_text()
        for code in diagnostics["codes"]:
            self.assertRegex(code, r"^AX[0-9]{4}$")
            self.assertNotIn('"' + code + '"', source)


if __name__ == "__main__":
    unittest.main()
