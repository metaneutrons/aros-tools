# Migration provenance

This repository was extracted on 30 August 2026 from
`metaneutrons/AROS-NG`, branch `integration/upstream-20260826`, at source
commit `a74b18a10f`.

The history filter retained and renamed exactly these roots:

| Source | Destination |
| --- | --- |
| `tools/aros-tools/` | repository root |
| `tools/rpi-debug/` | `support/rpi-debug/` |
| `tools/agent-skills/aros-rpi-debug/` | `skills/aros-rpi-debug/` |

Before any post-extraction edit, the result contained 196 files and 193
commits. A sorted source-to-destination manifest compared every Git blob ID and
path with the filtered tree and had no differences.

The only commit-message rewrite removed 89 exactly matched automated-assistant
co-author trailers. No human author name, human author email, committer
identity, other trailer, source byte or unrelated commit-message byte was
deliberately changed. Full history and source scans after filtering found no
remaining occurrence of either removed provider identity.

The filter-repo commit map and both blob manifests are preserved in the
verified AROS-NG migration safety snapshot outside this repository. Subsequent
commits make the extracted workspace standalone and are ordinary reviewed
history rather than part of the attribution rewrite.
