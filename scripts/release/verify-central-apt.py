#!/usr/bin/env python3
"""Read-only consumer verification of the centrally signed APT archive.

This repository never renders or signs archive metadata. Its trust decision is
the checked-in primary/subkey pair, the signed Release, both by-hash algorithms,
and exact candidate package bytes. Retained older versions are permitted, but
a newer channel, a mixed architecture set or a changed same-version payload is
not. Network and decompression inputs are bounded before parsing.
"""

from __future__ import annotations

import argparse
import datetime as dt
import email.utils
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts/release"
CONTRACT = ROOT / "contracts/apt-archive-v1.toml"
MIB = 1024 * 1024
VERSION = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)")
HEX40 = re.compile(r"[0-9A-F]{40}")


class VerificationError(Exception):
    """An actionable, fail-closed publication error."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerificationError(message)


def regular(path: Path) -> None:
    require(path.is_file() and not path.is_symlink(), f"not a regular file: {path}")


def command(arguments: list[str], *, accepted: tuple[int, ...] = (0,)) -> subprocess.CompletedProcess:
    result = subprocess.run(arguments, capture_output=True, timeout=180, check=False)
    require(result.returncode in accepted,
            f"{Path(arguments[0]).name} failed ({result.returncode}): "
            + result.stderr.decode("utf-8", errors="replace")[-4000:])
    return result


def load_contract(path: Path = CONTRACT) -> dict:
    regular(path)
    with path.open("rb") as stream:
        contract = tomllib.load(stream)
    fields = {
        "schema_version", "repository", "workflow", "domain", "project",
        "base_url", "prefix", "origin", "suite", "component", "architectures",
        "keyring", "primary_fingerprint", "signing_subkey", "valid_until_days",
        "keep_versions",
    }
    require(set(contract) == fields and type(contract["schema_version"]) is int
            and contract["schema_version"] == 1,
            "central APT contract has unexpected fields or schema")
    require(contract["repository"] == "metaneutrons/apt-archive"
            and contract["workflow"] == "publish.yml"
            and contract["domain"] == "metaneutrons.cc"
            and contract["project"] == contract["prefix"] == "aros-tools"
            and contract["base_url"] == "https://deb.metaneutrons.cc"
            and contract["origin"] == "metaneutrons"
            and contract["suite"] == "rolling" and contract["component"] == "main"
            and contract["architectures"] == ["amd64", "arm64"]
            and contract["keyring"] == "metaneutrons-archive-keyring.pgp"
            and type(contract["valid_until_days"]) is int
            and contract["valid_until_days"] == 180
            and type(contract["keep_versions"]) is int and contract["keep_versions"] == 5,
            "central APT contract does not describe the canonical consumer")
    for field in ("primary_fingerprint", "signing_subkey"):
        require(isinstance(contract[field], str) and HEX40.fullmatch(contract[field]) is not None,
                f"invalid central APT {field}")
    require(contract["primary_fingerprint"] != contract["signing_subkey"],
            "APT requires a separate domain signing subkey")
    return contract


def parse_deb822(data: bytes) -> list[dict[str, str]]:
    require(len(data) <= 16 * MIB and b"\x00" not in data and b"\r" not in data,
            "APT metadata is oversized or noncanonical")
    paragraphs = []
    current: dict[str, str] = {}
    previous = ""
    for line in data.decode("utf-8", errors="strict").splitlines() + [""]:
        if not line:
            if current:
                paragraphs.append(current)
            current, previous = {}, ""
        elif line[0].isspace():
            require(bool(previous), "orphan continuation in APT metadata")
            current[previous] += "\n" + line
        else:
            match = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9-]*):[ \t]*(.*)", line)
            require(match is not None, "malformed APT metadata field")
            assert match is not None
            previous = match[1].lower()
            require(previous not in current, f"duplicate APT field: {previous}")
            current[previous] = match[2]
    return paragraphs


def version_tuple(value: str) -> tuple[int, int, int]:
    match = VERSION.fullmatch(value)
    require(match is not None, f"noncanonical stable version: {value}")
    assert match is not None
    return tuple(int(part) for part in match.groups())


def safe_relative(value: str) -> str:
    parts = PurePosixPath(value).parts
    require(bool(parts) and not value.startswith("/") and str(PurePosixPath(value)) == value
            and all(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", part) and part not in (".", "..")
                    for part in parts), f"unsafe APT path: {value!r}")
    return value


def digest(path: Path, algorithm: str) -> str:
    regular(path)
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, algorithm).hexdigest()


class Archive:
    def __init__(self, contract: dict, root: Path, fixture: Path | None):
        self.contract, self.root, self.fixture = contract, root, fixture

    def fetch(self, relative: str, category: str, *, optional: bool = False,
              expected_size: int | None = None, domain_root: bool = False) -> Path | None:
        safe_relative(relative)
        destination = self.root / relative
        source_path = relative if domain_root else f"{self.contract['prefix']}/{relative}"
        arguments = [str(SCRIPTS / "download-bounded-https.sh"), "--output", str(destination),
                     "--class", category]
        if self.fixture is None:
            arguments += ["--url", f"{self.contract['base_url']}/{source_path}"]
        else:
            arguments += ["--source-file", str(self.fixture / source_path)]
        if optional:
            arguments += ["--allow-not-found"]
        if expected_size is not None:
            arguments += ["--expected-bytes", str(expected_size)]
        result = command(arguments, accepted=(0, 44) if optional else (0,))
        return None if result.returncode == 44 else destination

    def verify_key(self, key: Path) -> Path:
        home = self.root / "gnupg"
        home.mkdir(mode=0o700)
        result = command(["gpg", "--no-options", "--batch", "--no-autostart", "--homedir", str(home),
                          "--with-colons", "--show-keys", "--fingerprint", str(key)])
        primary, subkeys = [], []
        destination = None
        for line in result.stdout.decode("utf-8").splitlines():
            fields = line.split(":")
            if fields[0] in ("sec", "ssb"):
                raise VerificationError("public archive contains secret key material")
            if fields[0] in ("pub", "sub"):
                require(fields[1] not in ("r", "e", "d", "i"), "archive key is inactive")
                if fields[6]:
                    require(int(fields[6]) > time.time(), "archive key is expired")
                destination = primary if fields[0] == "pub" else subkeys
                capabilities = fields[11]
                if fields[0] == "pub":
                    require("c" in capabilities and "s" not in capabilities,
                            "archive primary must be certify-only")
                else:
                    require("s" in capabilities, "domain subkey cannot sign")
            elif fields[0] == "fpr" and destination is not None:
                destination.append(fields[9])
                destination = None
        require(primary == [self.contract["primary_fingerprint"]]
                and subkeys == [self.contract["signing_subkey"]],
                "archive keyring does not contain exactly the trusted primary and domain subkey")
        return home

    def verify_signature(self, signed: Path, home: Path, key: Path,
                         detached: Path | None = None) -> int:
        arguments = ["gpgv", "--homedir", str(home), "--keyring", str(key), "--status-fd", "1",
                     str(signed)]
        if detached is not None:
            arguments.append(str(detached))
        result = command(arguments)
        status = self.root / f"{signed.name}.status"
        status.write_bytes(result.stdout)
        command([str(SCRIPTS / "verify-gpgv-status.sh"), "--status-file", str(status),
                 "--fingerprint", self.contract["primary_fingerprint"],
                 "--signing-subkey", self.contract["signing_subkey"]])
        valid = [line.split() for line in result.stdout.decode("utf-8").splitlines()
                 if line.startswith("[GNUPG:] VALIDSIG ")]
        return int(valid[0][4])

    def verify(self, version: str, candidate: Path, mode: str) -> dict:
        policy = self.contract
        wanted = version_tuple(version)
        prefix = f"dists/{policy['suite']}"
        inrelease = self.fetch(f"{prefix}/InRelease", "apt-release", optional=mode == "preflight")
        if inrelease is None:
            return {"state": "absent", "version": None, "packages": {}}
        key = self.fetch(policy["keyring"], "apt-key", domain_root=True)
        assert key is not None
        home = self.verify_key(key)
        signature_epoch = self.verify_signature(inrelease, home, key)
        clear = self.root / "signed-Release"
        command(["gpg", "--no-options", "--batch", "--no-autostart", "--homedir", str(home),
                 "--no-default-keyring", "--keyring", str(key), "--output", str(clear),
                 "--decrypt", str(inrelease)])
        release = self.fetch(f"{prefix}/Release", "apt-release")
        detached = self.fetch(f"{prefix}/Release.gpg", "apt-release")
        assert release is not None and detached is not None
        require(clear.read_bytes() == release.read_bytes(), "Release differs from signed InRelease")
        require(self.verify_signature(detached, home, key, release) == signature_epoch,
                "Release signatures disagree on publication time")
        paragraphs = parse_deb822(clear.read_bytes())
        require(len(paragraphs) == 1, "signed Release must contain one stanza")
        fields = paragraphs[0]
        for field, expected in {
            "origin": policy["origin"], "label": policy["project"], "suite": policy["suite"],
            "codename": policy["suite"], "components": policy["component"],
            "architectures": " ".join(policy["architectures"]), "acquire-by-hash": "yes",
        }.items():
            require(fields.get(field) == expected, f"signed Release has unexpected {field}")
        epochs = []
        for field in ("date", "valid-until"):
            value = email.utils.parsedate_to_datetime(fields[field])
            require(value.tzinfo is not None and value.utcoffset() == dt.timedelta(0),
                    f"signed Release {field} is not UTC")
            epochs.append(int(value.timestamp()))
        published, expires = epochs
        now = int(time.time())
        require(published == signature_epoch and 0 < published <= now + 300,
                "signed Release publication time is inconsistent or in the future")
        require(expires - published == policy["valid_until_days"] * 86400,
                "signed Release validity period differs from the archive contract")
        require(mode == "preflight" or now < expires, "central APT metadata expired; refresh apt-archive")
        identities: dict[str, dict[str, tuple[str, int]]] = {}
        expected_paths = {f"main/binary-{arch}/{name}" for arch in policy["architectures"]
                          for name in ("Packages", "Packages.gz")}
        for label, algorithm, length in (("sha256", "sha256", 64), ("sha512", "sha512", 128)):
            records = {}
            for line in fields.get(label, "").splitlines():
                if not line.strip():
                    continue
                pieces = line.split()
                require(len(pieces) == 3, f"malformed signed {label} record")
                checksum, size, path = pieces
                require(re.fullmatch(rf"[0-9a-f]{{{length}}}", checksum) is not None
                        and re.fullmatch(r"[1-9][0-9]*", size) is not None
                        and int(size) <= 16 * MIB and path not in records,
                        f"invalid or duplicate signed {label} identity")
                records[path] = (checksum, int(size))
            require(set(records) == expected_paths, f"signed {label} inventory is not the 4-index matrix")
            identities[algorithm] = records
        measured = {}
        versions = {}
        for arch in policy["architectures"]:
            indexes = {}
            for name in ("Packages", "Packages.gz"):
                path = f"main/binary-{arch}/{name}"
                sha256, size = identities["sha256"][path]
                index = self.fetch(f"{prefix}/{path}", "apt-index", expected_size=size)
                assert index is not None
                for algorithm in ("sha256", "sha512"):
                    checksum, other_size = identities[algorithm][path]
                    require(other_size == size and digest(index, algorithm) == checksum,
                            f"{path} differs from signed {algorithm}")
                    by_hash = self.fetch(f"{prefix}/main/binary-{arch}/by-hash/{algorithm.upper()}/{checksum}",
                                         "apt-index", expected_size=size)
                    assert by_hash is not None
                    require(digest(by_hash, algorithm) == checksum, f"{path} by-hash differs")
                indexes[name] = index
            with gzip.GzipFile(fileobj=io.BytesIO(indexes["Packages.gz"].read_bytes())) as stream:
                expanded = stream.read(16 * MIB + 1)
            require(len(expanded) <= 16 * MIB and expanded == indexes["Packages"].read_bytes(),
                    f"{arch} compressed index differs or exceeds its expansion bound")
            packages = parse_deb822(expanded)
            releases = {}
            all_identities = set()
            for package in packages:
                identity = (package.get("package"), package.get("version"), package.get("architecture"))
                require(identity not in all_identities, f"duplicate package stanza for {arch}")
                all_identities.add(identity)
                require(identity[0] in ("aros-tools", "metaneutrons-archive-keyring")
                        and identity[2] in (arch, "all"), f"unexpected package or architecture in {arch}")
                if identity[0] != "aros-tools":
                    continue
                require(identity[2] == arch and isinstance(identity[1], str) and identity[1].endswith("-1"),
                        "aros-tools Debian identity is malformed")
                current = identity[1][:-2]
                parsed = version_tuple(current)
                require(parsed not in releases, "duplicate aros-tools version")
                releases[parsed] = package
            require(0 < len(releases) <= policy["keep_versions"], f"{arch} has no bounded release history")
            newest = max(releases)
            require(newest <= wanted, f"APT already exposes newer version {'.'.join(map(str, newest))}")
            require(mode == "preflight" or newest == wanted,
                    f"APT has not converged to {version} on {arch}")
            versions[arch] = newest
            if newest == wanted:
                package = releases[wanted]
                path = safe_relative(package["filename"])
                expected_path = f"pool/main/a/aros-tools/aros-tools_{version}-1_{arch}.deb"
                require(path == expected_path, "central APT package filename does not match its identity")
                size = package["size"]
                require(re.fullmatch(r"[1-9][0-9]*", size) is not None and int(size) <= 512 * MIB,
                        "APT package size exceeds its bound")
                payload = self.fetch(path, "apt-package", expected_size=int(size))
                assert payload is not None
                local = candidate / f"aros-tools_{version}_{arch}.deb"
                regular(local)
                for algorithm, length in (("sha256", 64), ("sha512", 128)):
                    expected = package[algorithm]
                    require(re.fullmatch(rf"[0-9a-f]{{{length}}}", expected) is not None
                            and digest(payload, algorithm) == expected
                            and digest(local, algorithm) == expected,
                            f"same-version APT {arch} bytes differ from the candidate or signed index")
                require(local.stat().st_size == int(size), "candidate size differs from signed APT index")
                measured[arch] = path
        require(len(set(versions.values())) == 1, "APT architectures disagree on latest version")
        newest = next(iter(versions.values()))
        return {"state": "same" if newest == wanted else "older",
                "version": ".".join(map(str, newest)), "expired": now >= expires,
                "packages": measured}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("preflight", "exact"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--candidate-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--fixture-root", type=Path)
    parser.add_argument("--contract", type=Path, default=CONTRACT)
    args = parser.parse_args()
    require((args.fixture_root is None and args.contract == CONTRACT)
            or os.environ.get("AROS_RELEASE_POLICY_FIXTURE") == "1",
            "fixture and contract overrides are permitted only in policy tests")
    contract = load_contract(args.contract)
    version_tuple(args.version)
    require(args.candidate_dir.is_dir() and not args.candidate_dir.is_symlink(), "unsafe candidate directory")
    if args.output_dir is not None:
        require(not args.output_dir.exists() and not args.output_dir.is_symlink(), "output directory must be new")
        command([str(SCRIPTS / "prepare-output-parent.sh"), "--path", str(args.output_dir), "--mode", "0755"])
    parent = args.output_dir.parent if args.output_dir is not None else None
    with tempfile.TemporaryDirectory(prefix="aros-central-apt-", dir=parent) as temporary:
        stage = Path(temporary) / "public"
        stage.mkdir()
        result = Archive(contract, stage, args.fixture_root).verify(args.version, args.candidate_dir, args.mode)
        (stage / "verification.json").write_text(json.dumps(result, sort_keys=True) + "\n", encoding="utf-8")
        if args.output_dir is not None:
            require(not args.output_dir.exists() and not args.output_dir.is_symlink(), "output appeared during verification")
            stage.rename(args.output_dir)
        print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (VerificationError, OSError, UnicodeError, KeyError, TypeError, ValueError, EOFError,
            subprocess.TimeoutExpired, tomllib.TOMLDecodeError) as error:
        print(f"::error::AP7250 central APT verification failed: {error}", file=sys.stderr)
        sys.exit(1)
