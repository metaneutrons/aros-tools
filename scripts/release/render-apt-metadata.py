#!/usr/bin/env python3
"""Render deterministic APT indexes without apt-ftparchive/dpkg-scanpackages.

The renderer intentionally uses only the Python standard library.  It reads the
Debian control member directly, so release bytes are independent from the
moving package set on a GitHub-hosted runner.  The calling shell script owns
OpenPGP signing and verifies the result with gpgv.
"""

from __future__ import annotations

import argparse
import bz2
import datetime as dt
import gzip
import hashlib
import io
import lzma
import os
from pathlib import Path
import re
import shutil
import tarfile
from typing import BinaryIO, NoReturn


AR_MAGIC = b"!<arch>\n"
SAFE_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
CONTROL_MEMBER = re.compile(r"^(?:\./)?control$")
MAX_DEB_BYTES = 512 * 1024 * 1024
MAX_AR_MEMBERS = 64
MAX_CONTROL_ARCHIVE_BYTES = 8 * 1024 * 1024
MAX_CONTROL_TAR_BYTES = 16 * 1024 * 1024
MAX_TAR_MEMBERS = 1_024
MAX_TAR_REGULAR_BYTES = 16 * 1024 * 1024
MAX_CONTROL_BYTES = 256 * 1024
COPY_CHUNK_BYTES = 64 * 1024


def fail(message: str) -> NoReturn:
    raise SystemExit(f"AP7200 {message}")


def read_exact(source: BinaryIO, size: int, description: str) -> bytes:
    data = source.read(size)
    if len(data) != size:
        fail(f"Debian package has a truncated {description}")
    return data


def ar_control_member(package: Path) -> tuple[str, bytes]:
    try:
        package_size = package.stat().st_size
    except OSError as error:
        fail(f"cannot inspect {package.name}: {error}")
    if package_size <= len(AR_MAGIC) or package_size > MAX_DEB_BYTES:
        fail(
            f"{package.name} size is outside the safe range "
            f"(maximum {MAX_DEB_BYTES} bytes)"
        )

    candidates: list[tuple[str, bytes]] = []
    names: set[str] = set()
    with package.open("rb") as source:
        if read_exact(source, len(AR_MAGIC), "ar archive signature") != AR_MAGIC:
            fail("Debian package has no ar archive signature")
        member_count = 0
        while source.tell() < package_size:
            member_count += 1
            if member_count > MAX_AR_MEMBERS:
                fail(f"{package.name} has too many ar members")
            header = read_exact(source, 60, "ar member header")
            if header[58:60] != b"`\n":
                fail("Debian package has a malformed ar member header")
            try:
                raw_name = header[:16].decode("ascii", "strict").strip()
                member_size = int(header[48:58].decode("ascii", "strict").strip())
            except (UnicodeDecodeError, ValueError):
                fail("Debian package has a malformed ar member identity or size")
            if (
                member_size < 0
                or source.tell() + member_size + (member_size % 2) > package_size
            ):
                fail("Debian package has a truncated ar member")

            name = raw_name[:-1] if raw_name.endswith("/") else raw_name
            payload_size = member_size
            if name.startswith("#1/"):
                try:
                    name_length = int(name[3:])
                except ValueError:
                    fail("Debian package has a malformed extended ar member name")
                if name_length <= 0 or name_length > member_size:
                    fail("Debian package has an unsafe extended ar member name")
                try:
                    name = read_exact(source, name_length, "extended ar member name").decode(
                        "utf-8", "strict"
                    )
                except UnicodeDecodeError:
                    fail("Debian package has a non-UTF-8 extended ar member name")
                payload_size -= name_length
            if not name or name in names:
                fail(f"Debian package repeats or omits an ar member name: {name!r}")
            names.add(name)

            if name.startswith("control.tar"):
                if payload_size > MAX_CONTROL_ARCHIVE_BYTES:
                    fail(
                        f"{package.name} control archive exceeds "
                        f"{MAX_CONTROL_ARCHIVE_BYTES} bytes"
                    )
                payload = read_exact(source, payload_size, "control archive")
                candidates.append((name, payload))
            else:
                source.seek(payload_size, os.SEEK_CUR)

            if member_size % 2:
                if read_exact(source, 1, "ar member padding") != b"\n":
                    fail("Debian package has malformed ar member padding")

    if len(candidates) != 1:
        fail(f"{package.name} must contain exactly one control.tar member")
    return candidates[0]


