#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7200 %s\n' "$*" >&2
    exit 1
}

candidate_dir=${1:-}
repository_dir=${2:-}
version=${3:-}
metadata_epoch=${4:-}
renderer=$(cd "$(dirname "$0")" && pwd)/render-apt-metadata.py

[[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || fail 'candidate directory is unsafe'
[[ -d "$repository_dir" && ! -L "$repository_dir" ]] || fail 'repository directory is unsafe'
[[ -f "$renderer" && ! -L "$renderer" ]] || fail 'APT renderer is unsafe'
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'version is malformed'
[[ "$metadata_epoch" =~ ^[1-9][0-9]*$ ]] || fail 'metadata epoch is malformed'

candidate_dir=$(cd "$candidate_dir" && pwd -P)
repository_dir=$(cd "$repository_dir" && pwd -P)

if [[ "${AROS_APT_RENDER_LOCAL_FOR_TESTS:-}" == 1 ]]; then
    [[ "${AROS_RELEASE_POLICY_FIXTURE:-}" == 1 ]] || \
        fail 'local APT renderer escape hatch is restricted to policy fixtures'
    exec python3 "$renderer" --candidate-dir "$candidate_dir" \
        --repository-dir "$repository_dir" --version "$version" \
        --metadata-epoch "$metadata_epoch"
fi

command -v docker >/dev/null || fail 'Docker is required for the digest-pinned APT renderer'
docker run --rm --network none --read-only --tmpfs /tmp:rw,noexec,nosuid,size=16m \
    --memory 256m --memory-swap 256m --pids-limit 64 --cpus 2 \
    --user "$(id -u):$(id -g)" \
    --volume "$renderer:/renderer.py:ro" \
    --volume "$candidate_dir:/candidate:ro" \
    --volume "$repository_dir:/repository:rw" \
    python:3.14.2-slim-bookworm@sha256:e87711ef5c86aaeaa7031718a69db79d334d94c545c709583f651b8185870941 \
    python3 /renderer.py --candidate-dir /candidate \
        --repository-dir /repository --version "$version" \
        --metadata-epoch "$metadata_epoch"
