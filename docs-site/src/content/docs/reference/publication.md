---
title: Package publication
description: Least-privilege credentials and fail-closed promotion across APT, Homebrew, and AUR.
---

Stable package publication is a gated continuation of the immutable GitHub
release workflow. It never rebuilds a binary. Every downstream package consumes
the exact archives or Debian packages already verified in the staged GitHub
prerelease.

## Promotion sequence

1. Validate all release-environment values and read access before making a
   public change.
2. Create an immutable GitHub draft, download it in isolation, and verify its
   checksums, Sigstore bundles, and GitHub attestations.
3. Expose the unchanged draft as a non-latest prerelease.
4. Publish a signed APT repository to R2, with package objects uploaded before
   the signed metadata, and verify the public bytes and signature.
5. Open a formula PR in `metaneutrons/homebrew-tap`; its protected
   `Formula qualification` gate installs and tests the measured formula on
   Intel and ARM macOS plus x86-64 and ARM64 Linux before merge.
6. Push the measured `PKGBUILD` and freshly generated `.SRCINFO` to
   `aros-tools-bin` in AUR, then verify both Git and AUR RPC state.
7. Only after all channels succeed may a stable tag lose its prerelease marker
   and become the latest GitHub release.

A prerelease tag remains a GitHub prerelease and deliberately does not update
the stable package-manager channels. A failed stable publication remains a
prerelease; reruns are idempotent and never retarget a tag or replace a release
asset.

## GitHub release environment

Create an environment named `release`. Keep the following credentials there,
not in source or repository variables:

Restrict the environment to custom deployment tag policies and allow only
annotated release tags matching `v*`. Pull requests, branches, and manual
qualification runs must never receive publication credentials.

| Secret | Required scope |
| --- | --- |
| `PACKAGE_PUBLISH_TOKEN` | Fine-grained access only to `metaneutrons/homebrew-tap`, with Contents and Pull requests read/write |
| `AUR_SSH_PRIVATE_KEY` | Dedicated unencrypted CI key whose public half is registered in the AUR account |
| `APT_GPG_PRIVATE_KEY` | Dedicated archive-signing private key |
| `APT_GPG_PASSPHRASE` | Passphrase for that signing key |
| `R2_ACCESS_KEY_ID` | R2 S3 credential with Object Read & Write on the single package bucket |
| `R2_SECRET_ACCESS_KEY` | Secret half of the same bucket-scoped credential |

The environment also supplies non-secret variables:

| Variable | Contract |
| --- | --- |
| `AUR_SSH_KNOWN_HOSTS` | Reviewed `aur.archlinux.org` host-key entry; dynamic trust-on-first-use is rejected |
| `APT_GPG_FINGERPRINT` | Full 40-hex primary-key fingerprint |
| `APT_PUBLIC_BASE_URL` | Production HTTPS custom-domain URL ending in `/apt` |
| `R2_ACCOUNT_ID` | Exact 32-hex Cloudflare account ID |
| `R2_BUCKET_NAME` | Dedicated Standard-storage bucket, planned as `aros-packages` |

The R2 bucket and its production custom domain are infrastructure prerequisites;
the release workflow does not create, delete, or reconfigure Cloudflare
resources. R2's S3 endpoint is used only for object access. Uploads never use a
recursive delete: immutable package objects go first, mutable index files next,
and the signed `InRelease` file is the final commit point.

## Failure contract

Preflight emits stable `AP71xx` diagnostics for missing or malformed release
configuration. APT construction and public verification use `AP72xx`, Homebrew
publication uses `AP73xx`, and AUR convergence uses `AP74xx`. Secret values are
never printed. A cross-channel failure blocks stable promotion and reports the
exact channel; it does not claim rollback across independent public services.
