---
title: Manage source checkouts
description: Clone upstream or a fork, select an exact revision, and synchronize a clean branch.
---

`aros source` works independently of where the tools are installed. Run
initialization from any directory; run synchronization inside the AROS checkout.

## Clone upstream

```sh
aros source init ~/Source/AROS
cd ~/Source/AROS
aros info
```

The default upstream is
`https://github.com/aros-development-team/AROS.git`.
The destination must not already exist. Initialization clones into a sibling
staging directory, validates the layout and recursive submodules, and only
then makes the requested path visible.

## Use your own fork

Replace `YOUR-NAME` with your GitHub account:

```sh
aros source init ~/Source/AROS \
  --fork git@github.com:YOUR-NAME/AROS.git
```

`origin` points at your fork; `upstream` keeps the canonical repository.
Git transport is non-interactive, so configure SSH access before running it.

To make AROS-NX the canonical source instead:

```sh
aros source init ~/Source/AROS-NX \
  --upstream https://github.com/metaneutrons/AROS-NX.git
```

## Select a branch, tag, or commit

For `source init --ref`, use a full `refs/heads/NAME`,
a full `refs/tags/NAME`, or an exact 40/64-digit commit OID.
Short names such as `main` are rejected. Omitting `--ref` uses the clone's
default branch.

Any explicit `--ref`, including a branch ref, leaves the checkout **detached**
at the resolved commit. That is useful for a fixed build input. Before later
synchronization, attach a deliberately named local branch, for example
`git switch -c work/aros`; keep the tree clean. Omit `--ref` for the ordinary
clone-and-sync workflow.

Record the source commit for a repeatable build:

```sh
git rev-parse HEAD
git submodule status --recursive
```

## Synchronize upstream

From a clean upstream checkout on an attached branch:

```sh
aros source sync --ref master
```

For AROS-NX, supply its canonical URL and branch explicitly:

```sh
aros source sync \
  --upstream https://github.com/metaneutrons/AROS-NX.git \
  --ref main
```

Unlike initialization, synchronization expects a branch name such as `main`,
**without** `refs/heads/`. It prefixes that namespace itself and does not
select tags or detached commits. The default is upstream AROS's `master`.

The command fetches one exact candidate, verifies it in an independent
repository, validates the target graphs, and publishes a fast-forward only
if the original branch and tree still match. Divergence requires a deliberate
Git merge/rebase outside this command.

:::caution[Clean means more than git status]
Synchronization also checks ignored files and recursive submodules.
An existing `build/` can therefore block it. Preserve build outputs and
boot evidence before preparing the tree. Do not use a blanket cleanup command
to make this check pass.
:::

`--no-transpile` explicitly skips target-graph validation; the default
performs it. A successful sync validates the candidate graphs, not a complete
product build.

## When synchronization stops

- `AR0113`: another process owns the repository lock. The lock's owner file
  persists after exit; its existence does not mean it is stale.
- `AR0114`: the branch, tree, submodules or local Git semantics are unsuitable.
- `AR0115`: candidate graph validation failed; inspect the named profile.
- `AR0116`: publication failed; inspect `context.commit_state` before retrying.

See [troubleshooting](/aros-tools/reference/troubleshooting/#sync-refused)
for recovery and [the source implementation](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/source.rs)
for the transaction contract.
