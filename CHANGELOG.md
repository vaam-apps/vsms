# Changelog

## [0.4.2](https://github.com/vaam-apps/vsms/compare/v0.4.1...v0.4.2) (2026-09-19)


### Bug Fixes

* **ci:** guard fromJSON so a no-commit release-please run does not fail ([f9b94d5](https://github.com/vaam-apps/vsms/commit/f9b94d5c766d728430e209f440456f2e7437f3d0))
* **ci:** scan the default branch on push, not just pull_request ([#414](https://github.com/vaam-apps/vsms/issues/414)) ([f228c6d](https://github.com/vaam-apps/vsms/commit/f228c6dcb1168ba95b6f5170251c6cc71b98a18c))

## [0.4.1](https://github.com/vaam-apps/vsms/compare/v0.4.0...v0.4.1) (2026-09-19)


### Continuous Integration

* pin every action in release-please.yml to a commit SHA ([#411](https://github.com/vaam-apps/vsms/issues/411)) ([7ae773b](https://github.com/vaam-apps/vsms/commit/7ae773b11bda49ffa356594ca76314c6188821aa))

## [0.4.0](https://github.com/vaam-apps/vsms/compare/v0.3.3...v0.4.0) (2026-09-18)


### ⚠ BREAKING CHANGES

* **sdk:** the npm package is now @vaam-apps/vsms-node. Consumers of @vymalo/vsms-node must change their dependency; the old name keeps its existing published versions but receives no further releases.

### Features

* **sdk:** rename the Node SDK to @vaam-apps/vsms-node ([#409](https://github.com/vaam-apps/vsms/issues/409)) ([2113691](https://github.com/vaam-apps/vsms/commit/211369175e7d35b9a808d230c60a23e0cff2f72f))


### Bug Fixes

* **ci:** satisfy DS-0002 where it is real, scope-suppress where it is not ([#410](https://github.com/vaam-apps/vsms/issues/410)) ([2f26db4](https://github.com/vaam-apps/vsms/commit/2f26db469877854ca467fdb04df4b1205f56ee6f))
* **ci:** scope-suppress the SDK test keypair for the secret scanner ([923c935](https://github.com/vaam-apps/vsms/commit/923c9353c864f6ccf158a9805d38482d9e57d2e1))


### Continuous Integration

* adopt org-wide SAST, lint, Trivy and issue governance ([#405](https://github.com/vaam-apps/vsms/issues/405)) ([1e3775b](https://github.com/vaam-apps/vsms/commit/1e3775b8ddef4328df83a87370a988edaa2c1f11))
* keep the org App for SARIF upload, skip deploy/charts in Trivy ([#407](https://github.com/vaam-apps/vsms/issues/407)) ([7def870](https://github.com/vaam-apps/vsms/commit/7def870eb7882a75e87e6545efffe3a92c12d8e2))
* re-pin org reusable workflows for the MD024 changelog fix ([c9ac2db](https://github.com/vaam-apps/vsms/commit/c9ac2dba220534132e1bc75bb5284d01cc1198bd))
* re-pin org reusable workflows for the MD024 changelog fix ([075d5b0](https://github.com/vaam-apps/vsms/commit/075d5b0e5c73f60804d06fd2fdaf43b93334b5f2))

## [0.3.3](https://github.com/vaam-apps/vsms/compare/v0.3.2...v0.3.3) (2026-09-18)


### Bug Fixes

* **ci:** a commit body can make release-please discard the whole commit ([#403](https://github.com/vaam-apps/vsms/issues/403)) ([ec245b3](https://github.com/vaam-apps/vsms/commit/ec245b397938d326b77791ece2fdebdffe7583a5))

## [0.3.2](https://github.com/vaam-apps/vsms/compare/v0.3.1...v0.3.2) (2026-09-17)


### Bug Fixes

* **ci:** release-please uses client-id, not the deprecated app-id ([#399](https://github.com/vaam-apps/vsms/issues/399)) ([0fe5ec0](https://github.com/vaam-apps/vsms/commit/0fe5ec025982605f8d6051c7cc6594048e01b712))
* **security:** --help must never print a secret env var's value ([#363](https://github.com/vaam-apps/vsms/issues/363)) ([#364](https://github.com/vaam-apps/vsms/issues/364)) ([b65b9f0](https://github.com/vaam-apps/vsms/commit/b65b9f08b9b1f9dcb69eed1b2ce10c159097c3a9))
* **security:** bump rustls to 0.23.45 in the three excluded lockfiles ([#397](https://github.com/vaam-apps/vsms/issues/397)) ([735a197](https://github.com/vaam-apps/vsms/commit/735a19735f661ce48f77cad7004e8811a7544021))
* **security:** RUSTSEC-2026-0285 is still live in three excluded manifests ([#398](https://github.com/vaam-apps/vsms/issues/398)) ([24ea417](https://github.com/vaam-apps/vsms/commit/24ea417829849e2a45f5dbe113f814e3e0da907e))
* **security:** RUSTSEC-2026-0285 survived [#392](https://github.com/vaam-apps/vsms/issues/392) in three excluded manifests ([24ea417](https://github.com/vaam-apps/vsms/commit/24ea417829849e2a45f5dbe113f814e3e0da907e))


### Continuous Integration

* release-please proposes the version bump, and a gate that makes it possible ([#395](https://github.com/vaam-apps/vsms/issues/395)) ([8063639](https://github.com/vaam-apps/vsms/commit/806363973195746390f15435280c9123ed1cfccc))
* run each job only when its inputs change, cancel superseded PR runs, serialise releases ([#386](https://github.com/vaam-apps/vsms/issues/386)) ([78b66ba](https://github.com/vaam-apps/vsms/commit/78b66bab23c83d0df0541a5af4651a4d216174a8))
