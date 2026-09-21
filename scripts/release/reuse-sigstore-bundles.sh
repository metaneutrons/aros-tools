#!/usr/bin/env bash

set -euo pipefail

fail() {
    printf '::error::AP7060 %s\n' "$*" >&2
    exit 1
}

candidate_dir=
existing_dir=
identity=
issuer=
allow_new=false
while (($#)); do
    case "$1" in
        --candidate-dir) candidate_dir=${2:-}; shift 2 ;;
        --existing-dir) existing_dir=${2:-}; shift 2 ;;
        --certificate-identity) identity=${2:-}; shift 2 ;;
        --certificate-oidc-issuer) issuer=${2:-}; shift 2 ;;
        --allow-new) allow_new=${2:-}; shift 2 ;;
        *) fail "unknown bundle-recovery argument: $1" ;;
    esac
done

command -v cosign >/dev/null || fail 'cosign is required'
[[ -d "$candidate_dir" && ! -L "$candidate_dir" ]] || \
    fail 'candidate-dir must be a real directory'
[[ -d "$existing_dir" && ! -L "$existing_dir" ]] || \
    fail 'existing-dir must be a real directory'
[[ "$identity" == https://github.com/*/.github/workflows/*.yml@refs/tags/v* ]] || \
    fail 'certificate identity is outside the release workflow/tag namespace'
[[ "$issuer" == 'https://token.actions.githubusercontent.com' ]] || \
    fail 'certificate issuer is not GitHub Actions OIDC'
[[ "$allow_new" == true || "$allow_new" == false ]] || \
    fail 'allow-new must be true or false'
[[ -z $(find "$candidate_dir" "$existing_dir" -mindepth 1 -maxdepth 1 ! -type f -print -quit) ]] || \
    fail 'bundle recovery inventories must contain regular files only'

sign_new_bundle() {
    local subject=$1 candidate_bundle=$2 attempt sign_status

    # OIDC, Fulcio and Rekor are external services. A single transient failure
    # must not discard an otherwise qualified release, but retries must remain
    # bounded and may never let publication continue without a fresh bundle.
    for attempt in 1 2; do
        sign_status=0
        cosign sign-blob --yes --bundle "$candidate_bundle" "$subject" || sign_status=$?
        if ((sign_status == 0)); then
            return 0
        fi
        if ((attempt == 2)); then
            fail "cosign failed twice while signing release subject: $(basename "$subject")"
        fi
        # A failed invocation can leave a partial bundle. It is generated only
        # in the isolated candidate directory and must not affect retry two.
        command rm -f -- "$candidate_bundle"
        printf '%s\n' "::warning::AP7062 cosign signing failed; retrying once after a 20-second backoff: $(basename "$subject")" >&2
        sleep 20
    done
}

for subject in "$candidate_dir"/*; do
    [[ -f "$subject" ]] || continue
    [[ "$subject" != *.sigstore.json ]] || continue
    name=$(basename "$subject")
    existing_subject="$existing_dir/$name"
    existing_bundle="${existing_subject}.sigstore.json"
    candidate_bundle="${subject}.sigstore.json"
    if [[ -f "$existing_subject" && ! -L "$existing_subject" && \
          -f "$existing_bundle" && ! -L "$existing_bundle" ]]; then
        cmp "$subject" "$existing_subject" || \
            fail "reproduced release subject differs from the existing immutable subject: $name"
        cosign verify-blob \
            --bundle "$existing_bundle" \
            --certificate-identity "$identity" \
            --certificate-oidc-issuer "$issuer" \
            "$existing_subject" >/dev/null
        install -m 0644 "$existing_bundle" "$candidate_bundle"
        printf '%s\n' "reused verified keyless bundle for $name"
    elif [[ -f "$existing_subject" && ! -L "$existing_subject" ]]; then
        cmp "$subject" "$existing_subject" || \
            fail "reproduced release subject differs from the existing draft subject: $name"
        [[ "$allow_new" == true ]] || \
            fail "immutable release is missing the required keyless bundle for $name"
        sign_new_bundle "$subject" "$candidate_bundle"
    elif [[ -f "$existing_bundle" && ! -L "$existing_bundle" ]]; then
        cosign verify-blob \
            --bundle "$existing_bundle" \
            --certificate-identity "$identity" \
            --certificate-oidc-issuer "$issuer" \
            "$subject" >/dev/null
        install -m 0644 "$existing_bundle" "$candidate_bundle"
        printf '%s\n' "reused verified draft bundle for $name"
    elif [[ -e "$existing_subject" || -e "$existing_bundle" ]]; then
        fail "existing release has an unsafe subject/bundle entry for $name"
    elif [[ -f "$candidate_bundle" && ! -L "$candidate_bundle" ]]; then
        cosign verify-blob \
            --bundle "$candidate_bundle" \
            --certificate-identity "$identity" \
            --certificate-oidc-issuer "$issuer" \
            "$subject" >/dev/null
        printf '%s\n' "retained verified current-run bundle for $name"
    elif [[ "$allow_new" == true ]]; then
        sign_new_bundle "$subject" "$candidate_bundle"
    else
        fail "immutable release is missing the required keyless bundle for $name"
    fi
done
