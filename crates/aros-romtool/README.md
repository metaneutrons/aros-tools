# `aros-romtool`

`aros-romtool` assembles and validates AROS ROM package layouts used by the
generated build graph. It owns package ordering, alignment, offsets, capacity
checks, and the final image bytes. It does not choose a target profile or
schedule a build.

Input or layout failures are fatal; the tool never publishes a knowingly
partial image. Run `aros-romtool --help` for the current command and package
contract.
