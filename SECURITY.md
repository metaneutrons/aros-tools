# Security policy

## Supported versions

Before the first public release, only the current `main` branch receives
security fixes. After release, the newest stable version and `main` are
supported. Older prereleases and development snapshots are not supported.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's
[private vulnerability reporting](https://github.com/metaneutrons/aros-tools/security/advisories/new)
for this repository. Include:

- the affected command, version or commit;
- a minimal reproduction and expected security boundary;
- impact, required privileges and affected platforms;
- whether any destructive operation or published artifact was involved; and
- a safe way to contact you for follow-up.

Do not include real credentials, private keys, personal data or destructive
device identifiers in the report. Use synthetic fixtures whenever possible.

The maintainers will acknowledge a complete report within five working days,
coordinate validation and remediation privately, and publish an advisory after
users have a verified upgrade path. No public timeline is promised before the
scope and fix are understood.

## Security boundaries

High-risk areas include raw-device and board operations, archive extraction,
network fetching, compiler/toolchain selection, generated build graphs,
release credentials, package repositories and provenance. The project treats
silent fallback, partial publication, path traversal and success after a
validation error as security defects.

Release credentials are valid only inside the protected release environment.
They must never be added to source, pull-request workflows, test fixtures,
diagnostic output or local logs.
