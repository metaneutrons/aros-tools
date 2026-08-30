---
title: Pristine upstream AROS
description: What works without carrying AROS-NX patches, and where the compatibility boundary is today.
---

`aros-tools` treats the selected AROS checkout as an explicit input. It does
not infer a sibling directory, require the repository to be named `AROS-NX`,
or expect the Rust workspace inside the operating-system tree.

The independently supported component suites can be exercised without
modifying a pristine checkout:

```sh
cargo test --locked -p aros-collect -p aros-fetch -p aros-genmodule
```

Repository discovery and installed-tool resolution do not require AROS-NX.
Individual parsers, the collector, fetcher and generators model upstream
source contracts directly. The complete workspace gate, however, currently
uses the immutable AROS-NX checkout named by CI because its translation and
verification tests require the consumer bridge and qualified denominators.
The classic MetaMake/GNU Make build stays available in upstream AROS and is not
replaced by a shell fallback.

:::caution[Current build frontend boundary]
The integrated `aros build` CMake path still requires the small consumer bridge
carried by AROS-NX. A native GNU Make backend for an entirely pristine upstream
checkout is not yet a completed release claim. Until its acceptance tests are
green, use upstream's documented configure/MetaMake path for full products and
use `aros-tools` for the independently supported component workflows.
:::
