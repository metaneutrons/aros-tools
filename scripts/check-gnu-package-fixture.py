#!/usr/bin/env python3
"""Run the package-qualified GNU RV32 compiler-role engine fixture."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import resource
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from typing import Any


REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
ENGINE_ROOT = REPOSITORY_ROOT / "crates/aros-cmake-engine/engine"
FIXTURE = ENGINE_ROOT / "tests/package-qualified/LiteralGnuCompilerRoleTest.cmake"
CAPTURE_HELPER = ENGINE_ROOT / "tests/package-qualified/BoundedProcessOutput.cmake"
MANIFEST_NAME = "toolchain-manifest.json"
LAYOUT_NAME = "toolchain-tools.json"
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
MAX_LAYOUT_BYTES = 16 * 1024
MAX_PACKAGE_ENTRIES = 100_000
MAX_TOOL_BYTES = 256 * 1024 * 1024
MAX_PRESET_LENGTH = 128
MAX_ENV_PATH_LENGTH = 4096
MAX_CAPTURE_BYTES = 1024 * 1024
MAX_CHILD_FILE_BYTES = 8 * 1024 * 1024
CAPTURE_CHUNK_BYTES = 64 * 1024
TERMINATE_GRACE_SECONDS = 1.0


class GateError(Exception):
    """An input or qualification failure with a path-safe diagnostic."""


@dataclass(frozen=True)
class GateInputs:
    source_root: Path
    preset: str
    cli: Path
    package_root: Path
    cmake: Path


@dataclass(frozen=True)
class PackageDescription:
    manifest_bytes: bytes
    layout_bytes: bytes
    manifest: dict[str, Any]
    layout: dict[str, Any]
    roles: dict[str, Path]
    readelf: Path


@dataclass(frozen=True)
class BoundedCommandResult:
    returncode: int | None
    stdout: bytes
    stderr: bytes
    output_limit_bytes: int = MAX_CAPTURE_BYTES
    timed_out: bool = False
    output_limited: bool = False
    overflow_streams: tuple[str, ...] = ()


def _child_environment() -> dict[str, str]:
    """Copy the environment without credentials or dynamic-loader injection."""
    omitted_fragments = (
        "TOKEN",
        "GOVERNANCE",
        "OIDC",
        "SECRET",
        "PASSWORD",
        "CREDENTIAL",
        "API_KEY",
        "ACCESS_KEY",
        "PRIVATE_KEY",
    )
    omitted_prefixes = (
        "GITHUB_", "GH_", "GIT_", "SSH_", "GPG_", "GNUPG", "LD_", "DYLD_", "PYTHON",
    )
    omitted_names = {
        "LIBPATH",
        "SHLIB_PATH",
        "_RLD_LIST",
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONSTARTUP",
    }
    clean: dict[str, str] = {}
    for name, value in os.environ.items():
        normalized = name.upper()
        if (
            normalized.startswith(omitted_prefixes)
            or normalized in omitted_names
            or any(fragment in normalized for fragment in omitted_fragments)
        ):
            continue
        clean[name] = value
    return clean


def _signal_process_group(process: subprocess.Popen[bytes], sig: int) -> None:
    try:
        os.killpg(process.pid, sig)
    except ProcessLookupError:
        pass
    except OSError:
        try:
            process.send_signal(sig)
        except OSError:
            pass


def _stop_process_group(process: subprocess.Popen[bytes]) -> None:
    _signal_process_group(process, signal.SIGTERM)
    try:
        process.wait(timeout=TERMINATE_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass
    # The group leader may have exited while a child still holds a pipe open
    # or ignores SIGTERM, so stop the rest of the group as well.
    _signal_process_group(process, signal.SIGKILL)
    if process.poll() is None:
        try:
            process.wait(timeout=TERMINATE_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            # Keep the failure bounded even if a child cannot be reaped yet.
            pass


def _set_child_file_size_limit() -> None:
    """Cap files created by CMake and its package-tool descendants."""
    infinity = resource.RLIM_INFINITY
    current_soft, current_hard = resource.getrlimit(resource.RLIMIT_FSIZE)
    hard_limit = MAX_CHILD_FILE_BYTES if current_hard == infinity else min(current_hard, MAX_CHILD_FILE_BYTES)
    soft_limit = hard_limit if current_soft == infinity else min(current_soft, hard_limit)
    resource.setrlimit(resource.RLIMIT_FSIZE, (soft_limit, hard_limit))


def _run_bounded(
    command: list[str],
    *,
    cwd: Path,
    timeout: float,
    env: dict[str, str],
    output_limit: int = MAX_CAPTURE_BYTES,
    limit_child_file_size: bool = False,
) -> BoundedCommandResult:
    """Run a process group while retaining at most output_limit bytes per stream."""
    if output_limit < 1:
        raise ValueError("output_limit must be positive")
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        preexec_fn=_set_child_file_size_limit if limit_child_file_size else None,
    )
    assert process.stdout is not None and process.stderr is not None
    stdout_fd = process.stdout.fileno()
    stderr_fd = process.stderr.fileno()
    streams = {stdout_fd: bytearray(), stderr_fd: bytearray()}
    selector = selectors.DefaultSelector()
    for stream in (process.stdout, process.stderr):
        os.set_blocking(stream.fileno(), False)
        selector.register(stream, selectors.EVENT_READ)

    deadline = time.monotonic() + timeout
    timed_out = False
    output_limited = False
    overflow_streams: set[str] = set()
    group_stopped = False
    drain_deadline: float | None = None
    try:
        while selector.get_map():
            now = time.monotonic()
            if not group_stopped and now >= deadline:
                timed_out = True
                _stop_process_group(process)
                group_stopped = True
                drain_deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
            if drain_deadline is not None and now >= drain_deadline:
                break
            wait_until = drain_deadline if drain_deadline is not None else deadline
            wait_seconds = min(max(wait_until - now, 0.001), 0.25)
            for key, _event in selector.select(wait_seconds):
                descriptor = key.fd
                captured = streams[descriptor]
                read_size = min(CAPTURE_CHUNK_BYTES, output_limit - len(captured) + 1)
                try:
                    block = os.read(descriptor, read_size)
                except BlockingIOError:
                    continue
                if not block:
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    continue
                remaining_capacity = output_limit - len(captured)
                captured.extend(block[:remaining_capacity])
                if len(block) > remaining_capacity:
                    output_limited = True
                    label = "stdout" if descriptor == stdout_fd else "stderr"
                    overflow_streams.add(label)
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    if not group_stopped:
                        _stop_process_group(process)
                        group_stopped = True
                        drain_deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
    finally:
        if (timed_out or output_limited) and not group_stopped:
            _stop_process_group(process)
        for key in list(selector.get_map().values()):
            selector.unregister(key.fileobj)
            key.fileobj.close()
        selector.close()

    returncode = process.poll()
    if returncode is None:
        try:
            returncode = process.wait(timeout=TERMINATE_GRACE_SECONDS)
        except subprocess.TimeoutExpired:
            _stop_process_group(process)
            returncode = process.poll()
    return BoundedCommandResult(
        returncode=returncode,
        stdout=bytes(streams[stdout_fd]),
        stderr=bytes(streams[stderr_fd]),
        output_limit_bytes=output_limit,
        timed_out=timed_out,
        output_limited=output_limited,
        overflow_streams=tuple(sorted(overflow_streams)),
    )


def _bounded_text(data: bytes) -> str:
    """Decode captured bytes without allowing diagnostic text to grow unbounded."""
    decoded = data.decode("utf-8", errors="replace")
    encoded = decoded.encode("utf-8")
    if len(encoded) <= MAX_CAPTURE_BYTES:
        return decoded
    encoded = encoded[:MAX_CAPTURE_BYTES]
    return encoded.decode("utf-8", errors="ignore")


def _command_diagnostic(context: str, result: BoundedCommandResult, timeout: int) -> str:
    details = [context]
    has_status = False
    if result.timed_out:
        details.append(f"timed out after {timeout} seconds")
        has_status = True
    if result.output_limited:
        labels = ", ".join(result.overflow_streams) or "unknown stream"
        details.append(f"{labels} exceeded the {result.output_limit_bytes}-byte capture limit")
        has_status = True
    if not has_status:
        details.append(f"exit {result.returncode}")
    for label, captured in (("stdout", result.stdout), ("stderr", result.stderr)):
        diagnostic = _bounded_text(captured).strip()
        if diagnostic:
            details.append(f"{label}:\n{diagnostic}")
    return "; ".join(details)


def _required_env(name: str) -> str:
    value = os.environ.get(name)
    if value is None or value == "":
        raise GateError(f"{name} is required by gnu-package-test")
    if len(value) > MAX_ENV_PATH_LENGTH:
        raise GateError(f"{name} exceeds its input size limit")
    if "\x00" in value or "\n" in value or "\r" in value:
        raise GateError(f"{name} contains an invalid control character")
    return value


def _canonical_directory(name: str) -> Path:
    raw = _required_env(name)
    candidate = Path(raw)
    if not candidate.is_absolute():
        raise GateError(f"{name} must be an absolute canonical directory")
    try:
        metadata = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise GateError(f"{name} is unavailable") from error
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise GateError(f"{name} must name a real directory")
    if os.fspath(candidate) != os.fspath(resolved):
        raise GateError(f"{name} must use its canonical absolute path")
    return resolved


def _canonical_executable(name: str) -> Path:
    raw = _required_env(name)
    candidate = Path(raw)
    if not candidate.is_absolute():
        raise GateError(f"{name} must name an absolute executable path")
    try:
        metadata = candidate.lstat()
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise GateError(f"{name} is unavailable") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or os.fspath(candidate) != os.fspath(resolved)
        or not os.access(candidate, os.X_OK)
    ):
        raise GateError(f"{name} must be a canonical regular executable")
    return resolved


def _source_root_is_checkout(source_root: Path) -> None:
    try:
        result = _run_bounded(
            ["git", "-C", os.fspath(source_root), "rev-parse", "--show-toplevel"],
            cwd=source_root,
            env=_child_environment(),
            timeout=15,
            output_limit=64 * 1024,
        )
    except OSError as error:
        raise GateError("AROS_TEST_SOURCE_ROOT is not a readable Git checkout") from error
    if result.timed_out or result.output_limited or result.returncode != 0:
        raise GateError("AROS_TEST_SOURCE_ROOT is not a readable Git checkout")
    try:
        checkout_root = Path(result.stdout.decode("utf-8").strip()).resolve(strict=True)
    except OSError as error:
        raise GateError("AROS_TEST_SOURCE_ROOT has no canonical Git root") from error
    if checkout_root != source_root:
        raise GateError("AROS_TEST_SOURCE_ROOT must name the Git checkout root")
    targets = source_root / "aros-targets.toml"
    try:
        target_metadata = targets.lstat()
    except OSError as error:
        raise GateError("AROS_TEST_SOURCE_ROOT has no regular aros-targets.toml") from error
    if not stat.S_ISREG(target_metadata.st_mode) or stat.S_ISLNK(target_metadata.st_mode):
        raise GateError("AROS_TEST_SOURCE_ROOT has no regular aros-targets.toml")


def _parse_unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def _read_regular_file(path: Path, limit: int) -> bytes:
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, os.O_RDONLY | nofollow)
    except OSError as error:
        raise GateError("a required package file is missing or unsafe") from error
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
            raise GateError("a required package file is not a bounded regular file")
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            data = stream.read(limit + 1)
        if len(data) > limit:
            raise GateError("a required package file exceeds its size limit")
        return data
    finally:
        os.close(descriptor)


def _hash_regular_file(path: Path, limit: int) -> tuple[int, str]:
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, os.O_RDONLY | nofollow)
    except OSError as error:
        raise GateError("a required package executable is missing or unsafe") from error
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > limit:
            raise GateError("a required package executable is not a bounded regular file")
        digest = hashlib.sha256()
        size = 0
        with os.fdopen(descriptor, "rb", closefd=False) as stream:
            while block := stream.read(1024 * 1024):
                size += len(block)
                if size > limit:
                    raise GateError("a required package executable exceeds its size limit")
                digest.update(block)
        after = os.fstat(descriptor)
        if size != before.st_size or after.st_size != before.st_size:
            raise GateError("a required package executable changed while being measured")
        return size, digest.hexdigest()
    finally:
        os.close(descriptor)


def _parse_json(data: bytes, label: str) -> dict[str, Any]:
    try:
        document = json.loads(data, object_pairs_hook=_parse_unique_object)
    except (UnicodeDecodeError, ValueError, RecursionError) as error:
        raise GateError(f"{label} is not valid bounded JSON") from error
    if not isinstance(document, dict):
        raise GateError(f"{label} must be a JSON object")
    return document


def _safe_manifest_path(value: Any) -> str:
    if not isinstance(value, str) or not value or len(value) > 1024 or "\\" in value:
        raise GateError("the package inventory contains an invalid path")
    path = PurePosixPath(value)
    parts = value.split("/")
    if (
        path.is_absolute()
        or any(part in ("", ".", "..") or part.startswith("-") for part in parts)
        or any(re.fullmatch(r"[A-Za-z0-9._+-]+", part) is None for part in parts)
    ):
        raise GateError("the package inventory contains an unsafe path")
    return path.as_posix()


def _inventory(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    entries = manifest.get("files")
    if not isinstance(entries, list) or not entries or len(entries) > MAX_PACKAGE_ENTRIES:
        raise GateError("the GNU manifest has no bounded file inventory")
    inventory: dict[str, dict[str, Any]] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise GateError("the GNU manifest inventory contains an invalid entry")
        relative = _safe_manifest_path(entry.get("path"))
        if relative in inventory:
            raise GateError("the GNU manifest inventory contains a duplicate path")
        if entry.get("type") not in ("file", "directory", "symlink"):
            raise GateError("the GNU manifest inventory contains an invalid entry type")
        inventory[relative] = entry
    return inventory


def _validate_rv32_fixture_identity(manifest: dict[str, Any]) -> str:
    if type(manifest.get("schema")) is not int or manifest["schema"] != 2:
        raise GateError("the package must contain a schema-v2 toolchain manifest")
    compiler = manifest.get("compiler")
    if not isinstance(compiler, dict) or compiler.get("family") != "gnu":
        raise GateError("the package must declare a GNU compiler identity")
    target = compiler.get("target")
    if not isinstance(target, dict) or target.get("schema") != "aros-riscv-target-v1":
        raise GateError("the package must declare a GNU RISC-V target contract")
    if target.get("abi") != "ilp32f":
        raise GateError("the package ABI must be RV32 ilp32f")
    isa = target.get("isa")
    if not isinstance(isa, str) or not (isa == "rv32imafc" or isa.startswith("rv32imafc_")):
        raise GateError("the package ISA must include the fixture's RV32IMAF+C baseline")
    architecture = target.get("architecture")
    if not isinstance(architecture, str) or not architecture.startswith("rv32"):
        raise GateError("the package architecture must be RV32")
    triple = manifest.get("target_triple")
    if (
        not isinstance(triple, str)
        or len(triple) > 128
        or re.fullmatch(r"riscv(?:-[A-Za-z0-9._+]+)*-aros", triple) is None
    ):
        raise GateError("the package target triple must identify an AROS RV32 target")
    return triple


def _layout_roles(layout: dict[str, Any], manifest: dict[str, Any], triple: str) -> dict[str, str]:
    schema = layout.get("schema")
    schema_roles = {
        "aros-toolchain-tools-v1": {"c", "cxx", "assembler", "linker", "archive", "ranlib", "strip", "collector"},
        "aros-toolchain-tools-v2": {"c", "cxx", "assembler", "linker", "archive", "ranlib", "strip", "collector", "nm", "objcopy"},
        "aros-toolchain-tools-v3": {"c", "cxx", "assembler", "linker", "archive", "ranlib", "strip", "collector", "nm", "objcopy", "objdump"},
    }
    expected_keys = {"schema", "compiler", "target_triple", "tools"}
    if schema not in schema_roles or set(layout) != expected_keys:
        raise GateError("the GNU executable layout has an unsupported schema")
    if layout.get("compiler") != manifest.get("compiler") or layout.get("target_triple") != triple:
        raise GateError("the GNU executable layout does not bind to the verified manifest")
    tools = layout.get("tools")
    if not isinstance(tools, dict) or set(tools) != schema_roles[schema]:
        raise GateError("the GNU executable layout has an invalid role set")
    resolved: dict[str, str] = {}
    for role in ("c", "cxx", "assembler", "linker", "archive", "ranlib", "strip", "collector", "nm", "objcopy", "objdump"):
        if role not in tools:
            continue
        resolved[role] = _safe_manifest_path(tools[role])
    return resolved


def _resolve_executable(root: Path, relative: str, inventory: dict[str, dict[str, Any]]) -> Path:
    record = inventory.get(relative)
    if record is None or record.get("type") not in ("file", "symlink"):
        raise GateError("a declared GNU executable is absent from the measured inventory")
    candidate = root.joinpath(*PurePosixPath(relative).parts)
    try:
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(root)
        metadata = resolved.stat()
    except (OSError, ValueError) as error:
        raise GateError("a declared GNU executable is missing or escapes the package") from error
    if not stat.S_ISREG(metadata.st_mode) or not os.access(resolved, os.X_OK):
        raise GateError("a declared GNU executable is not a regular executable file")
    return resolved


def _verify_measured_readelf(root: Path, relative: str, inventory: dict[str, dict[str, Any]]) -> Path:
    record = inventory.get(relative)
    if (
        record is None
        or record.get("type") != "file"
        or record.get("mode") != "0755"
        or type(record.get("size")) is not int
        or not isinstance(record.get("sha256"), str)
        or re.fullmatch(r"[0-9a-f]{64}", record["sha256"]) is None
    ):
        raise GateError("the target-triple readelf is not a measured executable file")
    candidate = root.joinpath(*PurePosixPath(relative).parts)
    try:
        metadata = candidate.lstat()
    except OSError as error:
        raise GateError("the target-triple readelf is missing") from error
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise GateError("the target-triple readelf must be a regular file, not a symlink")
    if not metadata.st_mode & 0o111 or metadata.st_size != record["size"]:
        raise GateError("the target-triple readelf mode or size differs from the measured inventory")
    if metadata.st_size > MAX_TOOL_BYTES:
        raise GateError("the target-triple readelf exceeds the bounded executable size")
    _size, digest = _hash_regular_file(candidate, MAX_TOOL_BYTES)
    if digest != record["sha256"]:
        raise GateError("the target-triple readelf bytes differ from the measured inventory")
    return candidate


def inspect_package(root: Path) -> PackageDescription:
    manifest_bytes = _read_regular_file(root / MANIFEST_NAME, MAX_MANIFEST_BYTES)
    manifest = _parse_json(manifest_bytes, "toolchain-manifest.json")
    triple = _validate_rv32_fixture_identity(manifest)
    inventory = _inventory(manifest)

    layout_bytes = _read_regular_file(root / LAYOUT_NAME, MAX_LAYOUT_BYTES)
    layout = _parse_json(layout_bytes, "toolchain-tools.json")
    layout_roles = _layout_roles(layout, manifest, triple)
    layout_record = inventory.get(LAYOUT_NAME)
    if (
        layout_record is None
        or layout_record.get("type") != "file"
        or layout_record.get("sha256") != hashlib.sha256(layout_bytes).hexdigest()
    ):
        raise GateError("toolchain-tools.json does not match the manifest inventory")

    roles = {
        role: _resolve_executable(root, layout_roles[role], inventory)
        for role in ("c", "cxx", "archive", "ranlib")
    }
    common_parent = {path.parent for path in roles.values()}
    if len(common_parent) != 1:
        raise GateError("the GNU C, C++, archive and ranlib roles must share one package directory")
    readelf_relative = (PurePosixPath(layout_roles["c"]).parent / f"{triple}-readelf").as_posix()
    readelf = _verify_measured_readelf(root, readelf_relative, inventory)
    return PackageDescription(
        manifest_bytes=manifest_bytes,
        layout_bytes=layout_bytes,
        manifest=manifest,
        layout=layout,
        roles=roles,
        readelf=readelf,
    )


def load_inputs() -> GateInputs:
    source_root = _canonical_directory("AROS_TEST_SOURCE_ROOT")
    _source_root_is_checkout(source_root)
    preset = _required_env("AROS_TEST_GNU_SOURCE_PRESET")
    if len(preset) > MAX_PRESET_LENGTH or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", preset) is None:
        raise GateError("AROS_TEST_GNU_SOURCE_PRESET must be a bounded source preset name")
    cli = _canonical_executable("AROS_TEST_AROS")
    package_root = _canonical_directory("AROS_TEST_GNU_TOOLCHAIN_ROOT")
    cmake_name = shutil.which("cmake")
    if cmake_name is None:
        raise GateError("cmake is required by gnu-package-test")
    if shutil.which("ninja") is None:
        raise GateError("ninja is required by gnu-package-test")
    cmake = Path(cmake_name).resolve(strict=True)
    return GateInputs(source_root, preset, cli, package_root, cmake)


def verify_package_with_cli(inputs: GateInputs) -> None:
    # `toolchain verify` has no --format option. Its successful explicit-local
    # path verifies schema-specific identity, the full tree inventory and GNU
    # layout, then probes the declared compiler executables.
    timeout = 300
    try:
        result = _run_bounded(
            [
                os.fspath(inputs.cli),
                "toolchain",
                "verify",
                "--preset",
                inputs.preset,
                "--local",
                os.fspath(inputs.package_root),
            ],
            cwd=inputs.source_root,
            env=_child_environment(),
            timeout=timeout,
        )
    except OSError as error:
        raise GateError("runtime toolchain verification could not start") from error
    if result.timed_out or result.output_limited or result.returncode != 0:
        raise GateError(
            _command_diagnostic("runtime toolchain verification failed", result, timeout)
        )


def run_fixture(inputs: GateInputs, package: PackageDescription) -> None:
    if not FIXTURE.is_file() or not ENGINE_ROOT.is_dir():
        raise GateError("the package-qualified GNU CMake fixture is missing")
    timeout = 900
    try:
        with tempfile.TemporaryDirectory(prefix="aros-gnu-package-fixture-") as scratch:
            definitions = {
                "ENGINE_DIR": os.fspath(ENGINE_ROOT),
                "TEST_BINARY_DIR": scratch,
                "GNU_CC": os.fspath(package.roles["c"]),
                "GNU_CXX": os.fspath(package.roles["cxx"]),
                "GNU_READELF": os.fspath(package.readelf),
                "GNU_AR": os.fspath(package.roles["archive"]),
                "GNU_RANLIB": os.fspath(package.roles["ranlib"]),
            }
            if any(";" in value or "\n" in value or "\r" in value for value in definitions.values()):
                raise GateError("a CMake fixture path contains an unsupported list separator")
            command = [
                os.fspath(inputs.cmake),
                *(f"-D{name}={value}" for name, value in definitions.items()),
                "-P",
                os.fspath(FIXTURE),
            ]
            result = _run_bounded(
                command,
                cwd=inputs.source_root,
                env=_child_environment(),
                timeout=timeout,
                limit_child_file_size=True,
            )
    except (OSError, subprocess.SubprocessError) as error:
        raise GateError("the package-qualified GNU CMake fixture could not start") from error
    if result.timed_out or result.output_limited or result.returncode != 0:
        raise GateError(
            _command_diagnostic("the package-qualified GNU CMake fixture failed", result, timeout)
        )


def main() -> int:
    try:
        inputs = load_inputs()
        package_initial = inspect_package(inputs.package_root)
        verify_package_with_cli(inputs)
        package_before = inspect_package(inputs.package_root)
        if (
            package_before.manifest_bytes != package_initial.manifest_bytes
            or package_before.layout_bytes != package_initial.layout_bytes
        ):
            raise GateError("the selected GNU package identity changed during verification")
        run_fixture(inputs, package_before)
        verify_package_with_cli(inputs)
        package_after = inspect_package(inputs.package_root)
        if (
            package_after.manifest_bytes != package_before.manifest_bytes
            or package_after.layout_bytes != package_before.layout_bytes
        ):
            raise GateError("the selected GNU package identity changed during the fixture")
    except GateError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "GNU package fixture passed for "
        f"{inputs.preset} ({package_before.manifest['target_triple']})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
