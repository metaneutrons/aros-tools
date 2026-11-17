#!/usr/bin/env python3
"""Fail-closed npm audit gate for the static documentation dependency tree."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import threading
from typing import Any, Mapping


ROOT = Path(__file__).resolve().parents[1]
DOCS_SITE = ROOT / "docs-site"
LOCKFILE = DOCS_SITE / "package-lock.json"
POLICY = ROOT / "contracts/docs-audit-exception-v1.json"
CONFIGURATION_FILES = (
    "docs-site/astro.config.mjs",
    "docs-site/wrangler.jsonc",
    "docs-site/package.json",
    "docs-site/package-lock.json",
)
AUDIT_COMMAND = ("npm", "audit", "--json", "--audit-level=high")
AUDIT_TIMEOUT_SECONDS = 90
MAX_OUTPUT_BYTES = 1_048_576

UTC = dt.timezone.utc
APPROVAL_START = dt.datetime(2026, 11, 17, 0, 0, 0, tzinfo=UTC)
EXCEPTION_EXPIRY = dt.datetime(2026, 12, 1, 22, 0, 0, tzinfo=UTC)
ADVISORY_ID = "GHSA-fc5q-39xc-vjpf"
ADVISORY_URL = f"https://github.com/advisories/{ADVISORY_ID}"
LEAF_PACKAGE = "picomatch"
LEAF_RANGE = "<=4.0.7"
APPROVAL_ID = "approval2026-11-17"
APPROVED_ON = "2026-11-17"
EXPIRY_TEXT = "2026-12-01T22:00:00Z"
RATIONALE = (
    "The affected package is confined to the locked documentation build graph, and the existing Worker contract limits publication to static assets. No fixed picomatch release exists (4.0.7 is the latest and is affected). This temporary exception allows the documentation build to continue until one is published."
)
SCOPE = (
    "Static documentation build and publication only, under the existing static-assets-only Worker contract."
)

# Every package maps to the (version, integrity) pairs that may appear in the
# lock, because the graph carries nested copies of some packages.
LOCKED_PACKAGES = {
    "picomatch": [
        {"version": "2.3.2", "integrity": "sha512-V7+vQEJ06Z+c5tSye8S+nHUfI51xoXIXjHQ99cQtKUkQqqO1kO/KCJUfZXuB47h/YBlDhah2H3hdUGXn8ie0oA=="},
        {"version": "4.0.7", "integrity": "sha512-qcJu88Q2IWqJsDD529JKMdwGm/dvInW4HvQnRwiH9JtihJvzGOscDtHE3x1pBKeUOTysQ8kVmLnJ2kJu7yhcGA=="},
    ],
    "@astrojs/check": [
        {"version": "0.9.12", "integrity": "sha512-hazpqxIfZWj2VhyPyR0pOEQ/IhFDM/2nIApL96zMXABdD8NrUKYDmhxTy7h+drTDl9MQG/Sd1ET/4amadVJUUA=="},
    ],
    "@astrojs/internal-helpers": [
        {"version": "0.11.0", "integrity": "sha512-3rzxJ+xbo0+8YyqOzLziIN32wmsHdCjEVz2sGOpRxJ+Ben/KiLph4ItxBy1abEL+E8fkRzqjg0rfXmaHJGw9JA=="},
        {"version": "0.13.2", "integrity": "sha512-SWiNS+OMrhwWYzJENTekHUyUCMKB+tdU6D4v1ZNIU1ZHh2mScQ8WIs0J6AJ8KPXkIrs9+EZU7Rm2CstfThyVnw=="},
    ],
    "@astrojs/language-server": [
        {"version": "2.17.3", "integrity": "sha512-QIAFu1e/4quJMTPdmBOm+ZPXc5NDGZE4/YQjgR3EgjhGLl/tdidaWJDr5WmO2lesVc+BW1qN/xNBURmuePDG2A=="},
    ],
    "@astrojs/markdown-satteri": [
        {"version": "0.4.1", "integrity": "sha512-EniHbFNa6SQHDih7JnpPos3NWXO6hdt4raBcOosfGmlfc3gP9CI/sESA3VOf1PgJZ7SFfewlZW9Livn0JugEZg=="},
        {"version": "0.4.10", "integrity": "sha512-3h/wheVjZIcP/m295nkiUjOvCsCs6QpvZ3l4BfDU4ghNQcrcFrNVu1+K+I+YT61hH9tuCDQiGdF8QDzfDoG42w=="},
    ],
    "@astrojs/mdx": [
        {"version": "8.0.1", "integrity": "sha512-VKodp/f3+XE6L0950K+ZOAolY47vdt0R78Ao+L2RdJ1hfAdYffL7IEjc/MsoyuS7CPVvCsd5/I3RP0ZyAIewdQ=="},
    ],
    "@astrojs/starlight": [
        {"version": "0.42.8", "integrity": "sha512-BGP8bmTbdky4OFCV9BNWZedLuzONyUtz754q+hie+li/2TQ5XJ6nvfO8fhRhIMcCpUEsA5Ynf4qTTFf/ft/3Pg=="},
    ],
    "@astrojs/vite-plugin-astro": [
        {"version": "0.1.3", "integrity": "sha512-yj+xGnr3HeHUtIdra/tWizJuisBoxAuWlBTElMlf22FpSxinrTwTl/dqftG0D2BsQTcn4/shgCCUwcnkcZ8buA=="},
    ],
    "anymatch": [
        {"version": "3.1.3", "integrity": "sha512-KMReFUr0B4t+D+OBkjR3KYqvocp2XaSzO55UcB6mgQMd3KbcE+mWTyvVV7D/zsdEbNnV6acZUutkiHQXvTr1Rw=="},
    ],
    "astro": [
        {"version": "7.4.12", "integrity": "sha512-DKX+Ds0rqV49HrLwEB9JFrCyfvQeC16FCe1HrPK8S3osiJH/e2vQ22CHIDOpI48UaeiPbxPT26peXgGTy6w0ig=="},
    ],
    "fdir": [
        {"version": "6.5.0", "integrity": "sha512-tIbYtZbucOs0BRGqPJkshJUYdL+SDH7dVM8gjy+ERp3WAUjLEFJE+02kanyHtwjWOnwrKYBiwAmM0p4kLJAnXg=="},
    ],
    "tinyglobby": [
        {"version": "0.2.17", "integrity": "sha512-wXR/dYpcqKmfWpEdZjiKJOwCNFndD0DMnrW/cYjVGttEkBfVgcLFHoNrlj47mjOVic9yyNu65alsgF4NQyTa2g=="},
    ],
    "unstorage": [
        {"version": "1.17.6", "integrity": "sha512-gjKtcmn/KwZuJZO9o2sc0esAQnhmTw0LviDHwevTXbJMYs0dRTp8ZEEWXHU0vTpzbN3cA0yxlTeyXssMp3zDqw=="},
    ],
    "vite": [
        {"version": "8.3.8", "integrity": "sha512-LzOn3h9rG/0vhLdXrV4yJ4QFA9eEoqCl+Yx+kGJd7aB9w6nfNgVHKSYwLPZUHrz4ZGa+t+kXjV9itvcHDLQouA=="},
    ],
}
CONFIGURATION_SHA256 = {
    "docs-site/astro.config.mjs": "61b0aca873b6dc9d18a69f7e4e8ef3a28f99ac27af7e2daaf28395139e26efda",
    "docs-site/wrangler.jsonc": "b594f73a59533f2765b95a8d286bafb8609d21f5d93b17c8aa5e8de5580d00ad",
    "docs-site/package.json": "4e1f4452f864930a73344b3c0038fe4c7d9b3f660cbd3b5691972549d03956c3",
    "docs-site/package-lock.json": "bf13e7728b714a300256b0b94c3e2a74f4c7f5fa151fcd8ada357ffbdadd9fc5",
}
LOCKED_ROOT = {
    "name": "aros-tools-docs",
    "dependencies": {
        "@astrojs/starlight": "0.42.8", "astro": "7.4.12", "sharp": "0.35.5",
    },
    "devDependencies": {
        "@astrojs/check": "0.9.12", "typescript": "6.0.3", "wrangler": "4.156.0",
    },
}
ALLOWED_VIA = {
    "@astrojs/check": ["@astrojs/language-server"],
    "@astrojs/internal-helpers": ["picomatch"],
    "@astrojs/language-server": ["tinyglobby"],
    "@astrojs/markdown-satteri": ["@astrojs/internal-helpers"],
    "@astrojs/mdx": ["@astrojs/internal-helpers", "@astrojs/markdown-satteri", "astro"],
    "@astrojs/starlight": ["@astrojs/markdown-satteri", "@astrojs/mdx", "astro"],
    "@astrojs/vite-plugin-astro": ["@astrojs/internal-helpers", "vite"],
    "anymatch": ["picomatch"],
    "astro": ["@astrojs/internal-helpers", "@astrojs/markdown-satteri", "@astrojs/vite-plugin-astro", "picomatch", "tinyglobby", "unstorage", "vite"],
    "fdir": ["picomatch"],
    "tinyglobby": ["fdir", "picomatch"],
    "unstorage": ["anymatch"],
    "vite": ["picomatch", "tinyglobby"],
}
EXPECTED_POLICY = {
    "schema_version": 1,
    "approval": {
        "id": APPROVAL_ID,
        "approved_on": APPROVED_ON,
        "rationale": RATIONALE,
        "scope": SCOPE,
        "expires_at": EXPIRY_TEXT,
    },
    "exception": {
        "advisory_url": ADVISORY_URL,
        "package": LEAF_PACKAGE,
        "severity": "high",
        "affected_range": LEAF_RANGE,
        "locked": list(LOCKED_PACKAGES[LEAF_PACKAGE]),
    },
    "allowed_ancestors": {
        name: values for name, values in LOCKED_PACKAGES.items() if name != LEAF_PACKAGE
    },
    "allowed_via": ALLOWED_VIA,
    "root_package": LOCKED_ROOT,
    "configuration_sha256": CONFIGURATION_SHA256,
}

SEVERITIES = {"info", "low", "moderate", "high", "critical"}
SEVERITY_RANK = {severity: index for index, severity in enumerate(
    ("info", "low", "moderate", "high", "critical")
)}
PACKAGE_NAME_RE = re.compile(r"(?:@[A-Za-z0-9._~-]+/)?[A-Za-z0-9._~-]+\Z")
SAFE_TEXT_RE = re.compile(r"[^\x00-\x1f\x7f]*\Z")


class AuditValidationError(ValueError):
    """An audit report, lock, policy, or exception binding is not acceptable."""


class DuplicateJsonKeyError(ValueError):
    """Strict JSON decoder error for repeated object members."""


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise DuplicateJsonKeyError("duplicate JSON object member")
        result[key] = value
    return result


def _reject_constant(_value: str) -> None:
    raise ValueError("non-standard JSON constant")


def strict_json_loads(raw: str | bytes) -> Any:
    """Parse JSON while rejecting duplicate fields and non-standard constants."""
    return json.loads(
        raw,
        object_pairs_hook=_unique_object,
        parse_constant=_reject_constant,
    )


def _require(condition: bool, reason: str) -> None:
    if not condition:
        raise AuditValidationError(reason)


def _exact_keys(value: Any, keys: set[str], reason: str) -> dict[str, Any]:
    _require(isinstance(value, dict) and set(value) == keys, reason)
    return value


def validate_policy(policy: Any) -> None:
    """Require the recorded approval to match the single authorized exception."""
    root = _exact_keys(
        policy,
        {"schema_version", "approval", "exception", "allowed_ancestors", "allowed_via", "root_package", "configuration_sha256"},
        "exception policy shape is invalid",
    )
    _require(type(root["schema_version"]) is int, "exception policy schema version is invalid")
    _require(root == EXPECTED_POLICY, "exception policy differs from the approved scope")


def _valid_package_name(name: Any) -> bool:
    return isinstance(name, str) and PACKAGE_NAME_RE.fullmatch(name) is not None


def _valid_text(value: Any, *, maximum: int = 4096) -> bool:
    return (
        isinstance(value, str)
        and 0 < len(value) <= maximum
        and SAFE_TEXT_RE.fullmatch(value) is not None
    )


def _valid_node(node: Any, package: str) -> bool:
    if not isinstance(node, str) or "\\" in node or "\x00" in node:
        return False
    parts = node.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        return False
    suffix = f"node_modules/{package}"
    return node == suffix or node.endswith("/" + suffix)


def _validate_metadata(metadata: Any) -> None:
    metadata = _exact_keys(
        metadata,
        {"vulnerabilities", "dependencies"},
        "npm audit metadata shape is invalid",
    )
    counts = _exact_keys(
        metadata["vulnerabilities"],
        {"info", "low", "moderate", "high", "critical", "total"},
        "npm audit vulnerability totals are invalid",
    )
    _require(
        all(type(counts[level]) is int and counts[level] >= 0 for level in counts),
        "npm audit vulnerability totals are invalid",
    )
    _require(
        counts["total"] == sum(counts[level] for level in SEVERITIES),
        "npm audit vulnerability totals are inconsistent",
    )
    dependencies = _exact_keys(
        metadata["dependencies"],
        {"prod", "dev", "optional", "peer", "peerOptional", "total"},
        "npm audit dependency totals are invalid",
    )
    _require(
        all(type(count) is int and count >= 0 for count in dependencies.values()),
        "npm audit dependency totals are invalid",
    )


def _validate_advisory(item: Any) -> None:
    _require(isinstance(item, dict), "npm audit advisory entry is malformed")
    required = {"source", "name", "dependency", "title", "url", "severity", "range"}
    optional = {"cwe", "cvss"}
    _require(required <= set(item) and set(item) <= required | optional,
             "npm audit advisory entry is malformed")
    _require(type(item["source"]) is int and item["source"] > 0,
             "npm audit advisory source is malformed")
    _require(_valid_package_name(item["name"]) and _valid_package_name(item["dependency"]),
             "npm audit advisory package is malformed")
    _require(_valid_text(item["title"]) and _valid_text(item["url"]),
             "npm audit advisory text is malformed")
    _require(item["url"].startswith("https://"), "npm audit advisory URL is malformed")
    _require(
        isinstance(item["severity"], str) and item["severity"] in SEVERITIES,
        "npm audit advisory severity is malformed",
    )
    _require(_valid_text(item["range"]), "npm audit advisory range is malformed")


def _validate_fix_available(value: Any) -> None:
    if type(value) is bool:
        return
    fix = _exact_keys(
        value,
        {"name", "version", "isSemVerMajor"},
        "npm audit fixAvailable field is malformed",
    )
    _require(_valid_package_name(fix["name"]), "npm audit fixAvailable field is malformed")
    _require(_valid_text(fix["version"], maximum=128), "npm audit fixAvailable field is malformed")
    _require(type(fix["isSemVerMajor"]) is bool, "npm audit fixAvailable field is malformed")


def _validate_vulnerability_records(value: Any) -> dict[str, dict[str, Any]]:
    _require(isinstance(value, dict), "npm audit vulnerabilities field is malformed")
    result: dict[str, dict[str, Any]] = {}
    expected_fields = {
        "name", "severity", "isDirect", "via", "effects", "range", "nodes", "fixAvailable"
    }
    for name, raw_record in value.items():
        _require(_valid_package_name(name), "npm audit package key is malformed")
        record = _exact_keys(raw_record, expected_fields, "npm audit vulnerability record is malformed")
        _require(record["name"] == name, "npm audit package identity is inconsistent")
        _require(
            isinstance(record["severity"], str) and record["severity"] in SEVERITIES,
            "npm audit severity is malformed",
        )
        _require(type(record["isDirect"]) is bool, "npm audit directness field is malformed")
        _require(_valid_text(record["range"]), "npm audit vulnerability range is malformed")
        _require(isinstance(record["via"], list) and len(record["via"]) > 0,
                 "npm audit via chain is malformed")
        for via in record["via"]:
            if isinstance(via, str):
                _require(_valid_package_name(via), "npm audit via package is malformed")
            else:
                _validate_advisory(via)
        _require(isinstance(record["effects"], list), "npm audit effects field is malformed")
        _require(all(_valid_package_name(effect) for effect in record["effects"]),
                 "npm audit effects field is malformed")
        nodes = record["nodes"]
        _require(isinstance(nodes, list) and len(nodes) > 0,
                 "npm audit node list is malformed")
        _require(all(_valid_node(node, name) for node in nodes),
                 "npm audit node list does not match its package")
        _require(len(nodes) == len(set(nodes)), "npm audit node list contains duplicates")
        _validate_fix_available(record["fixAvailable"])
        result[name] = record
    return result


def _validate_all_via_chains(vulnerabilities: dict[str, dict[str, Any]]) -> None:
    """Reject dangling/cyclic chains and severity labels below any via cause."""
    resolved: dict[str, int] = {}

    def visit(name: str, stack: tuple[str, ...]) -> int:
        if name in resolved:
            return resolved[name]
        _require(name not in stack, "npm audit via chain contains a cycle")
        record = vulnerabilities.get(name)
        _require(record is not None, "npm audit via chain contains a dangling package")
        causes: list[int] = []
        for via in record["via"]:
            if isinstance(via, str):
                _require(via in vulnerabilities, "npm audit via chain contains a dangling package")
                causes.append(visit(via, stack + (name,)))
            else:
                causes.append(SEVERITY_RANK[via["severity"]])
        _require(causes, "npm audit via chain is empty")
        highest = max(causes)
        _require(
            SEVERITY_RANK[record["severity"]] == highest,
            "npm audit severity differs from its via causes",
        )
        resolved[name] = highest
        return highest

    for name in vulnerabilities:
        visit(name, ())


def _validate_lock(lock: Any) -> dict[str, Any]:
    _require(isinstance(lock, dict) and isinstance(lock.get("packages"), dict),
             "documentation lockfile is malformed")
    packages = lock["packages"]
    _require(packages.get("") == LOCKED_ROOT,
             "documentation root dependency declarations changed")
    for path, record in packages.items():
        _require(isinstance(path, str) and isinstance(record, dict),
                 "documentation lockfile package entry is malformed")
    for package, expected in LOCKED_PACKAGES.items():
        seen = False
        for path, record in packages.items():
            if path == f"node_modules/{package}" or path.endswith(f"/node_modules/{package}"):
                seen = True
                _require(
                    _matches_locked(record, expected),
                    "locked exception package or ancestor changed",
                )
                edges: set[str] = set()
                for field in ("dependencies", "peerDependencies", "optionalDependencies"):
                    declarations = record.get(field, {})
                    _require(isinstance(declarations, dict),
                             "locked exception dependency declarations are malformed")
                    edges.update(set(declarations) & set(LOCKED_PACKAGES))
                _require(edges == set(ALLOWED_VIA.get(package, [])),
                         "locked exception dependency graph changed")
        _require(seen, "locked exception package or ancestor is missing")
    return packages


def _matches_locked(record: dict[str, Any], expected: list[dict[str, str]]) -> bool:
    return any(
        record.get("version") == variant["version"]
        and record.get("integrity") == variant["integrity"]
        for variant in expected
    )


def _verify_record_nodes(record: dict[str, Any], package: str, packages: dict[str, Any]) -> None:
    expected = LOCKED_PACKAGES[package]
    declared = set(LOCKED_ROOT["dependencies"]) | set(LOCKED_ROOT["devDependencies"])
    _require(record["isDirect"] == (package in declared),
             "audit direct-dependency status differs from the approved root")
    for node in record["nodes"]:
        lock_record = packages.get(node)
        _require(isinstance(lock_record, dict), "audit node is absent from the documentation lockfile")
        _require(
            _matches_locked(lock_record, expected),
            "audit node does not match the approved locked package",
        )


def _validate_leaf(name: str, record: dict[str, Any], packages: dict[str, Any]) -> None:
    _require(name == LEAF_PACKAGE, "high severity finding is outside the approved exception")
    _require(record["severity"] == "high", "approved advisory severity changed")
    _require(len(record["via"]) == 1 and isinstance(record["via"][0], dict),
             "approved package has an additional or malformed advisory")
    advisory = record["via"][0]
    _require(
        advisory["url"] == ADVISORY_URL
        and advisory["name"] == LEAF_PACKAGE
        and advisory["dependency"] == LEAF_PACKAGE
        and advisory["severity"] == "high"
        and advisory["range"] == LEAF_RANGE,
        "finding does not match the approved advisory",
    )
    _verify_record_nodes(record, LEAF_PACKAGE, packages)


def _validate_propagation(
    name: str,
    vulnerabilities: dict[str, dict[str, Any]],
    packages: dict[str, Any],
    stack: tuple[str, ...] = (),
) -> None:
    if name == LEAF_PACKAGE:
        record = vulnerabilities.get(name)
        _require(record is not None, "propagated advisory has a dangling leaf")
        _validate_leaf(name, record, packages)
        return
    _require(name in LOCKED_PACKAGES, "propagated finding is outside the approved dependency chain")
    _require(name not in stack, "npm audit via chain contains a cycle")
    record = vulnerabilities.get(name)
    _require(record is not None, "npm audit via chain contains a dangling package")
    _require(record["severity"] == "high", "propagated finding severity differs from its leaf")
    _verify_record_nodes(record, name, packages)
    _require(all(isinstance(via, str) for via in record["via"]),
             "propagated package has an additional direct advisory")
    _require(len(record["via"]) == len(set(record["via"]))
             and set(record["via"]) == set(ALLOWED_VIA[name]),
             "npm audit propagation differs from the approved locked graph")
    for via in record["via"]:
        _require(isinstance(via, str), "propagated package has an additional direct advisory")
        _require(via in vulnerabilities, "npm audit via chain contains a dangling package")
        _validate_propagation(via, vulnerabilities, packages, stack + (name,))


def _aware_utc(value: dt.datetime) -> dt.datetime:
    _require(isinstance(value, dt.datetime) and value.utcoffset() is not None,
             "exception validation time must be timezone-aware")
    return value.astimezone(UTC)


def validate_report(
    report: Any,
    *,
    exit_code: int,
    lock: Any,
    policy: Any,
    configuration_sha256: Mapping[str, str],
    now: dt.datetime,
) -> bool:
    """Validate npm v2 audit data and the narrow temporary exception.

    `now` is injectable for pure tests. The real command-line interface always
    supplies the current UTC time and offers no date or environment override.
    """
    validate_policy(policy)
    root = _exact_keys(
        report,
        {"auditReportVersion", "vulnerabilities", "metadata"},
        "npm audit report shape is invalid",
    )
    _require(type(root["auditReportVersion"]) is int and root["auditReportVersion"] == 2,
             "npm audit report version is unsupported")
    _validate_metadata(root["metadata"])
    vulnerabilities = _validate_vulnerability_records(root["vulnerabilities"])
    _validate_all_via_chains(vulnerabilities)

    counts = root["metadata"]["vulnerabilities"]
    severities = [record["severity"] for record in vulnerabilities.values()]
    record_counts = {
        severity: severities.count(severity)
        for severity in SEVERITIES
    }
    _require(
        all(counts[severity] == record_counts[severity] for severity in SEVERITIES),
        "npm audit severity totals do not match its findings",
    )
    _require(counts["total"] == len(vulnerabilities),
             "npm audit total does not match its findings")
    has_high_or_critical = any(severity in {"high", "critical"} for severity in severities)
    _require(type(exit_code) is int and exit_code in {0, 1},
             "npm audit returned an unexpected status")
    _require(exit_code == (1 if has_high_or_critical else 0),
             "npm audit status does not match its report")

    _require(not any(severity == "critical" for severity in severities),
             "critical severity findings are never exempt")
    high_names = [name for name, record in vulnerabilities.items() if record["severity"] == "high"]
    if not high_names:
        return False

    packages = _validate_lock(lock)
    current = _aware_utc(now)
    _require(APPROVAL_START <= current < EXCEPTION_EXPIRY,
             "temporary documentation audit exception is outside its approval window")
    _require(
        dict(configuration_sha256) == CONFIGURATION_SHA256,
        "documentation runtime configuration differs from the approved static scope",
    )

    _require(LEAF_PACKAGE in vulnerabilities, "approved advisory leaf is missing")
    _require(set(high_names) == set(LOCKED_PACKAGES),
             "high severity findings differ from the approved complete graph")
    _validate_leaf(LEAF_PACKAGE, vulnerabilities[LEAF_PACKAGE], packages)
    for name in high_names:
        if name == LEAF_PACKAGE:
            continue
        _require(name in LOCKED_PACKAGES, "high severity finding is outside the approved exception")
        _validate_propagation(name, vulnerabilities, packages)
    return True


def _read_json_file(path: Path) -> Any:
    try:
        return strict_json_loads(path.read_bytes())
    except (OSError, UnicodeError, json.JSONDecodeError, DuplicateJsonKeyError, ValueError) as error:
        raise AuditValidationError("required JSON input is unreadable or malformed") from error


def _hash_configurations() -> dict[str, str]:
    hashes: dict[str, str] = {}
    for relative in CONFIGURATION_FILES:
        try:
            data = (ROOT / relative).read_bytes()
        except OSError as error:
            raise AuditValidationError("documentation runtime configuration is unreadable") from error
        hashes[relative] = hashlib.sha256(data).hexdigest()
    return hashes


def _run_audit() -> tuple[int, bytes, bytes]:
    try:
        process = subprocess.Popen(
            AUDIT_COMMAND,
            cwd=DOCS_SITE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        raise AuditValidationError("npm audit could not be started") from error

    buffers = {"stdout": bytearray(), "stderr": bytearray()}
    overflow = {"stdout": False, "stderr": False}

    def drain(stream: Any, key: str) -> None:
        while True:
            chunk = stream.read(65536)
            if not chunk:
                return
            room = MAX_OUTPUT_BYTES - len(buffers[key])
            if room > 0:
                buffers[key].extend(chunk[:room])
            if len(chunk) > room:
                overflow[key] = True
                try:
                    process.kill()
                except ProcessLookupError:
                    # The child may have exited between the read and kill.
                    pass
                return

    assert process.stdout is not None and process.stderr is not None
    readers = [
        threading.Thread(target=drain, args=(process.stdout, "stdout"), daemon=True),
        threading.Thread(target=drain, args=(process.stderr, "stderr"), daemon=True),
    ]
    for reader in readers:
        reader.start()
    try:
        status = process.wait(timeout=AUDIT_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        process.kill()
        process.wait()
        for reader in readers:
            reader.join(timeout=2)
        raise AuditValidationError("npm audit exceeded its time limit") from error
    for reader in readers:
        reader.join(timeout=2)
    if any(reader.is_alive() for reader in readers):
        raise AuditValidationError("npm audit output transport did not close")
    if overflow["stdout"] or overflow["stderr"]:
        raise AuditValidationError("npm audit output exceeded its size limit")
    return status, bytes(buffers["stdout"]), bytes(buffers["stderr"])


def _parse_report(raw: bytes) -> Any:
    try:
        decoded = raw.decode("utf-8", errors="strict")
        return strict_json_loads(decoded)
    except (UnicodeError, json.JSONDecodeError, DuplicateJsonKeyError, ValueError, RecursionError) as error:
        raise AuditValidationError("npm audit returned malformed JSON") from error


def main() -> int:
    has_unexpected_arguments = len(sys.argv) != 1
    try:
        # Always perform the real audit before examining any policy or report.
        status, raw_report, raw_stderr = _run_audit()
        if has_unexpected_arguments:
            raise AuditValidationError("this checker accepts no command-line options")
        if raw_stderr:
            raise AuditValidationError("npm audit wrote diagnostics to stderr")
        report = _parse_report(raw_report)
        policy = _read_json_file(POLICY)
        lock = _read_json_file(LOCKFILE)
        exception_used = validate_report(
            report,
            exit_code=status,
            lock=lock,
            policy=policy,
            configuration_sha256=_hash_configurations(),
            now=dt.datetime.now(UTC),
        )
    except AuditValidationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except (TypeError, KeyError, AttributeError, RecursionError):
        print("error: npm audit data is malformed", file=sys.stderr)
        return 1
    if exception_used:
        print(
            f"documentation npm audit passed with approved "
            f"{ADVISORY_ID} exception through {EXPIRY_TEXT}"
        )
    elif report["vulnerabilities"]:
        print("documentation npm audit passed below the high severity threshold")
    else:
        print("documentation npm audit clean")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
