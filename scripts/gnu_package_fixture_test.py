"""Selector and routing tests with inert tools; these do not qualify a compiler."""

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
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/check-gnu-package-fixture.py"
SPEC = importlib.util.spec_from_file_location("gnu_package_fixture_gate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
gate = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = gate
SPEC.loader.exec_module(gate)


class GnuPackageFixtureGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        package = self.root / "package"
        package.mkdir()
        self.package = package.resolve()
        self.target = {
            "schema": "aros-riscv-target-v1",
            "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
            "abi": "ilp32f",
            "code_model": "medany",
            "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
            "unaligned_access": False,
            "atomic_abi": 0,
            "x3_reg_usage": 0,
        }
        self.compiler = {
            "family": "gnu",
            "gcc_version": "16.2.0",
            "binutils_version": "2.47",
            "target": self.target,
        }
        self.triple = "riscv-aros"
        self.tools = {
            "c": "riscv-aros-gcc",
            "cxx": "riscv-aros-g++",
            "assembler": "riscv-aros-as",
            "linker": "riscv-aros-ld",
            "archive": "riscv-aros-ar",
            "ranlib": "riscv-aros-ranlib",
            "strip": "riscv-aros-strip",
            "collector": "riscv-aros/bin/collect-aros",
            "nm": "riscv-aros-nm",
            "objcopy": "riscv-aros-objcopy",
            "objdump": "riscv-aros-objdump",
        }
        for relative in set(self.tools.values()) | {f"{self.triple}-readelf"}:
            path = self.package / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        self.write_contracts()

    def write_contracts(self):
        layout = {
            "schema": "aros-toolchain-tools-v3",
            "compiler": self.compiler,
            "target_triple": self.triple,
            "tools": self.tools,
        }
        layout_bytes = json.dumps(layout, separators=(",", ":"), sort_keys=True).encode()
        (self.package / "toolchain-tools.json").write_bytes(layout_bytes)
        inventory = []
        for relative in sorted(set(self.tools.values()) | {f"{self.triple}-readelf", "toolchain-tools.json"}):
            path = self.package / relative
            if not path.is_file():
                continue
            payload = path.read_bytes()
            inventory.append({
                "path": relative,
                "type": "file",
                "mode": "0644" if relative == "toolchain-tools.json" else "0755",
                "size": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            })
        self.manifest = {
            "schema": 2,
            "compiler": self.compiler,
            "target_triple": self.triple,
            "files": inventory,
        }
        manifest_bytes = json.dumps(self.manifest, separators=(",", ":"), sort_keys=True).encode()
        (self.package / "toolchain-manifest.json").write_bytes(manifest_bytes)

    def load_layout(self):
        return json.loads((self.package / "toolchain-tools.json").read_bytes())

    def rewrite_layout(self, layout):
        self.tools = layout["tools"]
        self.compiler = layout["compiler"]
        self.target = self.compiler["target"]
        self.triple = layout["target_triple"]
        self.write_contracts()

    def test_positive_package_selects_declared_roles_and_measured_readelf(self):
        selected = gate.inspect_package(self.package)
        self.assertEqual(selected.roles["c"], self.package / self.tools["c"])
        self.assertEqual(selected.roles["cxx"], self.package / self.tools["cxx"])
        self.assertEqual(selected.roles["archive"], self.package / self.tools["archive"])
        self.assertEqual(selected.roles["ranlib"], self.package / self.tools["ranlib"])
        self.assertEqual(selected.readelf, self.package / "riscv-aros-readelf")

    def test_missing_manifest_fails_before_runtime_verification(self):
        (self.package / "toolchain-manifest.json").unlink()
        with self.assertRaises(gate.GateError):
            gate.inspect_package(self.package)

    def test_wrong_declared_role_is_rejected(self):
        layout = self.load_layout()
        layout["tools"]["c"] = "missing-riscv-aros-gcc"
        self.rewrite_layout(layout)
        with self.assertRaisesRegex(gate.GateError, "declared GNU executable"):
            gate.inspect_package(self.package)

    def test_declared_symlink_role_resolves_to_canonical_executable(self):
        target = self.package / self.tools["c"]
        alias = self.package / "riscv-aros-gcc-alias"
        alias.symlink_to(target.name)
        selected = gate._resolve_executable(
            self.package,
            alias.name,
            {alias.name: {"type": "symlink"}},
        )
        self.assertEqual(selected, target.resolve())

    def test_traversal_in_declared_role_is_rejected(self):
        layout = self.load_layout()
        layout["tools"]["c"] = "../outside-gcc"
        self.rewrite_layout(layout)
        with self.assertRaisesRegex(gate.GateError, "unsafe path"):
            gate.inspect_package(self.package)

    def test_readelf_must_be_a_measured_regular_executable(self):
        readelf = self.package / "riscv-aros-readelf"
        readelf.unlink()
        self.manifest["files"] = [
            item for item in self.manifest["files"] if item["path"] != "riscv-aros-readelf"
        ]
        (self.package / "toolchain-manifest.json").write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(gate.GateError, "target-triple readelf"):
            gate.inspect_package(self.package)

    def test_wrong_abi_is_rejected_even_when_layout_matches_manifest(self):
        layout = self.load_layout()
        layout["compiler"]["target"]["abi"] = "ilp32d"
        self.rewrite_layout(layout)
        with self.assertRaisesRegex(gate.GateError, "ABI must be RV32 ilp32f"):
            gate.inspect_package(self.package)

    def test_wrong_family_and_wrong_rv32_isa_are_rejected(self):
        layout = self.load_layout()
        layout["compiler"]["family"] = "llvm"
        self.rewrite_layout(layout)
        with self.assertRaisesRegex(gate.GateError, "GNU compiler identity"):
            gate.inspect_package(self.package)

        layout = self.load_layout()
        layout["compiler"]["family"] = "gnu"
        layout["compiler"]["target"]["isa"] = "rv64imafdc"
        self.rewrite_layout(layout)
        with self.assertRaisesRegex(gate.GateError, "ISA must include"):
            gate.inspect_package(self.package)

    def _runtime_fixture(self):
        source = self.root / "source"
        source.mkdir()
        (source / "aros-targets.toml").write_text("[[targets]]\n")
        subprocess.run(["git", "-C", str(source), "init", "--quiet"], check=True, timeout=20)
        fake_bin = self.root / "bin"
        fake_bin.mkdir()
        runtime_log = self.root / "runtime.jsonl"
        script = fake_bin / "record"
        script.write_text(
            f"#!{sys.executable}\n"
            "import json, os, pathlib, sys\n"
            "name = pathlib.Path(sys.argv[0]).name\n"
            "with open(os.environ['FAKE_RUNTIME_LOG'], 'a') as stream:\n"
            "    stream.write(json.dumps({'name': name, 'args': sys.argv[1:], 'cwd': os.getcwd(), 'env_present': {key: bool(os.environ.get(key)) for key in ('GITHUB_TOKEN', 'GH_TOKEN', 'AROS_BUILD_TOKEN', 'CODEX_GOVERNANCE', 'ACTIONS_ID_TOKEN_REQUEST_URL', 'DYLD_INSERT_LIBRARIES', 'LD_PRELOAD', 'PATH')}}) + '\\n')\n"
            "if os.environ.get('FAKE_FAIL_TOOL') == name:\n"
            "    print('fake stdout diagnostic')\n"
            "    print('fake stderr diagnostic', file=sys.stderr)\n"
            "    sys.exit(int(os.environ.get('FAKE_FAIL_EXIT', '23')))\n"
        )
        script.chmod(0o755)
        cli = fake_bin / "aros"
        cli.symlink_to(script)
        # The gate requires the exact executable to be canonical, so use a
        # separately named regular file rather than the symlink above.
        cli.unlink()
        cli.write_text(script.read_text())
        cli.chmod(0o755)
        cmake = fake_bin / "cmake"
        cmake.write_text(script.read_text())
        cmake.chmod(0o755)
        ninja = fake_bin / "ninja"
        ninja.write_text(script.read_text())
        ninja.chmod(0o755)
        env = {
            key: value for key, value in os.environ.items()
            if not key.startswith("AROS_TEST_")
        }
        env.update({
            "PATH": str(fake_bin) + os.pathsep + os.environ.get("PATH", ""),
            "FAKE_RUNTIME_LOG": str(runtime_log),
            "AROS_TEST_SOURCE_ROOT": str(source.resolve()),
            "AROS_TEST_GNU_SOURCE_PRESET": "fixture-rv32",
            "AROS_TEST_AROS": str(cli.resolve()),
            "AROS_TEST_GNU_TOOLCHAIN_ROOT": str(self.package.resolve()),
        })
        return source, cli, runtime_log, env

    def test_explicit_mode_uses_exact_cli_source_cwd_and_package_roles(self):
        source, _cli, runtime_log, env = self._runtime_fixture()
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], cwd=self.root, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        records = [json.loads(line) for line in runtime_log.read_text().splitlines()]
        cli_calls = [item for item in records if item["name"] == "aros"]
        cmake_calls = [item for item in records if item["name"] == "cmake"]
        self.assertEqual(len(cli_calls), 2)
        self.assertEqual(len(cmake_calls), 1)
        expected_cli = [
            "toolchain", "verify", "--preset", "fixture-rv32",
            "--local", str(self.package),
        ]
        for call in cli_calls:
            self.assertEqual(call["args"], expected_cli)
            self.assertEqual(Path(call["cwd"]), source.resolve())
        self.assertEqual(Path(cmake_calls[0]["cwd"]), source.resolve())
        cmake_args = cmake_calls[0]["args"]
        self.assertIn(f"-DGNU_CC={self.package / self.tools['c']}", cmake_args)
        self.assertIn(f"-DGNU_CXX={self.package / self.tools['cxx']}", cmake_args)
        self.assertIn(f"-DGNU_AR={self.package / self.tools['archive']}", cmake_args)
        self.assertIn(f"-DGNU_RANLIB={self.package / self.tools['ranlib']}", cmake_args)
        self.assertIn(f"-DGNU_READELF={self.package / 'riscv-aros-readelf'}", cmake_args)
        self.assertNotIn("--format", json.dumps(records))

    def test_missing_explicit_mode_inputs_do_not_invoke_runtime(self):
        source, _cli, runtime_log, env = self._runtime_fixture()
        del env["AROS_TEST_GNU_SOURCE_PRESET"]
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], cwd=source, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("AROS_TEST_GNU_SOURCE_PRESET is required", result.stderr)
        self.assertFalse(runtime_log.exists())

    def test_runtime_verification_failure_retains_bounded_stdout_and_stderr(self):
        source, _cli, _runtime_log, env = self._runtime_fixture()
        env.update({
            "FAKE_FAIL_TOOL": "aros",
            "FAKE_FAIL_EXIT": "23",
            "GITHUB_TOKEN": "diagnostic-must-not-leak-github",
            "GH_TOKEN": "diagnostic-must-not-leak-gh",
            "AROS_BUILD_TOKEN": "diagnostic-must-not-leak-token",
            "CODEX_GOVERNANCE": "diagnostic-must-not-leak-governance",
            "ACTIONS_ID_TOKEN_REQUEST_URL": "diagnostic-must-not-leak-oidc",
        })
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], cwd=source, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exit 23", result.stderr)
        self.assertIn("fake stdout diagnostic", result.stderr)
        self.assertIn("fake stderr diagnostic", result.stderr)
        for secret in (
            "diagnostic-must-not-leak-github",
            "diagnostic-must-not-leak-gh",
            "diagnostic-must-not-leak-token",
            "diagnostic-must-not-leak-governance",
            "diagnostic-must-not-leak-oidc",
        ):
            self.assertNotIn(secret, result.stdout + result.stderr)

    def test_cmake_fixture_failure_retains_bounded_stdout_and_stderr(self):
        source, _cli, runtime_log, env = self._runtime_fixture()
        env.update({"FAKE_FAIL_TOOL": "cmake", "FAKE_FAIL_EXIT": "17"})
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], cwd=source, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exit 17", result.stderr)
        self.assertIn("fake stdout diagnostic", result.stderr)
        self.assertIn("fake stderr diagnostic", result.stderr)
        records = [json.loads(line) for line in runtime_log.read_text().splitlines()]
        cmake_calls = [item for item in records if item["name"] == "cmake"]
        self.assertEqual(len(cmake_calls), 1)
        self.assertTrue(cmake_calls[0]["env_present"]["PATH"])

    def test_child_environment_removes_credentials(self):
        source, _cli, runtime_log, env = self._runtime_fixture()
        env.update({
            "GITHUB_TOKEN": "not-for-child",
            "GH_TOKEN": "not-for-child",
            "AROS_BUILD_TOKEN": "not-for-child",
            "CODEX_GOVERNANCE": "not-for-child",
            "ACTIONS_ID_TOKEN_REQUEST_URL": "not-for-child",
        })
        result = subprocess.run(
            [sys.executable, str(SCRIPT)], cwd=source, env=env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        records = [json.loads(line) for line in runtime_log.read_text().splitlines()]
        for record in records:
            for name in (
                "GITHUB_TOKEN", "GH_TOKEN", "AROS_BUILD_TOKEN", "CODEX_GOVERNANCE",
                "ACTIONS_ID_TOKEN_REQUEST_URL",
            ):
                self.assertFalse(record["env_present"][name], name)
            self.assertTrue(record["env_present"]["PATH"])

    def test_child_environment_removes_loader_and_runtime_injection(self):
        injected = {
            "SSH_AUTH_SOCK": "/private/agent/socket",
            "SSH_AGENT_PID": "123",
            "GPG_AGENT_INFO": "/private/agent/gpg:123:1",
            "GNUPGHOME": "/private/credentials/gpg",
            "DYLD_INSERT_LIBRARIES": "not-for-child",
            "LD_PRELOAD": "not-for-child",
            "GIT_DIR": "not-for-child",
            "PYTHONPATH": "not-for-child",
        }
        with mock.patch.dict(os.environ, injected):
            child_env = gate._child_environment()
        for name in injected:
            self.assertNotIn(name, child_env)

    def test_output_limit_is_explicit_and_capture_stays_bounded(self):
        for descriptor, label in ((1, "stdout"), (2, "stderr")):
            with self.subTest(stream=label):
                result = gate._run_bounded(
                    [sys.executable, "-c", f"import os; os.write({descriptor}, b'x' * 4096)"],
                    cwd=self.root,
                    env=gate._child_environment(),
                    timeout=5,
                    output_limit=128,
                )
                self.assertTrue(result.output_limited)
                self.assertFalse(result.timed_out)
                self.assertEqual(result.overflow_streams, (label,))
                self.assertLessEqual(len(result.stdout), 128)
                self.assertLessEqual(len(result.stderr), 128)
                captured = result.stdout if label == "stdout" else result.stderr
                self.assertEqual(len(captured), 128)
                self.assertIn(
                    f"{label} exceeded the 128-byte capture limit",
                    gate._command_diagnostic("probe", result, 5),
                )

    def test_cmake_nested_output_file_is_bounded_before_read(self):
        cmake = shutil.which("cmake")
        if cmake is None:
            self.skipTest("cmake is unavailable")
        generated = self.root / "nested-output-probe.cmake"
        bracket = lambda value: f"[==[{value}]==]"
        producer_code = "__import__('os').write(1, b'x' * 9437185)"
        generated.write_text(
            f"include({bracket(gate.CAPTURE_HELPER)})\n"
            "_literal_gnu_execute_bounded_process(\n"
            "  _probe_result _probe_stdout _probe_stderr\n"
            f"  {bracket('inert nested-output probe')} {bracket(self.root)} 30\n"
            f"  {bracket(sys.executable)} -c "
            f"{bracket(producer_code)}\n"
            ")\n"
            "message(FATAL_ERROR \"probe unexpectedly succeeded\")\n"
        )
        result = gate._run_bounded(
            [cmake, "-P", str(generated)],
            cwd=self.root,
            env=gate._child_environment(),
            timeout=30,
            limit_child_file_size=True,
        )
        diagnostic = (result.stdout + result.stderr).decode("utf-8", errors="replace")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("inert nested-output probe stdout capture exceeds", diagnostic)
        self.assertIn("1048576-byte limit", diagnostic)
        self.assertNotIn("probe unexpectedly succeeded", diagnostic)
        captures = list(self.root.glob(".gnu-fixture-*.stdout"))
        self.assertEqual(len(captures), 1)
        self.assertGreater(captures[0].stat().st_size, gate.MAX_CAPTURE_BYTES)
        self.assertLessEqual(captures[0].stat().st_size, gate.MAX_CHILD_FILE_BYTES)

    def test_cmake_capture_rejects_fifo_before_read(self):
        cmake = shutil.which("cmake")
        if cmake is None:
            self.skipTest("cmake is unavailable")
        fifo = self.root / "capture.fifo"
        os.mkfifo(fifo)
        generated = self.root / "fifo-probe.cmake"
        generated.write_text(
            f"include([==[{gate.CAPTURE_HELPER}]==])\n"
            f"_literal_gnu_check_bounded_file([==[{fifo}]==] \"FIFO capture\")\n"
            'message(FATAL_ERROR "probe unexpectedly succeeded")\n'
        )
        result = gate._run_bounded(
            [cmake, "-P", str(generated)],
            cwd=self.root,
            env=gate._child_environment(),
            timeout=10,
        )
        diagnostic = (result.stdout + result.stderr).decode("utf-8", errors="replace")
        self.assertFalse(result.timed_out)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FIFO capture is not a regular file", diagnostic)
        self.assertNotIn("probe unexpectedly succeeded", diagnostic)

    def test_timeout_is_explicit(self):
        started = time.monotonic()
        result = gate._run_bounded(
            [sys.executable, "-c", "import time; time.sleep(5)"],
            cwd=self.root,
            env=gate._child_environment(),
            timeout=0.05,
        )
        self.assertTrue(result.timed_out)
        self.assertFalse(result.output_limited)
        self.assertLess(time.monotonic() - started, 5)


if __name__ == "__main__":
    unittest.main()
