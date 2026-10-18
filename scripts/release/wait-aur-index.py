#!/usr/bin/env python3
"""Wait for the public AUR RPC index to expose the exact stable package."""

from dataclasses import dataclass
from email.utils import parsedate_to_datetime
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time


PACKAGE = "aros-tools-bin"
RPC_URL = "https://aur.archlinux.org/rpc"
DEADLINE_SECONDS = 1200
CONNECT_TIMEOUT_SECONDS = 10
REQUEST_TIMEOUT_SECONDS = 20
MAX_BODY_BYTES = 1024 * 1024
MAX_HEADER_BYTES = 64 * 1024
RETRY_BACKOFF_SECONDS = (10, 20, 40, 60)
VERSION_RE = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\Z")
ARCH_VERSION_RE = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)-([1-9][0-9]*)\Z"
)
STATUS_MARKER = b"\n__AUR_RPC_STATUS__"
TRANSIENT_CURL_CODES = {5, 6, 7, 16, 18, 28, 52, 55, 56, 92}


class VerifyError(Exception):
    """The public AUR package cannot be safely verified."""


class TransientTransportError(Exception):
    """A known temporary transport failure that can be retried."""


@dataclass(frozen=True)
class Response:
    status: int
    body: bytes
    retry_after: str | None = None


def fail(code: str, message: str) -> VerifyError:
    return VerifyError(f"{code} {message}")


def parse_arch_version(value: str) -> tuple[int, int, int, int]:
    """Parse this project's canonical stable SemVer plus Arch pkgrel."""
    match = ARCH_VERSION_RE.fullmatch(value)
    if match is None:
        raise fail("AP7423", f"AUR returned a noncanonical package version: {value!r}")
    return tuple(int(part) for part in match.groups())


def parse_retry_after(value: str | None, *, wall_clock=time.time) -> float | None:
    """Read a valid Retry-After delta or HTTP date; ignore invalid values."""
    if value is None:
        return None
    value = value.strip()
    if re.fullmatch(r"[0-9]+", value):
        try:
            return float(int(value))
        except (ValueError, OverflowError):
            return float("inf")
    try:
        retry_at = parsedate_to_datetime(value)
        if retry_at is None or retry_at.tzinfo is None:
            return None
        return max(0.0, retry_at.timestamp() - wall_clock())
    except (TypeError, ValueError, OverflowError):
        return None


def _retry_after_header(path: Path) -> str | None:
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise fail("AP7422", "cannot read AUR RPC response headers") from error
    if len(raw) > MAX_HEADER_BYTES:
        raise fail("AP7422", "AUR RPC response headers exceed the size limit")

    blocks = re.split(rb"\r?\n\r?\n", raw)
    final = next(
        (
            block for block in reversed(blocks)
            if block.splitlines()
            and re.fullmatch(
                rb"HTTP/[^ ]+ [0-9]{3}(?:[ \t].*|[ \t]*\r?)",
                block.splitlines()[0],
            )
        ),
        None,
    )
    if final is None:
        return None
    values = []
    for line in final.splitlines()[1:]:
        if b":" not in line:
            continue
        name, value = line.split(b":", 1)
        if name.strip().lower() == b"retry-after":
            try:
                values.append(value.decode("ascii").strip())
            except UnicodeDecodeError as error:
                raise fail("AP7422", "AUR RPC Retry-After header is not ASCII") from error
    if len(values) > 1:
        raise fail("AP7422", "AUR RPC returned ambiguous Retry-After headers")
    return values[0] if values else None


def curl_request(*, connect_timeout: float, timeout: float) -> Response:
    """Fetch one bounded, unauthenticated AUR RPC response using curl."""
    with tempfile.TemporaryDirectory(prefix="aros-aur-rpc-") as temp_dir:
        headers_path = Path(temp_dir) / "headers"
        command = [
            "curl",
            "--disable",
            "--fail-with-body",
            "--silent",
            "--show-error",
            "--get",
            "--proto", "=https",
            "--tlsv1.2",
            "--connect-timeout", f"{connect_timeout:.6f}",
            "--max-time", f"{timeout:.6f}",
            "--max-filesize", str(MAX_BODY_BYTES),
            "--dump-header", str(headers_path),
            "--write-out", "\n__AUR_RPC_STATUS__%{response_code}",
            "--data-urlencode", "v=5",
            "--data-urlencode", "type=info",
            "--data-urlencode", f"arg[]={PACKAGE}",
            RPC_URL,
        ]
        try:
            result = subprocess.run(
                command, capture_output=True, timeout=timeout, check=False
            )
        except subprocess.TimeoutExpired as error:
            raise TransientTransportError("AUR RPC request timed out") from error
        except OSError as error:
            raise fail("AP7422", "cannot start curl for the AUR RPC request") from error

        if result.returncode not in (0, 22):
            if result.returncode in TRANSIENT_CURL_CODES:
                raise TransientTransportError(
                    f"temporary curl transport failure ({result.returncode})"
                )
            if result.returncode == 60:
                raise fail("AP7422", "AUR RPC TLS certificate verification failed")
            raise fail("AP7422", f"curl failed for the AUR RPC request ({result.returncode})")

        output = result.stdout or b""
        if len(output) > MAX_BODY_BYTES + 128:
            raise fail("AP7422", "AUR RPC response exceeds the size limit")
        if STATUS_MARKER not in output:
            raise fail("AP7422", "curl returned no AUR RPC HTTP status")
        body, status_raw = output.rsplit(STATUS_MARKER, 1)
        if re.fullmatch(rb"[0-9]{3}", status_raw) is None:
            raise fail("AP7422", "curl returned malformed AUR RPC HTTP metadata")
        if len(body) > MAX_BODY_BYTES:
            raise fail("AP7422", "AUR RPC response exceeds the size limit")
        status = int(status_raw)
        if status == 0:
            raise fail("AP7422", "curl received no AUR RPC HTTP response")
        return Response(status, body, _retry_after_header(headers_path))


