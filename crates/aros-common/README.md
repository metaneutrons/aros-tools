# `aros-common`

`aros-common` is the dependency leaf for AROS-NG host tools. It owns stable,
reusable contracts that must behave identically across components:

- versioned diagnostics and deterministic human/JSON rendering;
- opt-in local human/JSONL logging;
- fail-closed parsing of `aros-targets.toml`;
- typed SHA-256 parsing and streaming hashing;
- architecture, ELF, source-text, toolchain-lock, and manifest types.

It deliberately contains no command-line workflow policy. Each executable
chooses its own diagnostic codes, hints, stages, and logging schema through a
small component adapter. New shared code belongs here only when at least two
components require identical semantics and can preserve their own error
boundary while using it.

`TargetProfile::load_from_file` and `TargetProfile::load_config` treat their
path as authoritative. Missing files, invalid TOML, and empty target arrays are
errors; bootstrap defaults must never silently replace repository state.
