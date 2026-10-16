# GRUB host Python selection

The Darwin/ARM64 GRUB host builder prefers
`/opt/homebrew/opt/python@3.14/bin/python3.14`. If that path is absent, it checks
the installed `python3` at `/opt/homebrew/bin/python3`, then `/usr/bin/python3`.
It does not discover an interpreter through the caller's `PATH`, virtual
environment or `Python3_EXECUTABLE` setting. An existing but unusable candidate,
including a dangling link, fails rather than being silently skipped.
An installed preferred path must actually report Python 3.14.x; a differently
versioned interpreter at that path is an error, not a fallback selection.

The selected interpreter must be a regular executable and pass an isolated,
time-limited probe: stable Python 3.11 or newer, with `codecs`, `optparse` and
`re`. This is the supported host-runtime floor, not a statement that upstream
GRUB requires Python 3.11. Both locked GRUB sources declare an older minimum and
use the supplied `PYTHON` value for their normal configure/build generators.
Their optional maintenance, GDB and test-server scripts are separate from this
host-product build and are not qualified by this selection probe.

When a fallback is selected, CMake warns with its absolute path and full Python
version. Each generated lane contract records the logical path, selection kind,
resolved executable path, version and executable SHA-256. The runner independently
selects the expected interpreter and checks the retained path and bytes **before**
executing it again or removing the private build/install trees. A changed
selection, executable, resolution or version requires reconfiguration.

Python is passed explicitly through `PYTHON`; neither the preferred nor fallback
interpreter's directory is added to the closed host-tool `PATH`. Python-specific
inherited search/home/startup settings are cleared for the build, user-site code
is disabled, bytecode writes are disabled and the hash seed remains fixed.
The contract binds the interpreter executable, not the complete Python standard
library or every shared-library dependency; it is not a claim of a hermetic host.

A successful probe or fallback warning is not release qualification. The real
GRUB PC/EFI64/EFI32 product, inventory, hostile-PATH, no-op, repair and refusal
checks remain necessary for the selected interpreter, alongside the repository's
ordinary integration and release gates. No compiler/SDK publication authority or
release gate changes as a consequence of this fallback.

The source-independent `GrubPythonTest.cmake` checks selection and failure
boundaries and runs through both the embedded-engine Rust tests and the complete
CMake gate. The Darwin-only `GrubBuildTest.cmake` additionally checks actual host
products, interpreter contract substitution and a real installed fallback without
modifying any global Python installation. AROS source checkouts remain read-only
test inputs.
