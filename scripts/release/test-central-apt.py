#!/usr/bin/env python3
"""Adversarial consumer tests with synthetic packages and a disposable domain key."""

from __future__ import annotations

import argparse
import copy
import datetime as dt
import email.utils
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("central_apt", Path(__file__).with_name("verify-central-apt.py"))
assert spec is not None and spec.loader is not None
apt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apt)
contract_spec = importlib.util.spec_from_file_location("central_contract", Path(__file__).with_name("central-apt-contract.py"))
assert contract_spec is not None and contract_spec.loader is not None
contract_module = importlib.util.module_from_spec(contract_spec)
contract_spec.loader.exec_module(contract_module)


def run(*args: str) -> bytes:
    result = subprocess.run(args, capture_output=True, check=False, timeout=60)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors="replace"))
    return result.stdout


def create_fixture(root: Path) -> dict:
    root.mkdir()
    home = root / "gnupg"
    home.mkdir(mode=0o700)
    base = ["gpg", "--no-options", "--batch", "--homedir", str(home),
            "--pinentry-mode", "loopback", "--passphrase", ""]
    epoch = int(time.time()) - 3600
    run(*base, "--faked-system-time", f"{epoch}!", "--quick-generate-key",
        "AROS disposable archive fixture <apt@example.invalid>", "ed25519", "cert", "730d")
    primary = [line.split(":")[9] for line in run(*base, "--with-colons", "--list-keys")
               .decode().splitlines() if line.startswith("fpr:")][0]
    run(*base, "--faked-system-time", f"{epoch}!", "--quick-add-key", primary, "ed25519", "sign", "730d")
    fingerprints = [line.split(":")[9] for line in run(*base, "--with-colons", "--list-keys")
                    .decode().splitlines() if line.startswith("fpr:")]
    contract = apt.load_contract()
    original = apt.CONTRACT.read_text()
    original = original.replace(contract["primary_fingerprint"], fingerprints[0])
    original = original.replace(contract["signing_subkey"], fingerprints[1])
    (root / "contract.toml").write_text(original)
    contract = apt.load_contract(root / "contract.toml")
    archive = root / "archive"
    archive.mkdir()
    (archive / contract["keyring"]).write_bytes(run(*base, "--export", primary))
    (root / "candidate").mkdir()
    render_fixture(root, contract)
    return contract


def sign(root: Path, contract: dict, fields: str | None = None, epoch: int | None = None) -> None:
    release = root / "archive/aros-tools/dists/rolling/Release"
    if fields is not None:
        release.write_text(fields)
    timestamp = epoch or int(email.utils.parsedate_to_datetime(
        apt.parse_deb822(release.read_bytes())[0]["date"]).timestamp())
    args = ["gpg", "--no-options", "--batch", "--yes", "--homedir", str(root / "gnupg"),
            "--pinentry-mode", "loopback", "--passphrase", "", "--faked-system-time", f"{timestamp}!",
            "--local-user", contract["signing_subkey"] + "!"]
    run(*args, "--armor", "--output", str(release.with_name("InRelease")), "--clearsign", str(release))
    run(*args, "--output", str(release.with_name("Release.gpg")), "--detach-sign", str(release))


def render_fixture(root: Path, contract: dict, versions: tuple[str, ...] = ("1.2.2", "1.2.3"),
                   *, fields_override: dict | None = None, epoch: int | None = None) -> None:
    archive = root / "archive/aros-tools"
    dists = archive / "dists/rolling"
    for arch in ("amd64", "arm64"):
        index_dir = dists / f"main/binary-{arch}"
        index_dir.mkdir(parents=True, exist_ok=True)
        entries = []
        for version in versions:
            payload = f"synthetic Debian fixture {version} {arch}\n".encode()
            filename = f"pool/main/a/aros-tools/aros-tools_{version}-1_{arch}.deb"
            path = archive / filename
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
            (root / "candidate" / f"aros-tools_{version}_{arch}.deb").write_bytes(payload)
            entries.append(f"Package: aros-tools\nVersion: {version}-1\nArchitecture: {arch}\n"
                           f"Filename: {filename}\nSize: {len(payload)}\n"
                           f"SHA256: {hashlib.sha256(payload).hexdigest()}\n"
                           f"SHA512: {hashlib.sha512(payload).hexdigest()}\n")
        (index_dir / "Packages").write_text("\n".join(entries))
        (index_dir / "Packages.gz").write_bytes(gzip.compress((index_dir / "Packages").read_bytes(), mtime=0))
    published = epoch or int(time.time()) - 30
    fields = {
        "Origin": "metaneutrons", "Label": "aros-tools", "Suite": "rolling", "Codename": "rolling",
        "Architectures": "amd64 arm64", "Components": "main", "Acquire-By-Hash": "yes",
        "Date": email.utils.format_datetime(dt.datetime.fromtimestamp(published, dt.timezone.utc), usegmt=True),
        "Valid-Until": email.utils.format_datetime(
            dt.datetime.fromtimestamp(published + 180 * 86400, dt.timezone.utc), usegmt=True),
    }
    if fields_override:
        fields.update(fields_override)
    lines = [f"{key}: {value}" for key, value in fields.items()]
    for algorithm in ("sha256", "sha512"):
        lines.append(algorithm.upper() + ":")
        for arch in ("amd64", "arm64"):
            for name in ("Packages", "Packages.gz"):
                path = dists / f"main/binary-{arch}/{name}"
                checksum = hashlib.new(algorithm, path.read_bytes()).hexdigest()
                relative = str(path.relative_to(dists))
                lines.append(f" {checksum} {path.stat().st_size} {relative}")
                by_hash = path.parent / "by-hash" / algorithm.upper() / checksum
                by_hash.parent.mkdir(parents=True, exist_ok=True)
                by_hash.write_bytes(path.read_bytes())
    sign(root, contract, "\n".join(lines) + "\n", published)


class CentralAptTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # macOS UNIX-domain socket paths are shorter than its normal TMPDIR.
        cls.temporary = tempfile.TemporaryDirectory(prefix="aros-apt-", dir="/tmp")
        cls.base = Path(cls.temporary.name)
        cls.fixture = cls.base / "fixture"
        cls.contract = create_fixture(cls.fixture)
        cls.previous_fixture = os.environ.get("AROS_RELEASE_POLICY_FIXTURE")
        os.environ["AROS_RELEASE_POLICY_FIXTURE"] = "1"

    @classmethod
    def tearDownClass(cls):
        run("gpgconf", "--homedir", str(cls.fixture / "gnupg"), "--kill", "all")
        if cls.previous_fixture is None:
            os.environ.pop("AROS_RELEASE_POLICY_FIXTURE", None)
        else:
            os.environ["AROS_RELEASE_POLICY_FIXTURE"] = cls.previous_fixture
        cls.temporary.cleanup()

    def setUp(self):
        self.case = self.base / self.id().split(".")[-1]
        self.case.mkdir()
        shutil.copytree(self.fixture / "archive", self.case / "archive")
        shutil.copytree(self.fixture / "candidate", self.case / "candidate")
        (self.case / "gnupg").symlink_to(self.fixture / "gnupg", target_is_directory=True)
        self.contract = copy.deepcopy(type(self).contract)

    def change_signed_release(self, old: str, new: str):
        path = self.case / "archive/aros-tools/dists/rolling/Release"
        original = path.read_text()
        self.assertIn(old, original)
        # Malformed signed fields are the input under test, not a fixture error.
        epoch = int(email.utils.parsedate_to_datetime(
            apt.parse_deb822(original.encode())[0]["date"]).timestamp())
        sign(self.case, self.contract, original.replace(old, new), epoch)

    def refresh_index_signatures(self):
        directory = self.case / "archive/aros-tools/dists/rolling"
        path = directory / "Release"
        text = path.read_text().split("SHA256:\n")[0]
        for algorithm in ("sha256", "sha512"):
            text += algorithm.upper() + ":\n"
            for arch in ("amd64", "arm64"):
                for name in ("Packages", "Packages.gz"):
                    index = directory / f"main/binary-{arch}/{name}"
                    checksum = hashlib.new(algorithm, index.read_bytes()).hexdigest()
                    text += f" {checksum} {index.stat().st_size} {index.relative_to(directory)}\n"
                    by_hash = index.parent / "by-hash" / algorithm.upper() / checksum
                    by_hash.write_bytes(index.read_bytes())
        sign(self.case, self.contract, text)

    def change_packages(self, arch: str, transform):
        directory = self.case / f"archive/aros-tools/dists/rolling/main/binary-{arch}"
        payload = transform((directory / "Packages").read_text())
        (directory / "Packages").write_text(payload)
        (directory / "Packages.gz").write_bytes(gzip.compress(payload.encode(), mtime=0))
        self.refresh_index_signatures()

    def verify(self, mode="exact", version="1.2.3"):
        with tempfile.TemporaryDirectory(dir=self.case, prefix="verified-") as output:
            return apt.Archive(self.contract, Path(output), self.case / "archive").verify(
                version, self.case / "candidate", mode)

    def test_exact_retained_versions(self):
        result = self.verify()
        self.assertEqual(result["state"], "same")
        self.assertEqual(set(result["packages"]), {"amd64", "arm64"})

    def test_preflight_older_and_newer_are_distinct(self):
        self.assertEqual(self.verify("preflight", "1.2.4")["state"], "older")
        with self.assertRaisesRegex(apt.VerificationError, "newer version"):
            self.verify("preflight", "1.2.2")
        with self.assertRaisesRegex(apt.VerificationError, "not converged"):
            self.verify("exact", "1.2.4")

    def test_absent_is_only_allowed_before_publication(self):
        (self.case / "archive/aros-tools/dists/rolling/InRelease").unlink()
        self.assertEqual(self.verify("preflight")["state"], "absent")
        with self.assertRaises(apt.VerificationError):
            self.verify()

    def test_same_version_candidate_tampering(self):
        (self.case / "candidate/aros-tools_1.2.3_arm64.deb").write_bytes(b"different")
        with self.assertRaisesRegex(apt.VerificationError, "same-version APT arm64"):
            self.verify()

    def test_same_version_remote_payload_tampering(self):
        path = self.case / "archive/aros-tools/pool/main/a/aros-tools/aros-tools_1.2.3-1_amd64.deb"
        value = path.read_bytes()
        path.write_bytes(b"x" + value[1:])
        with self.assertRaisesRegex(apt.VerificationError, "same-version APT amd64"):
            self.verify()

    def test_wrong_primary(self):
        self.contract["primary_fingerprint"] = "A" * 40
        with self.assertRaisesRegex(apt.VerificationError, "trusted primary"):
            self.verify()

    def test_wrong_domain_subkey(self):
        self.contract["signing_subkey"] = "B" * 40
        with self.assertRaisesRegex(apt.VerificationError, "domain subkey"):
            self.verify()

    def test_tampered_signature_is_not_an_absent_channel(self):
        path = self.case / "archive/aros-tools/dists/rolling/InRelease"
        path.write_bytes(path.read_bytes().replace(b"Suite: rolling", b"Suite: changed"))
        with self.assertRaises(apt.VerificationError):
            self.verify("preflight")

    def test_detached_release_must_match_inrelease(self):
        path = self.case / "archive/aros-tools/dists/rolling/Release"
        path.write_bytes(path.read_bytes() + b"\n")
        with self.assertRaisesRegex(apt.VerificationError, "differs from signed"):
            self.verify()

    def test_by_hash_tampering(self):
        directory = self.case / "archive/aros-tools/dists/rolling/main/binary-amd64/by-hash/SHA512"
        for path in directory.iterdir():
            payload = path.read_bytes()
            path.write_bytes(b"x" + payload[1:])
        with self.assertRaisesRegex(apt.VerificationError, "by-hash differs"):
            self.verify()

    def test_index_alias_tampering(self):
        path = self.case / "archive/aros-tools/dists/rolling/main/binary-arm64/Packages"
        path.write_bytes(path.read_bytes() + b"\n")
        with self.assertRaises(apt.VerificationError):
            self.verify()

    def test_fixture_override_is_restricted(self):
        result = subprocess.run([sys.executable, str(Path(apt.__file__)), "--mode", "exact",
                                 "--version", "1.2.3", "--candidate-dir", str(self.case / "candidate"),
                                 "--fixture-root", str(self.case / "archive")], capture_output=True,
                                env={key: value for key, value in os.environ.items()
                                     if key != "AROS_RELEASE_POLICY_FIXTURE"}, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"permitted only in policy tests", result.stderr)

    def test_metadata_duplicate_field_is_case_insensitive(self):
        for payload in (b"Package: aros-tools\npackage: other\n", b" SHA256: x\n", b"Invalid\n"):
            with self.assertRaises(apt.VerificationError):
                apt.parse_deb822(payload)

    def test_path_safety(self):
        for value in ("../secret", "/tmp/secret", "pool//a.deb", "pool/a/../../secret", "pool/%2fsecret"):
            with self.assertRaises(apt.VerificationError):
                apt.safe_relative(value)

    def test_contract_rejects_origin_and_suite_drift(self):
        contract = apt.load_contract()
        contract["suite"] = "stable"
        path = self.case / "contract.toml"
        path.write_text(apt.CONTRACT.read_text().replace('suite = "rolling"', 'suite = "stable"'))
        with self.assertRaises(apt.VerificationError):
            apt.load_contract(path)

    def test_signed_wrong_suite_is_rejected(self):
        self.change_signed_release("Suite: rolling", "Suite: stable")
        with self.assertRaisesRegex(apt.VerificationError, "unexpected suite"):
            self.verify()

    def test_signed_duplicate_field_is_rejected(self):
        self.change_signed_release("Origin: metaneutrons", "Origin: metaneutrons\norigin: another")
        with self.assertRaisesRegex(apt.VerificationError, "duplicate APT field"):
            self.verify()

    def test_missing_signed_index_is_rejected(self):
        path = self.case / "archive/aros-tools/dists/rolling/Release"
        text = "\n".join(line for line in path.read_text().splitlines()
                         if not line.endswith("main/binary-arm64/Packages.gz")) + "\n"
        sign(self.case, self.contract, text)
        with self.assertRaisesRegex(apt.VerificationError, "4-index matrix"):
            self.verify()

    def test_signed_wrong_validity_period_is_rejected(self):
        path = self.case / "archive/aros-tools/dists/rolling/Release"
        fields = apt.parse_deb822(path.read_bytes())[0]
        self.change_signed_release("Valid-Until: " + fields["valid-until"], "Valid-Until: " + fields["date"])
        with self.assertRaisesRegex(apt.VerificationError, "validity period"):
            self.verify()

    def test_expiry_requires_central_refresh(self):
        future = time.time() + 181 * 86400
        with patch.object(apt.time, "time", return_value=future):
            with self.assertRaisesRegex(apt.VerificationError, "metadata expired"):
                self.verify()
            self.assertTrue(self.verify("preflight")["expired"])

    def test_future_publication_is_rejected(self):
        render_fixture(self.case, self.contract, epoch=int(time.time()) + 3600)
        with self.assertRaises(apt.VerificationError):
            self.verify()

    def test_signed_mixed_latest_architectures_are_rejected(self):
        self.change_packages("arm64", lambda text: text.split("\n\n")[0] + "\n")
        with self.assertRaisesRegex(apt.VerificationError, "architectures disagree"):
            self.verify("preflight")

    def test_signed_duplicate_package_is_rejected(self):
        self.change_packages("arm64", lambda text: text + "\n" + text)
        with self.assertRaisesRegex(apt.VerificationError, "duplicate package stanza"):
            self.verify()

    def test_signed_unsafe_filename_is_rejected(self):
        self.change_packages("amd64", lambda text: text.replace(
            "Filename: pool/main/a/aros-tools/aros-tools_1.2.3-1_amd64.deb", "Filename: ../secret"))
        with self.assertRaisesRegex(apt.VerificationError, "unsafe APT path"):
            self.verify()

    def test_signed_gzip_bomb_is_bounded(self):
        compressed = self.case / "archive/aros-tools/dists/rolling/main/binary-amd64/Packages.gz"
        compressed.write_bytes(gzip.compress(b"x" * (16 * apt.MIB + 1), mtime=0))
        self.refresh_index_signatures()
        with self.assertRaisesRegex(apt.VerificationError, "expansion bound"):
            self.verify()

    def test_failure_leaves_no_output(self):
        output = self.case / "failed-output"
        (self.case / "candidate/aros-tools_1.2.3_arm64.deb").write_bytes(b"broken")
        result = subprocess.run([sys.executable, str(Path(apt.__file__)), "--mode", "exact", "--version", "1.2.3",
                                 "--candidate-dir", str(self.case / "candidate"), "--fixture-root", str(self.case / "archive"),
                                 "--contract", str(self.fixture / "contract.toml"), "--output-dir", str(output)],
                                capture_output=True, timeout=120)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        self.assertIn(b"AP7250", result.stderr)

    def test_cli_output_is_committed_only_after_verification(self):
        output = self.case / "output"
        result = subprocess.run([sys.executable, str(Path(apt.__file__)), "--mode", "exact", "--version", "1.2.3",
                                 "--candidate-dir", str(self.case / "candidate"), "--fixture-root", str(self.case / "archive"),
                                 "--contract", str(self.fixture / "contract.toml"), "--output-dir", str(output)],
                                capture_output=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(json.loads(result.stdout)["state"], "same")
        self.assertTrue((output / "verification.json").is_file())


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--fixture-output":
        parser = argparse.ArgumentParser()
        parser.add_argument("--fixture-output", type=Path, required=True)
        arguments = parser.parse_args()
        with tempfile.TemporaryDirectory(prefix="aros-apt-", dir="/tmp") as temporary:
            fixture = Path(temporary) / "fixture"
            try:
                create_fixture(fixture)
                arguments.fixture_output.mkdir()
                for directory in ("archive", "candidate"):
                    shutil.copytree(fixture / directory, arguments.fixture_output / directory)
                shutil.copyfile(fixture / "contract.toml", arguments.fixture_output / "contract.toml")
            finally:
                if (fixture / "gnupg").exists():
                    run("gpgconf", "--homedir", str(fixture / "gnupg"), "--kill", "all")
    else:
        unittest.main()
