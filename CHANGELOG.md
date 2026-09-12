# Changelog

All notable user-visible changes to `aros-tools` are recorded here. Versions
follow [Semantic Versioning](https://semver.org/), and release entries are
maintained by Release Please from Conventional Commit history.

## [0.1.1](https://github.com/metaneutrons/aros-tools/compare/v0.1.0...v0.1.1) (2026-09-12)


### Features

* **apt:** pin signing to one subkey per archive domain ([#16](https://github.com/metaneutrons/aros-tools/issues/16)) ([d08016c](https://github.com/metaneutrons/aros-tools/commit/d08016cae41f7eea62dc59f884fdae26aef04fe5))
* harden aros-tools for the initial public release ([3bde118](https://github.com/metaneutrons/aros-tools/commit/3bde1188161363a8c97d41bbda417bc7b8d858ce))
* make the CMake engine tools-owned ([#24](https://github.com/metaneutrons/aros-tools/issues/24)) ([a7aacea](https://github.com/metaneutrons/aros-tools/commit/a7aacea458ac679132a70cfc8aae7840042b9461))
* **toolchain:** accept TCP-M1 local preview ([2bd8cd4](https://github.com/metaneutrons/aros-tools/commit/2bd8cd41b48d1215da23a5a8ec04b4696dcee63d))
* **toolchain:** add cancellation and work ownership guards ([#46](https://github.com/metaneutrons/aros-tools/issues/46)) ([6a0c951](https://github.com/metaneutrons/aros-tools/commit/6a0c951363db7605ea9765dea5888c0dd4abb315))
* **toolchain:** add deterministic native package writer ([#73](https://github.com/metaneutrons/aros-tools/issues/73)) ([7eac504](https://github.com/metaneutrons/aros-tools/commit/7eac504d4ba6fc3678cdf0e1aaebe83f9a4ae262))
* **toolchain:** add explicit read-only producer inspection ([#45](https://github.com/metaneutrons/aros-tools/issues/45)) ([1e34f7c](https://github.com/metaneutrons/aros-tools/commit/1e34f7c8b25789f3a84b8e59884e9241e150d993))
* **toolchain:** add native local lifecycle ([#60](https://github.com/metaneutrons/aros-tools/issues/60)) ([6f2956b](https://github.com/metaneutrons/aros-tools/commit/6f2956bcbde283732a134de0f1aad3a8cab470e9))
* **toolchain:** add non-executing producer foundation ([8ff86bf](https://github.com/metaneutrons/aros-tools/commit/8ff86bf31462a1d1c1dae70308a6d2f506ca6aa1))
* **toolchain:** audit recursive source material before execution ([#48](https://github.com/metaneutrons/aros-tools/issues/48)) ([9c104bb](https://github.com/metaneutrons/aros-tools/commit/9c104bb4785b07c1c259a08f20112ba39160fd67))
* **toolchain:** bind probes to poisoned environments ([#85](https://github.com/metaneutrons/aros-tools/issues/85)) ([295a999](https://github.com/metaneutrons/aros-tools/commit/295a99917fae4e609da91c6f2bfce45ebe88971b))
* **toolchain:** bind qualification evidence to release indexes ([#80](https://github.com/metaneutrons/aros-tools/issues/80)) ([e68b319](https://github.com/metaneutrons/aros-tools/commit/e68b31944d01b4e24d04c8369d98deb7a2ef9f56))
* **toolchain:** enforce recovery evidence policy ([#91](https://github.com/metaneutrons/aros-tools/issues/91)) ([4765c02](https://github.com/metaneutrons/aros-tools/commit/4765c02f9471299fa31fca64feab994ad8be5841))
* **toolchain:** execute compatibility command batches ([#88](https://github.com/metaneutrons/aros-tools/issues/88)) ([f583b45](https://github.com/metaneutrons/aros-tools/commit/f583b45c9c15b3cdaaf82ae6bc7f6a1738cb6ffa))
* **toolchain:** execute evidence-bound recovery packaging ([#93](https://github.com/metaneutrons/aros-tools/issues/93)) ([3a229c9](https://github.com/metaneutrons/aros-tools/commit/3a229c9ae76849393d9051edfcf298ca1b0d488f))
* **toolchain:** execute native compatibility phases ([#92](https://github.com/metaneutrons/aros-tools/issues/92)) ([edf5133](https://github.com/metaneutrons/aros-tools/commit/edf513358a4acbdf5d78007876fc76a1910c56f0))
* **toolchain:** expose closed Python compatibility environment ([#90](https://github.com/metaneutrons/aros-tools/issues/90)) ([5f44661](https://github.com/metaneutrons/aros-tools/commit/5f44661e174f38f47ca410b58cb9819e3080e47f))
* **toolchain:** expose native producer stages ([#96](https://github.com/metaneutrons/aros-tools/issues/96)) ([674962d](https://github.com/metaneutrons/aros-tools/commit/674962dc391e36343f66786d4eb8e59b6c1010c1))
* **toolchain:** extract verified compatibility roots ([#83](https://github.com/metaneutrons/aros-tools/issues/83)) ([cdf6454](https://github.com/metaneutrons/aros-tools/commit/cdf64543b4b301aa6d77dff1bf5ebf2d78b36106))
* **toolchain:** implement native input contracts ([89245f0](https://github.com/metaneutrons/aros-tools/commit/89245f082f3e55a063d24d28b8b70425c5d2cbca))
* **toolchain:** persist compatibility probe reports ([#82](https://github.com/metaneutrons/aros-tools/issues/82)) ([1d0ccc3](https://github.com/metaneutrons/aros-tools/commit/1d0ccc3a2ae9943cf6e4b63d54a206dae5bed1cd))
* **toolchain:** prepare guarded isolated legacy Git views ([#51](https://github.com/metaneutrons/aros-tools/issues/51)) ([b862b11](https://github.com/metaneutrons/aros-tools/commit/b862b11ff8db367fc918ee595e89ef9cd69e2b70))
* **toolchain:** prepare independent relocation roots ([#84](https://github.com/metaneutrons/aros-tools/issues/84)) ([9a0d2fc](https://github.com/metaneutrons/aros-tools/commit/9a0d2fc9a8476e0447e3d4e99150e7d0597e24b4))
* **toolchain:** prepare isolated committed source snapshots ([#49](https://github.com/metaneutrons/aros-tools/issues/49)) ([90e582b](https://github.com/metaneutrons/aros-tools/commit/90e582b3896ff70d64e600cdbf9cc82d17efe313))
* **toolchain:** prepare tools-owned compatibility inputs ([#81](https://github.com/metaneutrons/aros-tools/issues/81)) ([c5abdec](https://github.com/metaneutrons/aros-tools/commit/c5abdec969ea651b82ec9cf80ff31605478f6b55))
* **toolchain:** require complete compatibility probe sets ([#86](https://github.com/metaneutrons/aros-tools/issues/86)) ([9909ce1](https://github.com/metaneutrons/aros-tools/commit/9909ce1fc6d221f23a3fdcbfd85a6be62c976632))
* **toolchain:** revalidate native collector resume ([#61](https://github.com/metaneutrons/aros-tools/issues/61)) ([9163172](https://github.com/metaneutrons/aros-tools/commit/91631720d2dc7fb699c0cdf8f902ddb8d6c74740))
* **toolchain:** seal compatibility host tools ([#89](https://github.com/metaneutrons/aros-tools/issues/89)) ([5f68e8d](https://github.com/metaneutrons/aros-tools/commit/5f68e8dd5dfd6ff676f9b25669d7a82e763b4171))
* **toolchain:** validate bound compatibility inputs ([#87](https://github.com/metaneutrons/aros-tools/issues/87)) ([9eb52c7](https://github.com/metaneutrons/aros-tools/commit/9eb52c7620a84d4f6f092d7146af217828765035))
* **toolchain:** verify complete v1 release inventory ([#72](https://github.com/metaneutrons/aros-tools/issues/72)) ([0fa0a37](https://github.com/metaneutrons/aros-tools/commit/0fa0a37375afc00aafe2c848f1a2a58896d05512))
* **toolchain:** verify native package readback ([#71](https://github.com/metaneutrons/aros-tools/issues/71)) ([813c89f](https://github.com/metaneutrons/aros-tools/commit/813c89f8fe4d1468856e562c6df7d4b33103d79b))


### Bug Fixes

* accept canonical nested compatibility fetch markers ([253c11a](https://github.com/metaneutrons/aros-tools/commit/253c11a52af6c4eff8d0e3db2ea2d33b740bef79))
* **docs:** publish the canonical path through Cloudflare ([#25](https://github.com/metaneutrons/aros-tools/issues/25)) ([9be605c](https://github.com/metaneutrons/aros-tools/commit/9be605ca980cc673d698001e5ab12b2beaf034f4))
* **engine:** resolve product manifests from the selected tools engine ([#40](https://github.com/metaneutrons/aros-tools/issues/40)) ([e1cee6c](https://github.com/metaneutrons/aros-tools/commit/e1cee6cbadfa0038bd9db761c7fc8ed1c6f3d6df))
* **release:** accept generated release PR title ([876fdcc](https://github.com/metaneutrons/aros-tools/commit/876fdccaf75a7c85ac08e8783d483239d9d8d7e4))
* **release:** delegate signed APT publication to the central archive ([#39](https://github.com/metaneutrons/aros-tools/issues/39)) ([2f23c51](https://github.com/metaneutrons/aros-tools/commit/2f23c512ac3ae876701a12ad907402d5f6d26a63))
* **toolchain:** accept Cargo vendor checksum comments ([#62](https://github.com/metaneutrons/aros-tools/issues/62)) ([46b5a06](https://github.com/metaneutrons/aros-tools/commit/46b5a0658986aa1dd864616e96f20c8d9c913aed))
* **toolchain:** accept CMake-free compatibility sources ([#123](https://github.com/metaneutrons/aros-tools/issues/123)) ([d1d7be1](https://github.com/metaneutrons/aros-tools/commit/d1d7be127425ddb16e6881243aaf28f0090b3e48))
* **toolchain:** accept native lifecycle receipt contract ([#122](https://github.com/metaneutrons/aros-tools/issues/122)) ([654e63e](https://github.com/metaneutrons/aros-tools/commit/654e63ed53cf3127f4425d78330ca006e053b30f))
* **toolchain:** admit env to native compatibility closure ([#105](https://github.com/metaneutrons/aros-tools/issues/105)) ([bc254de](https://github.com/metaneutrons/aros-tools/commit/bc254de9ff13c2beabf43328109f169c4e554038))
* **toolchain:** admit gawk to compatibility closure ([#102](https://github.com/metaneutrons/aros-tools/issues/102)) ([9ac337a](https://github.com/metaneutrons/aros-tools/commit/9ac337a87f349e23667162a7485f62f834a24944))
* **toolchain:** allow repeated locked source use ([#63](https://github.com/metaneutrons/aros-tools/issues/63)) ([8a29fe4](https://github.com/metaneutrons/aros-tools/commit/8a29fe44d86ba036e34ca8cfdb103d3a4e291a72))
* **toolchain:** bind qualification evidence to release matrix ([#121](https://github.com/metaneutrons/aros-tools/issues/121)) ([2402118](https://github.com/metaneutrons/aros-tools/commit/2402118c251b0543b4d140253828e862584a42d7))
* **toolchain:** canonicalize unstable Gitiles source archives ([be764ef](https://github.com/metaneutrons/aros-tools/commit/be764ef58a7c1928a7b783e109773684c2aba9c7))
* **toolchain:** close CMake host compiler resolution ([#100](https://github.com/metaneutrons/aros-tools/issues/100)) ([dbe3fb3](https://github.com/metaneutrons/aros-tools/commit/dbe3fb3d3d1f9338fbc06ebda3767931a84e611c))
* **toolchain:** close compatibility replay host contracts ([#118](https://github.com/metaneutrons/aros-tools/issues/118)) ([13d5305](https://github.com/metaneutrons/aros-tools/commit/13d53059a4a809c1b7d23c8e9e580fbb3858704e))
* **toolchain:** close compatibility source graph ([4143886](https://github.com/metaneutrons/aros-tools/commit/41438862dfdf3ac4f3e02dbc26cc0dfbfd8d4da5))
* **toolchain:** close compatibility source inputs ([ccd2766](https://github.com/metaneutrons/aros-tools/commit/ccd27667542f25f9f9f18f9620e38386f4318ce0))
* **toolchain:** emit package members in lexical order ([#99](https://github.com/metaneutrons/aros-tools/issues/99)) ([a7a1643](https://github.com/metaneutrons/aros-tools/commit/a7a164356cf0cc1ac838be509214c0e651b8815b))
* **toolchain:** preserve upstream compatibility identity ([#120](https://github.com/metaneutrons/aros-tools/issues/120)) ([4e2ea87](https://github.com/metaneutrons/aros-tools/commit/4e2ea87cf31a8fdd6c9841c1eb7086f6d2218e53))
* **toolchain:** publish native candidates atomically ([#64](https://github.com/metaneutrons/aros-tools/issues/64)) ([9a5fa66](https://github.com/metaneutrons/aros-tools/commit/9a5fa66c7241f6a778b15e3b72db40240e7ea228))
* **toolchain:** seal bzip2 in compatibility closure ([#114](https://github.com/metaneutrons/aros-tools/issues/114)) ([3e4feee](https://github.com/metaneutrons/aros-tools/commit/3e4feee9ebab0d2e4a479c6b3d4c8c7a0c893531))
* **toolchain:** seal compatibility ports inputs ([#108](https://github.com/metaneutrons/aros-tools/issues/108)) ([28c79af](https://github.com/metaneutrons/aros-tools/commit/28c79af6a22400b677e64ac804847432d0f870ad))
* **toolchain:** seal complete compatibility host tools ([#101](https://github.com/metaneutrons/aros-tools/issues/101)) ([1e84170](https://github.com/metaneutrons/aros-tools/commit/1e84170e7e1c21285109be69cef11e6c0351d8d9))
* **toolchain:** seal configure host utility closure ([#104](https://github.com/metaneutrons/aros-tools/issues/104)) ([1d81de2](https://github.com/metaneutrons/aros-tools/commit/1d81de2f6c31550456d0c53ebd3fac27568a595e))
* **toolchain:** seal curl in compatibility closure ([#115](https://github.com/metaneutrons/aros-tools/issues/115)) ([5fe7597](https://github.com/metaneutrons/aros-tools/commit/5fe75970a3c2b3391491fba50edd4ffe16a30c73))
* **toolchain:** seal MetaMake autoconf closure ([935adf1](https://github.com/metaneutrons/aros-tools/commit/935adf11a4313110c0177ea8d153030c4b221deb))
* **toolchain:** seal tee in compatibility closure ([#116](https://github.com/metaneutrons/aros-tools/issues/116)) ([0e14ee8](https://github.com/metaneutrons/aros-tools/commit/0e14ee837eccde2b110c5fa561d8bd16f501962b))
* **toolchain:** seal upstream archive decompressors ([07687c7](https://github.com/metaneutrons/aros-tools/commit/07687c75f560d61c7b0abaa45c63a544973bbbe0))
* **toolchain:** seal upstream autotools in compatibility closure ([#103](https://github.com/metaneutrons/aros-tools/issues/103)) ([802acf0](https://github.com/metaneutrons/aros-tools/commit/802acf0d471fa7a9972f970d5726fa2c2cfa8350))
* **toolchain:** support nested upstream fetch markers ([963a9e8](https://github.com/metaneutrons/aros-tools/commit/963a9e8ceebf00c0572329e84df59378e65b57cf))
* **toolchain:** validate collector list symbols exactly ([#117](https://github.com/metaneutrons/aros-tools/issues/117)) ([f3b8868](https://github.com/metaneutrons/aros-tools/commit/f3b886823290e65cfeeea3332041b952f816b8b3))
* unanchor every .gitignore pattern ([#18](https://github.com/metaneutrons/aros-tools/issues/18)) ([e8fd9b9](https://github.com/metaneutrons/aros-tools/commit/e8fd9b98e1f83bb9c5c19fbb4b4a81a00572204f))

## 0.1.0 (2026-09-01)

### Features

* harden aros-tools for the initial public release ([3bde118](https://github.com/metaneutrons/aros-tools/commit/3bde1188161363a8c97d41bbda417bc7b8d858ce))

### Bug Fixes

* **release:** accept generated release PR title ([876fdcc](https://github.com/metaneutrons/aros-tools/commit/876fdccaf75a7c85ac08e8783d483239d9d8d7e4))
