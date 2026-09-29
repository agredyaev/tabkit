# Changelog

## [0.1.6](https://github.com/agredyaev/tabkit/compare/tabkit-v0.1.5...tabkit-v0.1.6) (2026-09-29)


### Bug Fixes

* exclude development artifacts from release build ([6db3a78](https://github.com/agredyaev/tabkit/commit/6db3a78896bc7e218ac981cf4626b301d180f33c))


### Performance Improvements

* **core:** reduce lookup and hashing overhead ([9b6e19e](https://github.com/agredyaev/tabkit/commit/9b6e19eb1a40ed565c65079c3f98ca815600a919))
* **edit:** hash candidate during admitted emission ([7c18d7b](https://github.com/agredyaev/tabkit/commit/7c18d7bef36402796c4032fcdb93f35c99acb907))
* **edit:** keep sparse product delta baseline ([5d2bc93](https://github.com/agredyaev/tabkit/commit/5d2bc939496d00721a1681c8172b774db40b0e0b))
* **edit:** prove bounded candidates without full reparse ([be718d8](https://github.com/agredyaev/tabkit/commit/be718d804ad5acb712a25a0e18b0d34d87252f82))
* **edit:** skip unused source validation in product plan ([9275012](https://github.com/agredyaev/tabkit/commit/9275012c665620f5be9d05ee5828fa9b7dd14774))
* **hash:** enable runtime-detected AArch64 SHA-256 acceleration ([c24d41f](https://github.com/agredyaev/tabkit/commit/c24d41f403693603ecc712cf32cc51570c51099c))
* **package:** reuse admitted digest for plain twb ([b790851](https://github.com/agredyaev/tabkit/commit/b7908511d605fc03467855338828fe2aa5b97fe6))
* **plan:** hash reviewed candidate without materializing it ([ba31b68](https://github.com/agredyaev/tabkit/commit/ba31b6886c180b2a01dff3bcd98987e8b9b0a8c9))
* **workbook:** avoid transient validation allocations ([a510c0e](https://github.com/agredyaev/tabkit/commit/a510c0e7c2868a13cb28a7800cf926b92056a993))
* **xml:** add conservative ASCII fast path ([5265089](https://github.com/agredyaev/tabkit/commit/52650894bee64b7eb8b5de6cd9fb148cf7a617f3))
* **xml:** classify ASCII qnames with a static table ([efd14b4](https://github.com/agredyaev/tabkit/commit/efd14b43768554bb639d91d3eef27da17c8b6e7e))
* **xml:** compact retained node records ([df74586](https://github.com/agredyaev/tabkit/commit/df74586beb499dfb8db47b2a287a7cc721c45642))
* **xml:** fast-path column name lookup ([da70370](https://github.com/agredyaev/tabkit/commit/da7037093b59a48ee7c10c7660a48f56e7e0f572))
* **xml:** fast-path dominant qnames ([74dcab1](https://github.com/agredyaev/tabkit/commit/74dcab1a902ed6b35e8c0178f7a6483df2de2de4))
* **xml:** fast-path namespace and repeated tag lookup ([bdce466](https://github.com/agredyaev/tabkit/commit/bdce4667d5a6b961e45f1676de6b4caf0a9e4cfd))
* **xml:** index semantic nodes during streaming admission ([7cfa376](https://github.com/agredyaev/tabkit/commit/7cfa376f98df6c14c0decb0f0a48a1deb01dacd1))
* **xml:** overlap character admission with digest ([5712b64](https://github.com/agredyaev/tabkit/commit/5712b6409fd9c740ec4ec8a27552e5ca6a95f3c4))
* **xml:** overlap digest and streamline hot scans ([12cc116](https://github.com/agredyaev/tabkit/commit/12cc116bd2adb4a7b74b7d009aa100c950cee129))
* **xml:** reduce allocation and index builder memory with measured equivalence ([46d3fe0](https://github.com/agredyaev/tabkit/commit/46d3fe0bfd453e47be64fbd5c2efccce4f470742))
* **xml:** remove redundant validation scans ([f003f85](https://github.com/agredyaev/tabkit/commit/f003f850bbc33058a5ad0919a4c5432ef4a43a94))
* **xml:** resolve only prefixed attributes at tag close ([dce5a7e](https://github.com/agredyaev/tabkit/commit/dce5a7ecc2dbeb8cae4836877a059304cff04148))
* **xml:** reuse repeated normalized attribute values ([74cc1f9](https://github.com/agredyaev/tabkit/commit/74cc1f967a444effa82395a08e296979cbe62afc))
* **xml:** scan fast-path attribute values once ([0614722](https://github.com/agredyaev/tabkit/commit/06147224054043cbd1bab7037e708aba8e04f6b6))
* **xml:** store attributes in SoA columns with borrowed spans ([f808cb9](https://github.com/agredyaev/tabkit/commit/f808cb910f5e8cc789ae0fb94d898015df2355a4))
* **xml:** stream checked input and consume source during planning ([aa44491](https://github.com/agredyaev/tabkit/commit/aa44491a526a92541924995e2d260e7878c85085))
* **xml:** use hash indices in streaming admission ([b1cb6f2](https://github.com/agredyaev/tabkit/commit/b1cb6f279510988e4b8a261801969b62dd03377a))
* **xml:** vectorize fast-path character admission ([0c38133](https://github.com/agredyaev/tabkit/commit/0c38133f4a4dbb8f1ef6abccb9887f20cec86e91))
