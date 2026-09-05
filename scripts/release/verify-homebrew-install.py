#!/usr/bin/env python3
"""Bind native Homebrew qualification to its real host and verified staging bytes.

This supplements the canonical aros-release manifest/archive verifier, which
already ran in the aggregate job. It is not a second archive format verifier.
"""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import stat
import struct
import subprocess
import sys

HOSTS = {
    "x86_64-apple-darwin": ("Darwin", "x86_64", "x86_64", "/usr/local"),
    "aarch64-apple-darwin": ("Darwin", "arm64", "arm64", "/opt/homebrew"),
    "x86_64-unknown-linux-gnu": ("Linux", "x86_64", "x86_64", "/home/linuxbrew/.linuxbrew"),
    "aarch64-unknown-linux-gnu": ("Linux", "aarch64", "arm64", "/home/linuxbrew/.linuxbrew"),
}
BINARY_NAMES = {
    "aros", "aros-ahi-runner", "aros-collect", "aros-fetch",
    "aros-genmodule", "aros-romtool", "aros-transpiler", "aros-verify",
}


class QualificationError(Exception):
    """A native package-manager qualification guarantee failed."""


def check_host(target, system, machine, brew_arch, brew_prefix, translated=False):
    measured = (system, machine, brew_arch, brew_prefix)
    if translated or measured != HOSTS[target]:
        raise QualificationError(
            f"AP7320 expected native {target}: {HOSTS[target]!r}; "
            f"measured {measured!r}, translated={translated}"
        )


def is_translated():
    if platform.system() != "Darwin":
        return False
    result = subprocess.run(
        ["/usr/sbin/sysctl", "-in", "sysctl.proc_translated"],
        capture_output=True, text=True, timeout=10,
    )
    # -i ignores this absent optional property on a genuine Intel host.
    if result.returncode != 0 or result.stdout.strip() not in {"", "0", "1"}:
        raise QualificationError("AP7320 cannot determine macOS translation state")
    return result.stdout.strip() == "1"


def check_header(header, target):
    if len(header) < 32:
        raise QualificationError("AP7321 executable header is truncated")
    if target.endswith("apple-darwin"):
        cpu = 0x0100000C if target.startswith("aarch64") else 0x01000007
        valid = (header[:4] == b"\xcf\xfa\xed\xfe"
                 and struct.unpack_from("<I", header, 4)[0] == cpu
                 and struct.unpack_from("<I", header, 12)[0] == 2)
    else:
        cpu = 183 if target.startswith("aarch64") else 62
        valid = (header[:7] == b"\x7fELF\x02\x01\x01"
                 and struct.unpack_from("<H", header, 18)[0] == cpu
                 and struct.unpack_from("<H", header, 16)[0] in {2, 3})
    if not valid:
        raise QualificationError(f"AP7321 installed executable is not native {target}")


def check_install(manifest_path, prefix, target, version):
    metadata = manifest_path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 65536:
        raise QualificationError("AP7321 staging manifest is missing, unsafe or oversized")
    manifest = json.loads(manifest_path.read_text())
    expected_archive = f"aros-tools-v{version}-{target}.tar.gz"
    if (manifest["schema"] != 1 or manifest["package"] != "aros-tools"
            or manifest["target"] != target or manifest["version"] != version
            or manifest["archive"] != expected_archive):
        raise QualificationError("AP7321 staging manifest does not identify the selected target/version")
    entries = [entry for entry in manifest["files"] if entry["path"].startswith("bin/")]
    expected_paths = {f"bin/{name}" for name in BINARY_NAMES}
    if len(entries) != len(expected_paths) or {entry["path"] for entry in entries} != expected_paths:
        raise QualificationError("AP7321 staging binary inventory differs from the native release contract")
    bin_root = prefix / "bin"
    if bin_root.is_symlink() or {entry.name for entry in bin_root.iterdir()} != BINARY_NAMES:
        raise QualificationError("AP7321 installed bin directory has an unexpected inventory or type")
    for entry in entries:
        binary = prefix / entry["path"]
        metadata = binary.lstat()
        if (not stat.S_ISREG(metadata.st_mode) or not metadata.st_mode & 0o111
                or metadata.st_size != entry["size"] or metadata.st_size > 64 * 1024 * 1024):
            raise QualificationError(f"AP7321 installed file type, size or executable mode differs: {binary.name}")
        with binary.open("rb") as stream:
            check_header(stream.read(32), target)
            stream.seek(0)
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if digest != entry["sha256"]:
            raise QualificationError(f"AP7321 installed bytes differ from staged {target}: {binary.name}")
    print(f"Homebrew installed bytes verified: {target}, {version}, {len(entries)} native executables")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    host = commands.add_parser("host")
    host.add_argument("--target", choices=HOSTS, required=True)
    host.add_argument("--brew-prefix", required=True)
    host.add_argument("--brew-arch", required=True)
    installed = commands.add_parser("installed")
    installed.add_argument("--target", choices=HOSTS, required=True)
    installed.add_argument("--version", required=True)
    installed.add_argument("--manifest", type=Path, required=True)
    installed.add_argument("--prefix", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "host":
            check_host(args.target, platform.system(), platform.machine(),
                       args.brew_arch, args.brew_prefix, is_translated())
            print(f"Homebrew host verified: {args.target}, prefix {args.brew_prefix}")
        else:
            check_install(args.manifest, args.prefix, args.target, args.version)
    except (QualificationError, OSError, ValueError, KeyError, TypeError,
            subprocess.TimeoutExpired) as error:
        print(f"::error::Homebrew qualification: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
