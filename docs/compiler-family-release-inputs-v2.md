# Compiler-family release inputs v2

This contract collects reviewed recipe, source-lock and profiles documents for
one or more compiler-family groups. It binds their exact bytes in memory and
derives the expected host/profile lanes from every profile in every group.

## Collection shape

The JSON object is closed. A minimal generic collection is:

    {
      "schema": "aros-toolchain-release-inputs-v2",
      "producer_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "tools_commit": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "hosts": ["linux-aarch64", "linux-x86_64", "macos-aarch64"],
      "groups": [{
        "id": "gnu-riscv",
        "recipe": {
          "file": "gnu-recipe.json",
          "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "source_lock": {
          "file": "gnu-source-lock.json",
          "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        },
        "profiles": {
          "file": "gnu-profiles.json",
          "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        }
      }]
    }

The commits and hashes above are placeholders for the field shape. They are
not a validly bound fixture and make no provenance or release claim.

Producer and tools commits are lowercase 40-hex Git object IDs. The hosts
field is exactly the sorted active set shown in the example. There must be
1–32 groups, ordered by unique lower-case kebab IDs. IDs contain only ASCII
lowercase letters, digits and hyphens, start and end with a letter or digit,
and are at most 64 bytes. Each group has three closed document references.

Each file is a unique, lower-case .json basename of at most 128 bytes.
References cannot contain separators, control characters, empty dot segments
or a leading dot. Fixed release inventory names such as the index, checksums,
provenance bundle and published support documents are reserved.

The exact referenced documents are passed in a separate in-memory map keyed by
those basenames. The map must have no missing or extra keys. Each document is
limited to 1 MiB and its declared SHA-256 is checked over the raw bytes.
Collection JSON is also limited to 1 MiB.

## Acceptance boundary

The collection parser directly deserializes closed structures, so duplicate
JSON keys and unknown fields fail. It then parses each recipe, source lock and
profiles document through their existing validators. Every recipe must bind
the collection producer/tools commits; producer and tools trees must agree
across groups. Within a group, the recipe source commit must equal the
profiles upstream commit, source-lock and profiles digests must match the
recipe, and the lock and profiles must select the same compiler family.
Across groups, a repeated source commit must always name the same source tree;
different source commits remain valid.

Every profile must pass compiler-family-v2 identity and recipe-binding checks.
Profile IDs are unique across all groups. The expected matrix is derived from
all those profiles crossed with all three active hosts; it is not inferred
from package listings. Different groups may refer to different AROS source
commits.

Success establishes only that the bounded input documents are internally
consistent and mutually bound. It does not establish a compiler build,
compatibility result, signature, provenance, publication, release readiness or
consumer installation.

## Follow-on scope

The [native executor input-group selector](native-executor-input-groups-v2.md)
allows the same committed producer revision to execute each group's recipe.
It is a separate TOML selection contract, not this published JSON collection.

A versioned release index, evidence ledger and recovery contract remain
separate work. This input boundary does not add release orchestration or
publication behavior. The broader acceptance target is [RV4: Immutable
toolchain releases](riscv-board-integration-plan.md#rv4-immutable-toolchain-releases).
