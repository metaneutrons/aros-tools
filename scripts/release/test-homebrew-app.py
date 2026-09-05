#!/usr/bin/env python3
"""Offline positive/negative contracts for Homebrew App publication."""

import json
import hashlib
import importlib.util
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
TOKEN = "synthetic-secret-must-not-appear"
SLUG = "metaneutrons-homebrew"
TAP = "metaneutrons/homebrew-tap"

MOCK_GH = """#!/usr/bin/env python3
import json, os, sys
fixture = json.loads(os.environ["GH_FIXTURE"])
key = sys.argv[2]
assert sys.argv[1] == "api" and not any(x in sys.argv for x in ["POST", "PUT", "PATCH", "DELETE"])
if key.startswith("installation/"):
    assert sys.argv[3:] == ["--paginate", "--slurp"]
value = fixture[key]
if value == "API_ERROR":
    print(os.environ["GH_TOKEN"], file=sys.stderr)  # simulated noisy upstream error
    sys.exit(1)
if value == "BAD_JSON":
    print("{")
else:
    print(json.dumps(value))
"""


class AppPreflight(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-test-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        gh = self.work / "gh"
        gh.write_text(MOCK_GH)
        gh.chmod(0o755)
        self.fixture = {
            "installation/repositories?per_page=100": [
                {"total_count": 1, "repositories": [{"full_name": TAP}]}
            ],
            f"repos/{TAP}": {"full_name": TAP, "permissions": {"push": False}},
            f"users/{SLUG}%5Bbot%5D": {"login": f"{SLUG}[bot]", "type": "Bot", "id": 123},
        }
        self.env = dict(os.environ, PATH=f"{self.work}:{os.environ['PATH']}",
                        GH_TOKEN=TOKEN, HOMEBREW_APP_SLUG=SLUG,
                        HOMEBREW_INSTALLATION_ID="123", GITHUB_OUTPUT=str(self.work / "output"))

    def check(self, error=None):
        self.env["GH_FIXTURE"] = json.dumps(self.fixture)
        result = subprocess.run(["bash", str(ROOT / "scripts/release/verify-homebrew-app.sh")],
                                env=self.env, capture_output=True, text=True, timeout=10)
        self.assertNotIn(TOKEN, result.stdout + result.stderr)
        if error:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(error, result.stderr)
            self.assertFalse((self.work / "output").exists())
        else:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((self.work / "output").read_text(),
                             f"bot-name={SLUG}[bot]\nbot-email=123+{SLUG}[bot]@users.noreply.github.com\n")

    def test_valid(self):
        self.check()

    def test_no_token(self):
        self.env["GH_TOKEN"] = ""
        self.check("AP7110")

    def test_wrong_app(self):
        self.env["HOMEBREW_APP_SLUG"] = "different-app"
        self.check("AP7111")

    def test_invalid_installation(self):
        self.env["HOMEBREW_INSTALLATION_ID"] = "0"
        self.check("AP7111")

    def test_pat_rejected_and_error_body_not_logged(self):
        self.fixture["installation/repositories?per_page=100"] = "API_ERROR"
        self.check("AP7112")

    def test_malformed_json(self):
        self.fixture["installation/repositories?per_page=100"] = "BAD_JSON"
        self.check("AP7112")

    def test_missing_page(self):
        self.fixture["installation/repositories?per_page=100"] = []
        self.check("AP7113")

    def test_non_integer_count(self):
        self.fixture["installation/repositories?per_page=100"][0]["total_count"] = True
        self.check("AP7113")

    def test_wrong_repository(self):
        self.fixture["installation/repositories?per_page=100"][0]["repositories"][0]["full_name"] = "other/tap"
        self.check("AP7113")

    def test_additional_repository_on_later_page(self):
        self.fixture["installation/repositories?per_page=100"].append(
            {"total_count": 1, "repositories": [{"full_name": "other/tap"}]})
        self.check("AP7113")

    def test_repository_identity_mismatch(self):
        self.fixture[f"repos/{TAP}"]["full_name"] = "other/tap"
        self.check("AP7114")

    def test_personal_identity(self):
        self.fixture[f"users/{SLUG}%5Bbot%5D"]["type"] = "User"
        self.check("AP7115")

    def test_bad_bot_id(self):
        self.fixture[f"users/{SLUG}%5Bbot%5D"]["id"] = True
        self.check("AP7115")

    def test_output_unwritable(self):
        self.env["GITHUB_OUTPUT"] = str(self.work / "missing" / "output")
        self.check("AP7116")


class WorkflowPolicy(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-homebrew-policy-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        shutil.copytree(ROOT / ".github", self.work / ".github")
        self.action = self.work / ".github/actions/homebrew-token/action.yml"
        self.publish = self.work / ".github/workflows/publish-ecosystem.yml"

    def check(self, marker=None):
        result = subprocess.run(["bash", str(ROOT / "scripts/release/check-actions-policy.sh"), str(self.work)],
                                capture_output=True, text=True, timeout=10)
        if marker:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(marker, result.stderr)
        else:
            self.assertEqual(result.returncode, 0, result.stderr)

    def replace(self, path, old, new):
        source = path.read_text()
        self.assertIn(old, source)
        path.write_text(source.replace(old, new))

    def test_valid(self):
        self.check()

    def test_legacy_pat(self):
        self.replace(self.publish, "secrets.HOMEBREW_APP_PRIVATE_KEY", "secrets.HOMEBREW_TAP_TOKEN")
        self.check("legacy Homebrew PAT")

    def test_admin_write(self):
        self.replace(self.action, "permission-administration: read", "permission-administration: write")
        self.check("unexpected or duplicate permission grant")

    def test_missing_write_permission(self):
        self.replace(self.action, "permission-contents: write", "permission-contents: read")
        self.check("unexpected or duplicate permission grant")

    def test_overbroad_repository_scope(self):
        self.replace(self.action, "repositories: homebrew-tap", "repositories: homebrew-tap,other")
        self.check("credential factory target")

    def test_no_revocation(self):
        self.replace(self.action, "permission-actions: read", "permission-actions: read\n        skip-token-revoke: true")
        self.check("forbidden credential-factory override")

    def test_unpinned_nested_action(self):
        self.replace(self.action, "@bcd2ba49218906704ab6c1aa796996da409d3eb1", "@v3")
        self.check("external action is not pinned")

    def test_no_renewal(self):
        self.replace(self.publish, "id: homebrew-merge-token", "id: obsolete-token")
        self.check("must renew")

    def test_stale_token_for_merge(self):
        self.replace(self.publish, "steps.homebrew-merge-token.outputs.token", "steps.homebrew-token.outputs.token")
        self.check("newly verified App token")

    def test_wait_unbounded(self):
        self.replace(self.publish, "timeout-minutes: 35", "timeout-minutes: 90")
        self.check("must be bounded")

    def test_no_registration_wait(self):
        self.replace(self.publish, "python3 scripts/release/wait-homebrew-checks.py", "true")
        self.check("must be bounded")

    def test_wrong_environment(self):
        self.replace(self.publish, "environment: homebrew-publication", "environment: release")
        self.check("must stay inside homebrew-publication")

    def test_arm_runner_cannot_qualify_intel(self):
        release = self.work / ".github/workflows/release.yml"
        self.replace(release, "runner: macos-15-intel", "runner: macos-14")
        self.check("four genuine native hosts")

    def test_missing_native_identity_proof(self):
        release = self.work / ".github/workflows/release.yml"
        for mode in ("host", "installed"):
            original = release.read_text()
            self.replace(release, f"verify-homebrew-install.py {mode}", "true")
            self.check("Homebrew install qualification omits")
            release.write_text(original)


class CheckRegistration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location("wait_checks", ROOT / "scripts/release/wait-homebrew-checks.py")
        cls.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.module)

    def setUp(self):
        self.head = "a" * 40
        self.data = dict(state="OPEN", isDraft=False, headRefOid=self.head, statusCheckRollup=[])
        self.row = dict(__typename="CheckRun", name="CI success", status="QUEUED", conclusion=None)
        self.now = 0
        self.reads = 0

    def sleep(self, seconds):
        self.now += seconds

    def wait(self, read):
        self.module.wait("123", self.head, {"CI success"}, read=read,
                         clock=lambda: self.now, sleep=self.sleep)

    def test_empty_then_unrelated_then_required(self):
        def read(_):
            self.reads += 1
            if self.reads == 2:
                self.data["statusCheckRollup"] = [dict(self.row, name="lint")]
            if self.reads == 3:
                self.data["statusCheckRollup"].append(self.row)
            return self.data
        self.wait(read)
        self.assertEqual((self.reads, self.now), (3, 40))

    def test_never_registered_is_bounded_failure(self):
        with self.assertRaisesRegex(self.module.CheckError, "AP7314"):
            self.wait(lambda _: self.data)
        self.assertEqual(self.now, 300)

    def test_api_failure_not_retried(self):
        def read(_):
            raise self.module.CheckError("AP7312")
        with self.assertRaisesRegex(self.module.CheckError, "AP7312"):
            self.wait(read)
        self.assertEqual(self.now, 0)

    def test_changed_head_closed_or_draft(self):
        for change in (dict(headRefOid="b" * 40), dict(state="CLOSED"), dict(isDraft=True)):
            with self.subTest(change=change), self.assertRaisesRegex(self.module.CheckError, "AP7310"):
                self.module.registered(dict(self.data, **change), self.head, {"CI success"})

    def test_failures_even_before_required_check_registered(self):
        for result in ("FAILURE", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE"):
            self.data["statusCheckRollup"] = [dict(self.row, name="lint", status="COMPLETED", conclusion=result)]
            with self.subTest(result=result), self.assertRaisesRegex(self.module.CheckError, "AP7313"):
                self.wait(lambda _: self.data)

    def test_malformed_or_unknown_check(self):
        for rows in (None, [None], [dict(self.row, status="invented")], [dict(__typename="Other")]):
            self.data["statusCheckRollup"] = rows
            with self.subTest(rows=rows), self.assertRaisesRegex(self.module.CheckError, "AP7312"):
                self.wait(lambda _: self.data)

    def test_pending_legacy_status_registered(self):
        self.data["statusCheckRollup"] = [dict(__typename="StatusContext", context="CI success", state="PENDING")]
        self.wait(lambda _: self.data)
        self.assertEqual(self.now, 0)


class NativeInstallation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location(
            "verify_brew_install", ROOT / "scripts/release/verify-homebrew-install.py")
        cls.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.module)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="aros-brew-native-")
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        self.prefix = self.work / "installed"
        (self.prefix / "bin").mkdir(parents=True)
        self.manifest_path = self.work / "manifest.json"
        self.target = "x86_64-apple-darwin"
        self.prepare(self.target)

    def prepare(self, target):
        header = bytearray(32)
        arm = target.startswith("aarch64")
        if target.endswith("apple-darwin"):
            header[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<I", header, 4, 0x0100000C if arm else 0x01000007)
            struct.pack_into("<I", header, 12, 2)
        else:
            header[:7] = b"\x7fELF\x02\x01\x01"
            struct.pack_into("<HH", header, 16, 3, 183 if arm else 62)
        self.manifest = dict(schema=1, package="aros-tools", version="0.1.0", target=target,
                             archive=f"aros-tools-v0.1.0-{target}.tar.gz", files=[])
        for name in sorted(self.module.BINARY_NAMES):
            payload = bytes(header) + name.encode()
            path = self.prefix / "bin" / name
            path.write_bytes(payload)
            path.chmod(0o755)
            self.manifest["files"].append(dict(path=f"bin/{name}", size=len(payload),
                                               sha256=hashlib.sha256(payload).hexdigest()))

    def check(self, marker=None, target=None):
        self.manifest_path.write_text(json.dumps(self.manifest))
        if marker:
            with self.assertRaisesRegex(self.module.QualificationError, marker):
                self.module.check_install(self.manifest_path, self.prefix, target or self.target, "0.1.0")
        else:
            self.module.check_install(self.manifest_path, self.prefix, target or self.target, "0.1.0")

    def test_four_native_hosts_and_wrong_mappings(self):
        for target, host in self.module.HOSTS.items():
            with self.subTest(target=target):
                self.module.check_host(target, *host)
                for other in self.module.HOSTS.values():
                    if other != host:
                        with self.assertRaisesRegex(self.module.QualificationError, "AP7320"):
                            self.module.check_host(target, *other)
                with self.assertRaisesRegex(self.module.QualificationError, "AP7320"):
                    self.module.check_host(target, *host, translated=True)

    def test_binary_inventory_tracks_canonical_rust_contract(self):
        source = (ROOT / "crates/aros-release/src/archive.rs").read_text()
        names = re.search(r'pub const BINARIES:.*?= &\[(.*?)\];', source, re.S).group(1)
        self.assertEqual(set(re.findall(r'"([^"]+)"', names)), self.module.BINARY_NAMES)

    def test_four_payload_formats(self):
        for target in self.module.HOSTS:
            with self.subTest(target=target):
                self.prepare(target)
                self.check(target=target)

    def test_wrong_manifest_identity(self):
        for field, value in (("schema", 2), ("target", "aarch64-apple-darwin"),
                             ("version", "0.1.1"), ("archive", "other.tar.gz")):
            with self.subTest(field=field):
                previous = self.manifest[field]
                self.manifest[field] = value
                self.check("AP7321")
                self.manifest[field] = previous

    def test_arm_payload_even_with_matching_digest_is_not_intel(self):
        self.prepare("aarch64-apple-darwin")
        self.manifest.update(target=self.target, archive=f"aros-tools-v0.1.0-{self.target}.tar.gz")
        self.check("not native")

    def test_altered_installed_bytes(self):
        binary = self.prefix / "bin/aros"
        binary.write_bytes(binary.read_bytes()[:-1] + b"!")
        self.check("installed bytes differ")

    def test_symlink_and_nonexecutable_rejected(self):
        binary = self.prefix / "bin/aros"
        binary.chmod(0o644)
        self.check("executable mode")
        binary.rename(self.work / "actual-aros")
        binary.symlink_to(self.work / "actual-aros")
        self.check("file type")

    def test_extra_installed_binary(self):
        (self.prefix / "bin/extra").write_text("not in the release")
        self.check("unexpected inventory")

    def test_duplicate_or_traversing_manifest_entry(self):
        self.manifest["files"].append(self.manifest["files"][0])
        self.check("binary inventory")
        self.manifest["files"].pop()
        self.manifest["files"][0]["path"] = "bin/../../escape"
        self.check("binary inventory")


if __name__ == "__main__":
    unittest.main()
