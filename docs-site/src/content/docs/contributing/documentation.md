---
title: Writing documentation
description: Keep task guides readable and every capability claim tied to the implementation.
---

The public documentation explains how to use and develop AROS tools.
Write in English, begin with the reader's task, and make examples usable from
a stated working directory.

Keep service-hosting runbooks, credentials, account configuration and operational
procedures out of the public documentation. Public endpoint URLs and consumer
verification instructions belong here; infrastructure administration does not.

## Place content by purpose

| Reader's question | Section |
| --- | --- |
| How do I start? | Get started |
| How do I complete a task? | Build and develop |
| What does this option or file mean? | Reference |
| Why did it fail; is this version available? | Help and releases |
| How do I change the tools? | Contribute |

Preserve published page paths and meaningful anchors when reorganizing content.
Prefer a link to the canonical explanation over another copy of a contract.

## Verify against source

Before documenting a capability, inspect the command model **and** its handler.
An accepted option may be deliberately unsupported, or may choose a directory
without changing the underlying implementation.

Useful source anchors:

- [Frontend commands](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/main.rs)
  and [handlers](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-cli/src/commands.rs).
- [Target defaults](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-common/config/aros-targets.toml)
  and [public environment contract](https://github.com/metaneutrons/aros-tools/blob/main/contracts/public-environment-v1.toml).
- [Board model/transport validation](https://github.com/metaneutrons/aros-tools/blob/main/crates/aros-board/src/config.rs).
- The owning standalone tool's parser, implementation and tests.

Separate **implemented**, **tested against an exact source**, **released** and
**booted on hardware**. Never turn a profile name, zero exit code in report-only
mode, or workflow definition into broader evidence.

Examples that write or remove data must explain the effect beside the command.
Keep dry-run and apply steps separate. Mark illustrative paths visibly.

## Visual style

Astro and Starlight provide the documentation shell, search, table of contents,
mobile navigation, code copying and theme control. The shared AROS palette and
typography live in `src/styles/custom.css`.

Use ordinary Markdown for guides. Use Starlight cards only when a page presents
a useful choice. Maintain comfortable text widths, readable tables and a clear
heading hierarchy. Test dark and light mode; decoration must not obscure content
or navigation.

## Preview and verify

From the tools repository:

```sh
scripts/check-workspace.sh docs
cd docs-site
npm run preview -- --host 127.0.0.1
```

Open the printed local URL with the `/aros-tools/` prefix. Preview the
production build when checking search; its index is generated during the build.

For a style/navigation change, check a narrow mobile viewport, tablet and
desktop. Exercise search, mobile menu, theme selection, code copying, keyboard
focus and a long reference table. Review browser errors and horizontal overflow.

Documentation changes do not require rewriting the application's Rust code.
Run the additional source-related checks only when the change affects a
corresponding contract.
