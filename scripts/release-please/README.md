# Published release baseline

This adapter prepares the normal App-authored Release Please version PR. It
does not create tags, drafts or releases, dispatch builds, or change admission
and publication permissions.

Release Please 17.6.0 normally looks for a release matching the committed
manifest version. A merged version PR that never produced a release can leave
that version without a published tag. Falling back to the initial bootstrap
then repeats old notes and produces a comparison against a nonexistent release.

The adapter separates two inputs:

- **History:** the highest published stable SemVer release, checked for an
  immutable nonempty release, annotated tag, exact commit, and protected-main
  ancestry. Drafts and prereleases are not published baselines.
- **Version floor:** the unchanged committed root manifest version. A registered
  versioning strategy delegates to the pinned library's default strategy and
  rebases its bump on the floor only if the first result would reuse or regress
  that version. Commit classification and SemVer arithmetic remain in Release
  Please; no `release-as` or fixed target version is used.

The root package's options and Cargo extra-file updaters are preserved. Both
library iterators must actually find the authenticated baseline within their
configured bounds; missing history, failed API reads, ambiguous inventory,
identity mismatches and explicit baseline/version overrides stop preparation.
Metadata selection does not replace release asset qualification.

The dependency is exactly 17.6.0, matching the pinned release-start action's
library. `Version` and `DefaultVersioningStrategy` require internal module
imports in that version. The frozen lock, runtime version assertion and real
library tests make this an explicit API contract. An upgrade must requalify it.
Only one root package and the default versioning policy are supported.

Install and test without credentials:

```sh
pnpm --dir scripts/release-please install --frozen-lockfile --ignore-scripts
pnpm --dir scripts/release-please test
```

For a read-only provider calculation, supply the repository's authorized token
as `GH_TOKEN`, set `GITHUB_REPOSITORY` and `GITHUB_REF=refs/heads/main`, then run:

```sh
node scripts/release-please/published-baseline.mjs --dry-run
```

Normal preparation runs only on protected main with the existing repository-only
Release Please App token. Dependencies and offline tests run before that token
is issued. Only `createPullRequests()` may perform the PR mutation; the later
release-start job remains the separate, admitted release-creation path.
