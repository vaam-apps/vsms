# Changelog

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
