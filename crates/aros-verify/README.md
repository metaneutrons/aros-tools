# `aros-verify`

`aros-verify` compares generated CMake targets with expansion performed by the
historic `tools/genmf/genmf.py` path. It checks coverage and target shape and
returns a non-zero status for missing or incompatible declarations.

The verifier is intentionally independent of `aros-transpiler`. Its MetaMake
condition and architecture logic is a reference oracle, not shared production
implementation; differential fixtures connect the two contracts without
making the same defect authoritative on both sides.

The library owns collection, cache validation, reference expansion,
comparison, and report generation. The binary is a thin argument/exit
boundary. Run `aros-verify --help` for profile, architecture, cache, and report
options.
