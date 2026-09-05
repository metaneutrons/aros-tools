# Homebrew publication authentication

Maintainer contract for the Homebrew channel. This does not change how users
install aros-tools.

## Trust boundary

The protected `homebrew-publication` environment supplies the non-secret
`HOMEBREW_APP_CLIENT_ID` variable and `HOMEBREW_APP_PRIVATE_KEY` secret.
Its deployment policy permits release tags only. The dedicated App is installed
only on `metaneutrons/homebrew-tap`, not additionally on each source project.
Local custody paths, private keys and recovery instructions are operator data;
they do not belong in the repository.

[The local token action](../.github/actions/homebrew-token/action.yml) is the
single token factory for both preflight and publication. It pins the official
issuer by commit and requests only Contents/Pull requests write and
Actions/Checks/Statuses/Administration read. Metadata read is mandatory.
Administration **read** is necessary for the existing classic branch-protection
check; no Administration write, workflow modification permission or bypass is
granted.

GitHub rejects issuance if the installation cannot grant a requested
permission. The [read-only verifier](../scripts/release/verify-homebrew-app.sh)
then requires one repository across the complete installation-token listing
and the expected App bot identity. The user-role `permissions.push` field is
not used as evidence of an installation grant: it returned false for this App
while the measured grant explicitly included Contents write.
The token action's [permission contract](https://github.com/actions/create-github-app-token#inputs)
is the issuance gate; effective tap protection is verified separately.

No stored PAT fallback exists. The source workflow's `github.token` continues
to authenticate source-release checks, never tap writes.

## Publication sequence

1. Verify the staged, immutable formula; issue and verify a tap-only token.
2. Create or reuse the exact formula branch/PR using the measured App bot
   identity. Preserve version rollback, path, source-release and tap-governance
   guards. This step has a ten-minute bound.
3. Within a 35-minute qualification step, first wait at most five minutes for
   every required check from the governance contract to register on the exact
   PR head. Empty or incomplete inventories are pending, never green. API
   failures, a changed/closed/draft PR and failed checks abort immediately.
   Then watch all registered checks; a timeout stops the job.
4. Issue and verify a **new** token, then recheck CI, exact PR head, source
   identity/assets and effective tap protection. Merge with the exact-head guard
   in a bounded ten-minute step; never bypass protections.
5. Read the formula from protected main with the new token and compare its
   bytes against the qualified asset.

The underlying token action automatically revokes both tokens in post-job
cleanup, including failure. Tokens are not persisted in artifacts, git config
or caches; git authentication uses the ephemeral credential helper. A failed
run may be resumed only through the existing exact-byte roll-forward rules.

## Diagnostics and verification

| Code | Failed guarantee |
| --- | --- |
| AP7110 | Required App token missing |
| AP7111 | Wrong App or invalid installation identity |
| AP7112 | API access, timeout or JSON response failure |
| AP7113 | Token does not identify exactly the intended tap |
| AP7114 | Repository identity mismatch |
| AP7115 | Bot identity mismatch |
| AP7116 | Verified bot outputs could not be recorded |
| AP7030 | Effective protection differs from the governance contract |
| AP7310–AP7311 | PR identity changed or exact-head/check contract missing |
| AP7312 | Check API, response or state is invalid; no retry as empty inventory |
| AP7313 | A check failed while others were still registering |
| AP7314 | Required checks did not register within five minutes |

API error bodies and credential values are not logged by the verifier. A
local successful preflight proves access and protection, not a completed
publication.

```sh
python3 scripts/release/test-homebrew-app.py
bash scripts/release/test-governance-policy.sh
bash scripts/release/test-release-policy.sh
bash scripts/release/check-actions-policy.sh
actionlint
```

Fixtures reject PAT authentication, extra repositories (including later
pages), wrong identity, bad permissions, unpinned actions, retained tokens,
missing renewal and stale-token merges. The normal workspace quality gate
runs these tests. Removal of a legacy stored PAT is a separate, explicitly
approved cleanup after real workflow qualification, not part of token setup.
