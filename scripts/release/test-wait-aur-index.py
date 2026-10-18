#!/usr/bin/env python3
"""Offline tests for bounded AUR RPC index convergence."""

from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("wait-aur-index.py")
spec = importlib.util.spec_from_file_location("wait_aur_index", SCRIPT)
aur = importlib.util.module_from_spec(spec)
spec.loader.exec_module(aur)


def package_response(version="0.6.0-1", *, name=aur.PACKAGE, count=1, results=None):
    if results is None:
        results = [] if count == 0 else [{"Name": name, "Version": version}]
    return aur.Response(200, json.dumps({"resultcount": count, "results": results}).encode())


class FakeTime:
    def __init__(self, wall=1_800_000_000):
        self.now = 0.0
        self.wall = wall
        self.sleeps = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.sleeps.append(seconds)
        self.now += seconds

    def wall_clock(self):
        return self.wall


class QueueTransport:
    def __init__(self, results, *, clock=None, elapsed=0.0):
        self.results = list(results)
        self.calls = []
        self.clock = clock
        self.elapsed = elapsed

    def __call__(self, *, connect_timeout, timeout):
        self.calls.append((connect_timeout, timeout))
        if self.clock is not None:
            self.clock.now += self.elapsed
        result = self.results.pop(0)
        if isinstance(result, Exception):
            raise result
        return result


class WaitForAURIndex(unittest.TestCase):
    def setUp(self):
        self.time = FakeTime()

    def wait(self, responses, **kwargs):
        transport = QueueTransport(responses, clock=self.time, elapsed=kwargs.pop("elapsed", 0.0))
        aur.wait_for_index(
            "0.6.0", transport=transport, clock=self.time.monotonic,
            sleep=self.time.sleep, wall_clock=self.time.wall_clock, **kwargs,
        )
        return transport

    def test_stale_older_version_then_exact_version_succeeds(self):
        transport = self.wait([package_response("0.5.9-7"), package_response()])
        self.assertEqual(len(transport.calls), 2)
        self.assertEqual(self.time.sleeps, [10])

    def test_exact_latest_version_succeeds_without_retry(self):
        transport = self.wait([package_response()])
        self.assertEqual(len(transport.calls), 1)
        self.assertEqual(self.time.sleeps, [])

    def test_absent_package_then_exact_version_succeeds(self):
        transport = self.wait([package_response(count=0), package_response()])
        self.assertEqual(len(transport.calls), 2)
        self.assertEqual(self.time.sleeps, [10])

    def test_transient_http_and_transport_failures_retry(self):
        transport = self.wait([
            aur.Response(503, b"unavailable"),
            aur.TransientTransportError("connection reset"),
            package_response(),
        ])
        self.assertEqual(len(transport.calls), 3)
        self.assertEqual(self.time.sleeps, [10, 20])

    def test_retry_after_delta_and_http_date_are_honored(self):
        date = datetime.fromtimestamp(self.time.wall + 45, timezone.utc).strftime(
            "%a, %d %b %Y %H:%M:%S GMT"
        )
        transport = self.wait([
            aur.Response(429, b"rate limited", "25"),
            aur.Response(503, b"unavailable", date),
            package_response(),
        ])
        self.assertEqual(len(transport.calls), 3)
        self.assertEqual(self.time.sleeps, [25, 45])

    def test_retry_after_cannot_extend_the_total_deadline(self):
        transport = QueueTransport([aur.Response(429, b"rate limited", "3600")])
        with self.assertRaisesRegex(aur.VerifyError, "within 30 seconds"):
            aur.wait_for_index(
                "0.6.0", transport=transport, clock=self.time.monotonic,
                sleep=self.time.sleep, deadline_seconds=30,
            )
        self.assertEqual(len(transport.calls), 1)
        self.assertEqual(self.time.sleeps, [30])
        self.assertEqual(self.time.now, 30)

    def test_permanent_http_malformed_ambiguous_wrong_and_newer_fail_fast(self):
        cases = [
            aur.Response(401, b"unauthorized"),
            aur.Response(404, b"not found"),
            aur.Response(200, b"{"),
            package_response(count=2, results=[
                {"Name": aur.PACKAGE, "Version": "0.6.0-1"},
                {"Name": aur.PACKAGE, "Version": "0.6.0-1"},
            ]),
            package_response(name="another-package"),
            package_response("0.6.1-1"),
        ]
        for response in cases:
            with self.subTest(response=response):
                transport = QueueTransport([response])
                with self.assertRaises(aur.VerifyError):
                    aur.wait_for_index(
                        "0.6.0", transport=transport, clock=self.time.monotonic,
                        sleep=self.time.sleep, wall_clock=self.time.wall_clock,
                    )
                self.assertEqual(len(transport.calls), 1)
                self.assertEqual(self.time.sleeps, [])

    def test_duplicate_keys_and_oversized_json_fail_fast(self):
        for body in (
            b'{"resultcount":0,"resultcount":1,"results":[]}',
            b" " * (aur.MAX_BODY_BYTES + 1),
        ):
            with self.subTest(length=len(body)), self.assertRaises(aur.VerifyError):
                aur.wait_for_index(
                    "0.6.0", transport=QueueTransport([aur.Response(200, body)]),
                    clock=self.time.monotonic, sleep=self.time.sleep,
                )
        self.assertEqual(self.time.sleeps, [])

    def test_deadline_caps_backoff_and_each_request_timeout(self):
        self.assertEqual(aur.DEADLINE_SECONDS, 1200)
        transport = QueueTransport(
            [package_response("0.5.0-1") for _ in range(6)], clock=self.time
        )
        with self.assertRaisesRegex(aur.VerifyError, "within 135 seconds"):
            aur.wait_for_index(
                "0.6.0", transport=transport, clock=self.time.monotonic,
                sleep=self.time.sleep, deadline_seconds=135,
            )
        self.assertEqual(self.time.sleeps, [10, 20, 40, 60, 5])
        self.assertEqual([call[1] for call in transport.calls], [20, 20, 20, 20, 5])
        self.assertEqual([call[0] for call in transport.calls], [10, 10, 10, 10, 5])
        self.assertEqual(self.time.now, 135)

    def test_transport_time_counts_toward_the_total_deadline(self):
        transport = QueueTransport(
            [package_response()], clock=self.time, elapsed=21
        )
        with self.assertRaisesRegex(aur.VerifyError, "within 20 seconds"):
            aur.wait_for_index(
                "0.6.0", transport=transport, clock=self.time.monotonic,
                sleep=self.time.sleep, deadline_seconds=20,
            )
        self.assertEqual(len(transport.calls), 1)
        self.assertEqual(transport.calls[0], (10, 20))

    def test_invalid_version_is_rejected_before_transport(self):
        calls = []

        def transport(**kwargs):
            calls.append(kwargs)
            return package_response()

        for value in ("v0.6.0", "0.6", "0.06.0", "0.6.0-rc.1", ""):
            with self.subTest(version=value), self.assertRaises(aur.VerifyError):
                aur.wait_for_index(value, transport=transport)
        self.assertEqual(calls, [])


