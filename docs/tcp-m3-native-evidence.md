# TCP-M3 native build evidence

Status: accepted on 2026-09-07. This record is local diagnostic evidence, not
a release, an attestation, an A/B comparison, or hardware boot evidence.

## Fixed inputs

All six executions used the same clean inputs:

- AROS-NX source: `f3cfc243a84065166a46da28b0a5b22bbd0f8869`.
- aros-tools executor: `9a5fa66c7241f6a778b15e3b72db40240e7ea228`.
- aros-toolchains producer: `1b863c9849e0765d4162ee355be55aa852548cd4`.
- Recipe: `2fbf12b7472b420eeb426ff291f2bfcedc8e57e17191bcd9ef4a97e25a376531`.
- Rust: the producer and collector used the explicit `1.98.0` channel.
- Each lane used an isolated work/output root, offline cache, eight jobs and a
  six-hour deadline.

## Results

Every lane completed `preflight`, `environment`, `configure`, `compiler`,
`collector` and no-clobber `publish`; each published prefix then passed
`aros toolchain verify --local` for its profile.

| Host | Profile | `aros-collect` SHA-256 | Bytes | Publish receipt SHA-256 |
| --- | --- | --- | ---: | --- |
| macOS ARM64 | `pc-x86_64` | `05625a64463ffedf5a733ccd32c961fb56dfdee5a4b47fd308f9643077c8eefe` | 921,616 | `f165df66ff215d9b1009e0daec17fb0163db41090775dc4da06ac662da0e9d19` |
| macOS ARM64 | `arm-raspi` | `05625a64463ffedf5a733ccd32c961fb56dfdee5a4b47fd308f9643077c8eefe` | 921,616 | `a7e709a02b59e2fc77a61b940664537790dcf7dc29d21ce1d7b1872672a12955` |
| macOS ARM64 | `rpi-aarch64` | `05625a64463ffedf5a733ccd32c961fb56dfdee5a4b47fd308f9643077c8eefe` | 921,616 | `031c3ad75eb53c080e37f1ec1b3687f4e36b79d5da746d23669a755fb663df72` |
| Linux x86-64 | `pc-x86_64` | `8b63b33fbd40f72c858980e3f07bba47380289a6b11ca25f38723fdf5df4fe9a` | 1,070,080 | `d1a2a9c4020511a3b0856f84b986060eb84e279efa032e31f6da9b422a192cac` |
| Linux x86-64 | `arm-raspi` | `8b63b33fbd40f72c858980e3f07bba47380289a6b11ca25f38723fdf5df4fe9a` | 1,070,080 | `91d9ff97bfcc0c9755b491950701ea75375f5b565a7e78129307b6e1236c9007` |
| Linux x86-64 | `rpi-aarch64` | `8b63b33fbd40f72c858980e3f07bba47380289a6b11ca25f38723fdf5df4fe9a` | 1,070,080 | `6d40844df21925472ba9e505f385f5c0ae2a894df0cf0a0021680f60286cf3c9` |

The differing collector binaries are host-native executables, not a
cross-host reproducibility claim. Detailed logs, resource observations and
retained roots stay outside Git because they contain machine-specific paths.

## Acceptance boundary

TCP-M3 establishes a verified native local-build candidate for all required
host/profile combinations. It does not create a release artifact or qualify
the later archive, compatibility, replay, recovery, CI-cutover, or immutable
release gates.