def decompress_control(name: str, payload: bytes, package: Path) -> bytes:
    compressed = io.BytesIO(payload)
    try:
        if name.endswith(".gz"):
            source: BinaryIO = gzip.GzipFile(fileobj=compressed, mode="rb")
        elif name.endswith(".xz"):
            source = lzma.LZMAFile(compressed, mode="rb")
        elif name.endswith(".bz2"):
            source = bz2.BZ2File(compressed, mode="rb")
        elif name == "control.tar":
            source = compressed
        else:
            fail(f"unsupported Debian control compression in {name!r}")

        output = io.BytesIO()
        while True:
            block = source.read(COPY_CHUNK_BYTES)
            if not block:
                break
            if output.tell() + len(block) > MAX_CONTROL_TAR_BYTES:
                fail(
                    f"{package.name} expanded control archive exceeds "
                    f"{MAX_CONTROL_TAR_BYTES} bytes"
                )
            output.write(block)
        source.close()
        return output.getvalue()
    except (EOFError, OSError, lzma.LZMAError) as error:
        fail(f"cannot decompress control metadata from {package.name}: {error}")


def control_text(package: Path) -> str:
    name, compressed = ar_control_member(package)
    payload = decompress_control(name, compressed, package)
    control_matches = 0
    control_data: bytes | None = None
    regular_bytes = 0
    member_count = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r|") as archive:
            for member in archive:
                member_count += 1
                if member_count > MAX_TAR_MEMBERS:
                    fail(f"{package.name} control archive has too many members")
                if member.isfile():
                    regular_bytes += member.size
                    if regular_bytes > MAX_TAR_REGULAR_BYTES:
                        fail(
                            f"{package.name} control archive regular-file total exceeds "
                            f"{MAX_TAR_REGULAR_BYTES} bytes"
                        )
                if CONTROL_MEMBER.fullmatch(member.name):
                    control_matches += 1
                    if not member.isfile() or member.size > MAX_CONTROL_BYTES:
                        fail(
                            f"{package.name} control metadata is not a bounded regular file"
                        )
                    extracted = archive.extractfile(member)
                    if extracted is None:
                        fail(f"cannot read control metadata from {package.name}")
                    control_data = extracted.read(MAX_CONTROL_BYTES + 1)
                    if len(control_data) != member.size or len(control_data) > MAX_CONTROL_BYTES:
                        fail(f"{package.name} control metadata exceeds its declared safe size")
    except (tarfile.TarError, OSError) as error:
        fail(f"cannot parse control archive from {package.name}: {error}")
    if control_matches != 1 or control_data is None:
        fail(f"{package.name} must contain exactly one regular control file")
    try:
        text = control_data.decode("utf-8", "strict").replace("\r\n", "\n")
    except UnicodeDecodeError:
        fail(f"{package.name} has non-UTF-8 Debian control metadata")
    if "\x00" in text or not text.endswith("\n"):
        fail(f"{package.name} has unsafe Debian control metadata")
    return text.rstrip("\n")