def _reject_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise fail("AP7423", f"AUR RPC JSON contains duplicate key {key!r}")
        result[key] = value
    return result


def _reject_json_constant(value):
    raise ValueError(f"invalid JSON constant {value}")


def classify_response(body: bytes, expected: tuple[int, int, int, int]) -> bool:
    """Return true for the exact version, false while the index is stale."""
    if len(body) > MAX_BODY_BYTES:
        raise fail("AP7423", "AUR RPC JSON exceeds the size limit")
    try:
        data = json.loads(
            body.decode("utf-8"),
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_json_constant,
        )
    except VerifyError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as error:
        raise fail("AP7423", "AUR RPC returned malformed JSON") from error

    if not isinstance(data, dict):
        raise fail("AP7423", "AUR RPC JSON has an invalid top-level shape")
    count = data.get("resultcount")
    results = data.get("results")
    if type(count) is not int or count < 0 or not isinstance(results, list) or count != len(results):
        raise fail("AP7423", "AUR RPC result count and entries are inconsistent")
    if count == 0:
        return False
    if count != 1:
        raise fail("AP7423", "AUR RPC returned ambiguous package results")

    package = results[0]
    if not isinstance(package, dict):
        raise fail("AP7423", "AUR RPC package result is malformed")
    if package.get("Name") != PACKAGE:
        raise fail("AP7423", "AUR RPC returned a different package name")
    version = package.get("Version")
    if not isinstance(version, str):
        raise fail("AP7423", "AUR RPC package version is missing or malformed")
    actual = parse_arch_version(version)
    if actual == expected:
        return True
    if actual < expected:
        return False
    raise fail("AP7423", f"AUR index has newer version {version}; refusing to verify {'.'.join(map(str, expected[:3]))}-1")


def _timeout_error(deadline_seconds: float) -> VerifyError:
    return fail("AP7420", f"AUR metadata did not converge within {deadline_seconds:g} seconds")


def wait_for_index(
    version: str,
    *,
    transport=curl_request,
    clock=time.monotonic,
    sleep=time.sleep,
    wall_clock=time.time,
    deadline_seconds: float = DEADLINE_SECONDS,
) -> None:
    """Wait for the public RPC index, with one total monotonic deadline."""
    if not isinstance(version, str) or VERSION_RE.fullmatch(version) is None:
        raise fail("AP7421", "release version must be canonical stable SemVer")
    if deadline_seconds <= 0:
        raise fail("AP7421", "AUR index deadline must be positive")
    expected = parse_arch_version(f"{version}-1")
    deadline = clock() + deadline_seconds
    backoff_index = 0

    while True:
        remaining = deadline - clock()
        if remaining <= 0:
            raise _timeout_error(deadline_seconds)
        request_timeout = min(REQUEST_TIMEOUT_SECONDS, remaining)
        connect_timeout = min(CONNECT_TIMEOUT_SECONDS, request_timeout)
        try:
            response = transport(connect_timeout=connect_timeout, timeout=request_timeout)
        except TransientTransportError:
            if clock() >= deadline:
                raise _timeout_error(deadline_seconds)
            retry_after = None
            pending = True
        else:
            if clock() >= deadline:
                raise _timeout_error(deadline_seconds)
            if not isinstance(response, Response):
                raise fail("AP7422", "AUR RPC transport returned invalid response metadata")
            if response.status in (408, 429) or 500 <= response.status <= 599:
                retry_after = parse_retry_after(response.retry_after, wall_clock=wall_clock)
                pending = True
            elif response.status != 200:
                raise fail("AP7422", f"AUR RPC returned permanent HTTP status {response.status}")
            else:
                exact = classify_response(response.body, expected)
                if clock() >= deadline:
                    raise _timeout_error(deadline_seconds)
                if exact:
                    return
                retry_after = None
                pending = True

        if pending:
            backoff = RETRY_BACKOFF_SECONDS[min(backoff_index, len(RETRY_BACKOFF_SECONDS) - 1)]
            delay = max(backoff, retry_after) if retry_after is not None else backoff
            remaining = deadline - clock()
            if remaining <= 0:
                raise _timeout_error(deadline_seconds)
            sleep(min(delay, remaining))
            backoff_index += 1


def main(argv=None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 1:
        raise fail("AP7421", "usage: wait-aur-index.py VERSION")
    wait_for_index(argv[0])
    print(f"AUR RPC index exposes {PACKAGE} {argv[0]}-1.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except VerifyError as error:
        print(f"::error::{error}", file=sys.stderr)
        sys.exit(1)