class CurlRequest(unittest.TestCase):
    def curl_result(self, *, returncode, output, headers):
        def run(command, **kwargs):
            header_path = Path(command[command.index("--dump-header") + 1])
            header_path.write_bytes(headers)
            self.command = command
            self.run_kwargs = kwargs
            return subprocess.CompletedProcess(command, returncode, output, b"")

        return run

    def test_https_request_is_bounded_and_parses_status_and_body(self):
        body = b'{"resultcount":0,"results":[]}'
        run = self.curl_result(
            returncode=0,
            output=body + b"\n__AUR_RPC_STATUS__200",
            headers=b"HTTP/2 200\r\nContent-Type: application/json\r\n\r\n",
        )
        with patch.object(aur.subprocess, "run", side_effect=run):
            response = aur.curl_request(connect_timeout=4.5, timeout=8.0)

        self.assertEqual(response, aur.Response(200, body))
        self.assertEqual(self.command[:3], ["curl", "--disable", "--fail-with-body"])
        self.assertIn(("--proto", "=https"), list(zip(self.command, self.command[1:])))
        self.assertIn("--tlsv1.2", self.command)
        self.assertIn(("--connect-timeout", "4.500000"), list(zip(self.command, self.command[1:])))
        self.assertIn(("--max-time", "8.000000"), list(zip(self.command, self.command[1:])))
        self.assertIn(("--max-filesize", str(aur.MAX_BODY_BYTES)), list(zip(self.command, self.command[1:])))
        self.assertFalse(any(option in ("-L", "--location", "--location-trusted") for option in self.command))
        self.assertEqual(self.run_kwargs, {"capture_output": True, "timeout": 8.0, "check": False})

    def test_429_parses_retry_after_from_http2_final_headers(self):
        run = self.curl_result(
            returncode=22,
            output=b"rate limited\n__AUR_RPC_STATUS__429",
            headers=b"HTTP/2 429\r\nRetry-After: 17\r\n\r\n",
        )
        with patch.object(aur.subprocess, "run", side_effect=run):
            response = aur.curl_request(connect_timeout=10, timeout=20)
        self.assertEqual(response, aur.Response(429, b"rate limited", "17"))

    def test_certificate_failure_is_permanent(self):
        run = self.curl_result(returncode=60, output=b"", headers=b"")
        with patch.object(aur.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(aur.VerifyError, "TLS certificate verification failed"):
                aur.curl_request(connect_timeout=10, timeout=20)

    def test_subprocess_timeout_is_retryable(self):
        def timeout(command, **kwargs):
            raise subprocess.TimeoutExpired(command, kwargs["timeout"])

        with patch.object(aur.subprocess, "run", side_effect=timeout):
            with self.assertRaises(aur.TransientTransportError):
                aur.curl_request(connect_timeout=0.5, timeout=0.5)


if __name__ == "__main__":
    unittest.main()