def parse_fields(text: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    current: str | None = None
    for line in text.splitlines():
        if line.startswith((" ", "\t")):
            if current is None:
                fail("Debian control continuation has no field")
            fields[current] += "\n" + line
            continue
        if ": " not in line:
            fail("Debian control metadata has a malformed field")
        current, value = line.split(": ", 1)
        if not re.fullmatch(r"[A-Za-z0-9-]+", current) or current in fields:
            fail(f"Debian control field is unsafe or repeated: {current!r}")
        fields[current] = value
    return fields


def digest(path: Path, algorithm: str) -> str:
    value = hashlib.new(algorithm)
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def write_packages(package: Path, repository: Path, destination: Path, version: str, arch: str) -> None:
    control = control_text(package)
    fields = parse_fields(control)
    required = {
        "Package": "aros-tools",
        "Version": f"{version}-1",
        "Architecture": arch,
    }
    for name, expected in required.items():
        if fields.get(name) != expected:
            fail(f"{package.name} {name} is {fields.get(name)!r}; expected {expected!r}")
    relative = package.relative_to(repository).as_posix()
    generated = (
        f"{control}\n"
        f"Filename: {relative}\n"
        f"Size: {package.stat().st_size}\n"
        f"MD5sum: {digest(package, 'md5')}\n"  # APT compatibility; SHA-256 remains authoritative.
        f"SHA1: {digest(package, 'sha1')}\n"
        f"SHA256: {digest(package, 'sha256')}\n"
        f"SHA512: {digest(package, 'sha512')}\n"
    ).encode("utf-8")
    destination.write_bytes(generated)
    os.chmod(destination, 0o644)


def rfc2822(epoch: int) -> str:
    return dt.datetime.fromtimestamp(epoch, tz=dt.timezone.utc).strftime(
        "%a, %d %b %Y %H:%M:%S +0000"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--candidate-dir", type=Path, required=True)
    parser.add_argument("--repository-dir", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--metadata-epoch", type=int, required=True)
    parser.add_argument("--valid-for-seconds", type=int, default=7_776_000)
    args = parser.parse_args()
    if not SAFE_VERSION.fullmatch(args.version):
        fail("version must be a stable canonical SemVer")
    if args.metadata_epoch <= 0 or args.valid_for_seconds < 604_800:
        fail("metadata epoch/validity is outside the safe range")
    repository = args.repository_dir.resolve()
    candidate = args.candidate_dir.resolve()
    pool = repository / "pool/main/a/aros-tools"
    pool.mkdir(parents=True, exist_ok=False)
    for arch in ("amd64", "arm64"):
        source = candidate / f"aros-tools_{args.version}_{arch}.deb"
        if not source.is_file() or source.is_symlink():
            fail(f"qualified Debian package is missing: {source}")
        package = pool / source.name
        shutil.copyfile(source, package)
        os.chmod(package, 0o644)
        binary = repository / f"dists/stable/main/binary-{arch}"
        binary.mkdir(parents=True, exist_ok=False)
        packages = binary / "Packages"
        write_packages(package, repository, packages, args.version, arch)
        with packages.open("rb") as source_file, (binary / "Packages.gz").open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", compresslevel=9, mtime=0, fileobj=raw) as target:
                shutil.copyfileobj(source_file, target)
        by_hash = binary / "by-hash/SHA256"
        by_hash.mkdir(parents=True, exist_ok=False)
        for index in (packages, binary / "Packages.gz"):
            destination = by_hash / digest(index, "sha256")
            shutil.copyfile(index, destination)
            os.chmod(destination, 0o644)

    release = repository / "dists/stable/Release"
    indexes = sorted(
        path
        for path in (repository / "dists/stable").rglob("*")
        if path.is_file() and "/by-hash/" not in path.as_posix()
    )
    header = [
        "Origin: AROS tools",
        "Label: AROS tools",
        "Suite: stable",
        "Codename: stable",
        "Architectures: amd64 arm64",
        "Components: main",
        "Acquire-By-Hash: yes",
        "Description: Signed aros-tools packages",
        f"Date: {rfc2822(args.metadata_epoch)}",
        f"Valid-Until: {rfc2822(args.metadata_epoch + args.valid_for_seconds)}",
    ]
    sections: list[str] = []
    for algorithm, label in (("sha256", "SHA256"), ("sha512", "SHA512")):
        rows = [label + ":"]
        for index in indexes:
            relative = index.relative_to(release.parent).as_posix()
            rows.append(f" {digest(index, algorithm)} {index.stat().st_size:16d} {relative}")
        sections.extend(rows)
    release.write_text("\n".join((*header, *sections)) + "\n", encoding="utf-8")
    os.chmod(release, 0o644)


if __name__ == "__main__":
    main()
