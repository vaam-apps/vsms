# Changelog

## [0.10.0](https://github.com/vaam-apps/vsms/compare/v0.9.0...v0.10.0) (2026-10-31)


### ⚠ BREAKING CHANGES

* **api:** an app key now needs the `optout:read` scope to list opt-outs; without it the list is empty and a detail is 404.
* **auth:** the eleven SMS_SSO_* environment variables are retired. A gateway that sets one its configuration does not reference as `${VAR}` refuses to start; move the settings into `human_login.sso`. A Helm release that still sets the old top-level `sso:` values is refused by the retired-values guard.
* **auth:** [keypair, human_login] deployments are now envelope-Optional and accept a person's bearer, where every unsigned request used to be refused. A password session acts as its non-admin role; owner and admin must pair a console key. bootstrap --owner-password-out yields a password that opens nothing at the API for the owner it creates. The console must ask for `openid profile`. A deactivation or promotion takes effect within the 60-second standing cache. Migration: none; contract digest identical.
* **login:** no per-peer bucket; per_email and aggregate buckets; the cheap checks first (VSA-4a)
* **db:** No release tag contains 0009 yet; the latest is v0.9.0. The next release therefore runs 0009 and 0012 together on every deployment that upgrades from v0.9.0 or earlier. That upgrade deletes the deployment's OAuth clients, the OP's signing keys, its Argon2id password hashes and its in-flight SSO sign-ins. The tables come back empty, and nothing serves them until VSA-3 and VSA-4.
* **config:** vsms reads its deployment configuration from YAML ([#449](https://github.com/vaam-apps/vsms/issues/449))
* **webhooks:** deliver sealed or HMAC, chosen per endpoint ([#447](https://github.com/vaam-apps/vsms/issues/447))
* **auth:** no token endpoint or login exists; machine clients register a public key (register-client), consoles pair a key (pair-console-key), and bootstrap creates the user row only.
* **console:** the OIDC code leg, cookie session, SSO discovery and seven console env vars are deleted, with no compatibility mode.
* **sdk-node:** VsmsClient.privateKeyJwt, PrivateKeyJwtTokenStore, TokenStore, the tokenStore accessor and every token option are removed. Build a VsmsClient with a key and serverKeys. Needs Node 22.11 or newer.
* **auth:** every non-OP route requires a signed request; bearer access tokens are no longer accepted. The Next.js console and the Node SDK cannot reach a live gateway until VS-5b and VS-7 move them to signing.

### Features

* **auth:** accept a machine token beside the envelope (VSA-3b) ([#455](https://github.com/vaam-apps/vsms/issues/455)) ([81cdc0a](https://github.com/vaam-apps/vsms/commit/81cdc0a3ece2bcc68e1b4c0c1b487560d918e4c5))
* **auth:** ClientKey and register-client, an app client's signing public key (VS-2) ([#441](https://github.com/vaam-apps/vsms/issues/441)) ([c15ab28](https://github.com/vaam-apps/vsms/commit/c15ab2817d21e1427714b0d4ff92ff2365d15627))
* **auth:** ConsoleKey, registerConsoleKey and pair-console-key, a human console's signing public key (VS-5a) ([#442](https://github.com/vaam-apps/vsms/issues/442)) ([515bdc2](https://github.com/vaam-apps/vsms/commit/515bdc263dd5c053cc11558c00dd374ac5937947))
* **auth:** seal every route in a signed envelope, and the Rust SDK signs (VS-3, VS-6) ([#443](https://github.com/vaam-apps/vsms/issues/443)) ([8403d4b](https://github.com/vaam-apps/vsms/commit/8403d4bbb23b24a9e81c13908a3ef64338c4824b))
* **auth:** serve machine tokens again behind the op: block (VSA-3a) ([#454](https://github.com/vaam-apps/vsms/issues/454)) ([fb62c4d](https://github.com/vaam-apps/vsms/commit/fb62c4d245adcb01694d9df51b5f61d37d28269d))
* **auth:** single sign-on comes back under human_login.sso (VSA-4c) ([#458](https://github.com/vaam-apps/vsms/issues/458)) ([47cb94b](https://github.com/vaam-apps/vsms/commit/47cb94b5b183b116f80192ec19652b1d29d908f5))
* **auth:** the API accepts a person's token from human_login, never as owner or admin (VSA-4b) ([#457](https://github.com/vaam-apps/vsms/issues/457)) ([56d662d](https://github.com/vaam-apps/vsms/commit/56d662dbfd5610a80c15c6e3ba74830c7d88ac0a))
* **auth:** the password login, the console client row and the human_login gate (VSA-4a) ([e04bda2](https://github.com/vaam-apps/vsms/commit/e04bda296408563542a79089cf9b5b6e277a835c))
* **config:** auth.methods, and a deployment that does not permit hmac_webhooks ([#451](https://github.com/vaam-apps/vsms/issues/451)) ([d41108b](https://github.com/vaam-apps/vsms/commit/d41108b8d03cdbd570bce2574dd73d2cf7301965))
* **config:** the human_login: block, and the rules that serve it (VSA-4a) ([df85a0a](https://github.com/vaam-apps/vsms/commit/df85a0a7d39daa9ed01b38834359741349d69dde))
* **config:** vsms reads its deployment configuration from YAML ([#449](https://github.com/vaam-apps/vsms/issues/449)) ([ca689ab](https://github.com/vaam-apps/vsms/commit/ca689ab5a71b329f64dce1769509ddc97699d879))
* **console:** sign in with a paired browser key, delete the OIDC session ([#445](https://github.com/vaam-apps/vsms/issues/445)) ([1fbc14e](https://github.com/vaam-apps/vsms/commit/1fbc14e940c6dab4c22e11728e59171a9d18c682))
* **core:** Argon2id password hashing and the one-time password generator (VSA-4a) ([7e4467a](https://github.com/vaam-apps/vsms/commit/7e4467ab6b0b93c4735296b106fcc5ddcc93e312))
* **db:** restore the OpenID Provider tables, empty (migration 0012, VSA-2) ([#453](https://github.com/vaam-apps/vsms/issues/453)) ([773240f](https://github.com/vaam-apps/vsms/commit/773240f5b7ffdd39a6fd011f6dc5d3b43c0a4ace))
* **gateway:** POST /login, the grant gate and a provider that serves either method (VSA-4a) ([bae6c3c](https://github.com/vaam-apps/vsms/commit/bae6c3cac7f4297f4a2b6418748e54f2d143023e))
* **gateway:** seed-console-client, and a password for provision-user and bootstrap (VSA-4a) ([f1496fd](https://github.com/vaam-apps/vsms/commit/f1496fd4a70d98504702e50071d06c07d3cf1845))
* **sdk-node:** sign every request through the Rust sealer compiled to WebAssembly (VS-7) ([#444](https://github.com/vaam-apps/vsms/issues/444)) ([88d7d1a](https://github.com/vaam-apps/vsms/commit/88d7d1af5efd4ae45af8501c7e6aeaceb066fb41))
* **webhooks:** deliver sealed or HMAC, chosen per endpoint ([#447](https://github.com/vaam-apps/vsms/issues/447)) ([47b8f1a](https://github.com/vaam-apps/vsms/commit/47b8f1a4d3e730753dcc8d04f796dad726f8a3ad))


### Bug Fixes

* **api:** reading messages needs a permission, not just a signed-in person ([#469](https://github.com/vaam-apps/vsms/issues/469)) ([edae701](https://github.com/vaam-apps/vsms/commit/edae7012ca332d3918129dca59d054ac17fa0e4c))
* **auth:** stamp lastLoginAt after the code, do the same database work on every refusal, no fixed dummy password (VSA-4a) ([5321e2b](https://github.com/vaam-apps/vsms/commit/5321e2b4028af5a6958282bde891e064dd8e51d3))
* clippy 1.99.0 on the review round's code (VSA-4a) ([3efc3ec](https://github.com/vaam-apps/vsms/commit/3efc3ec257d555546fb79acef84bacd83d5adc32))
* **config:** the chart keeps an operator's ConfigMaps, and no error repeats part of a secret ([#450](https://github.com/vaam-apps/vsms/issues/450)) ([49bb45c](https://github.com/vaam-apps/vsms/commit/49bb45cdc5b9f62650edff3e17ba79c984163f27))
* **console-client:** seed-console-client and serve refuse a machine client's row (VSA-4a) ([994a990](https://github.com/vaam-apps/vsms/commit/994a99031e38985b5ed09ca0b6937d0b8a3edaed))
* **deploy:** an edge zone for /login and /token, and a Caddyfile comment that is true again (VSA-4a) ([539ee57](https://github.com/vaam-apps/vsms/commit/539ee57650f5b8032d847fe42cf8b154472c796d))
* **deps:** clear the 2026-10-30 Dependabot alerts by re-resolution ([#452](https://github.com/vaam-apps/vsms/issues/452)) ([eb8418f](https://github.com/vaam-apps/vsms/commit/eb8418f05a89713f08d7ff822b9feaf55832c27f))
* **gateway:** announce an account without sharing a variable with the password file (VSA-4a) ([012e084](https://github.com/vaam-apps/vsms/commit/012e0846282f693a0705d31e271c17590fb29ed7))
* **login:** no per-peer bucket; per_email and aggregate buckets; the cheap checks first (VSA-4a) ([e0959c5](https://github.com/vaam-apps/vsms/commit/e0959c51829eded5e41479f95399121266f0ad6e))
* **token:** key the authorization_code grant on its code, not the public client_id (VSA-4a) ([73ab9e5](https://github.com/vaam-apps/vsms/commit/73ab9e58f6dce73df3dd6e3559dc407e332de20b))


### Performance

* **worker:** a keyless claim costs the hmac work, not the sealed backlog ([#448](https://github.com/vaam-apps/vsms/issues/448)) ([a6ac007](https://github.com/vaam-apps/vsms/commit/a6ac0071a96eb8ac4a0e31fc2181fc5e57773d22))


### Refactoring

* **auth:** delete the embedded OpenID Provider and the human OAuth plane ([#446](https://github.com/vaam-apps/vsms/issues/446)) ([29f93c4](https://github.com/vaam-apps/vsms/commit/29f93c4c873c5b48e6478258042f7c7f6d480e76))
* **gateway:** announce a new account's id without a user_id binding (VSA-4a) ([e191730](https://github.com/vaam-apps/vsms/commit/e1917303091a3f45196dc4ec67c0fb3e9a3225e0))


### Documentation

* **agents:** the review round's five non-obvious findings (VSA-4a) ([5b8e9fa](https://github.com/vaam-apps/vsms/commit/5b8e9fab0dcc1d70d6bc018ba07cb3277a01c3cf))
* align four tables Super-linter's MD060 refused (VSA-4a) ([d20c7d3](https://github.com/vaam-apps/vsms/commit/d20c7d3bfa57cc7e8acb4aca8ae849b49f423c14))
* align the rate-limit table Super-linter's MD060 wants aligned (VSA-4a) ([0c799a0](https://github.com/vaam-apps/vsms/commit/0c799a0c7aa4adc29ba3e8862ea53855db702ace))
* **auth:** the console sign-in in the architecture, the runbook, the example config and AGENTS.md (VSA-4a) ([25099ad](https://github.com/vaam-apps/vsms/commit/25099ad33fe3479d84281f9fbb32145fcdaa78c0))
* the edge diagram names the new zone (VSA-4a) ([add5ec7](https://github.com/vaam-apps/vsms/commit/add5ec772970ad6654f2094e36ba0766aeb41c43))
* the limiter split, the edge zone, and one sentence about what a password may be (VSA-4a) ([7c499c7](https://github.com/vaam-apps/vsms/commit/7c499c799cf34c453995975a584220e99ef5dc4f))


### Tests

* **auth:** compare the statement counts before asserting the count is plausible (VSA-4a) ([74310a5](https://github.com/vaam-apps/vsms/commit/74310a5ea43a11b135acb5ee885da190042e6dfe))
* **auth:** do not spell the driver's path where the R1 scan reads it (VSA-4a) ([a7f09c1](https://github.com/vaam-apps/vsms/commit/a7f09c15d0d83a853032d43a9ea28274cec23bce))
* **auth:** the login suites, the grant gate, serve's boot check and the commands (VSA-4a) ([5f095c5](https://github.com/vaam-apps/vsms/commit/5f095c5adf83c6979b79562a8278ee8c32cf3b9f))
* **gateway:** a throttled sign-in is observably never authenticated (VSA-4a) ([cdc8270](https://github.com/vaam-apps/vsms/commit/cdc8270b36349d6c84cfdebc24f495b33e2c61ce))


### Chores

* **deps:** cratestack 0.16.0 with envelope and cose (VS-1); clear the advisories cargo deny fails on ([#439](https://github.com/vaam-apps/vsms/issues/439)) ([e3e38ff](https://github.com/vaam-apps/vsms/commit/e3e38ffa8d2c0380169dc5df23c82d9654285c7f))

## [0.9.0](https://github.com/vaam-apps/vsms/compare/v0.8.0...v0.9.0) (2026-10-22)


### Features

* **provider:** sms-fake-aws, the AWS SMS provider end to end without an account ([#437](https://github.com/vaam-apps/vsms/issues/437)) ([69afaf0](https://github.com/vaam-apps/vsms/commit/69afaf0b931d12388e1361a74b9da7aba72ca1db))

## [0.8.0](https://github.com/vaam-apps/vsms/compare/v0.7.0...v0.8.0) (2026-10-21)


### ⚠ BREAKING CHANGES

* **admin:** rebuild the console on M3 Expressive and stop the page reloads ([#436](https://github.com/vaam-apps/vsms/issues/436))

### Features

* **admin:** rebuild the console on M3 Expressive and stop the page reloads ([#436](https://github.com/vaam-apps/vsms/issues/436)) ([90c0c70](https://github.com/vaam-apps/vsms/commit/90c0c70525e55636dec39e8e8da7fbff4a95e8db))
* **provider:** AWS End User Messaging SMS as the go-live provider, receipts via SNS ([#435](https://github.com/vaam-apps/vsms/issues/435)) ([f2662c7](https://github.com/vaam-apps/vsms/commit/f2662c7d8806d1c92f0905a688b2baf7121b66ff))


### Bug Fixes

* **rbac:** only an owner may grant, remove or deactivate owner ([#434](https://github.com/vaam-apps/vsms/issues/434)) ([181a087](https://github.com/vaam-apps/vsms/commit/181a08749f9338066cc0b004f2c7156773a48eba))


### Documentation

* add a VitePress docs site with the vaam-apps Material 3 theme ([#431](https://github.com/vaam-apps/vsms/issues/431)) ([7e8a9b4](https://github.com/vaam-apps/vsms/commit/7e8a9b416c4f3204af69acaa5049f42728ebfb9e))

## [0.7.0](https://github.com/vaam-apps/vsms/compare/v0.6.0...v0.7.0) (2026-10-20)


### Features

* **auth:** admin console sign-in through Keycloak (OIDC broker) ([#429](https://github.com/vaam-apps/vsms/issues/429)) ([7dfad66](https://github.com/vaam-apps/vsms/commit/7dfad661a828b63485b7e46bb056576c97e38d50))

## [0.6.0](https://github.com/vaam-apps/vsms/compare/v0.5.0...v0.6.0) (2026-10-19)


### Features

* **release:** publish every image for linux/amd64 and linux/arm64 ([#426](https://github.com/vaam-apps/vsms/issues/426)) ([bcc044e](https://github.com/vaam-apps/vsms/commit/bcc044e8e2bb8672ee583c48501486ffe0556142))


### Bug Fixes

* **ci:** clear the six drifted checks that turned main red ([#427](https://github.com/vaam-apps/vsms/issues/427)) ([8d2a859](https://github.com/vaam-apps/vsms/commit/8d2a8594eb05bbf8dc87dd64f6e8ee9f5a4903bf))


### Chores

* **deps:** bump cratestack 0.11.0 to 0.15.0 ([#424](https://github.com/vaam-apps/vsms/issues/424)) ([63848f7](https://github.com/vaam-apps/vsms/commit/63848f77592f1c35b2a431eec84a2083efa61992))

## [0.5.0](https://github.com/vaam-apps/vsms/compare/v0.4.4...v0.5.0) (2026-09-25)


### Features

* **admin:** move the console to @vaam-apps/ui 0.4.0 ([#420](https://github.com/vaam-apps/vsms/issues/420)) ([74d937f](https://github.com/vaam-apps/vsms/commit/74d937f3e4c12c558c2477a08c4d5fe37e455b96))


### Documentation

* **contributing:** state the docs↔skills parity rule, and ask for companions ([#421](https://github.com/vaam-apps/vsms/issues/421)) ([40bfb83](https://github.com/vaam-apps/vsms/commit/40bfb83ce658739a51e9318a66136b42ae4fed9a))
* **design:** note that long dialogs go full-screen on phones since [#420](https://github.com/vaam-apps/vsms/issues/420) ([#423](https://github.com/vaam-apps/vsms/issues/423)) ([c89ba4a](https://github.com/vaam-apps/vsms/commit/c89ba4a19a6228f86f83126d5c0dd87b73f87dde))

## [0.4.4](https://github.com/vaam-apps/vsms/compare/v0.4.3...v0.4.4) (2026-09-23)


### Chores

* **ci:** bump the vaam-apps/.github workflow pin to pick up the lint fix ([#417](https://github.com/vaam-apps/vsms/issues/417)) ([848e28c](https://github.com/vaam-apps/vsms/commit/848e28cbdf337ee743f68c3b0ea0ab5011138fbd))
* **deps:** bump @vaam-apps/ui 0.2.0 -&gt; 0.2.4 ([#419](https://github.com/vaam-apps/vsms/issues/419)) ([ff2af82](https://github.com/vaam-apps/vsms/commit/ff2af82b97ef840e9a20f72618efcb8d25d03383))

## [0.4.3](https://github.com/vaam-apps/vsms/compare/v0.4.2...v0.4.3) (2026-09-20)


### Bug Fixes

* **test:** give each spawned worker its own metrics port ([#415](https://github.com/vaam-apps/vsms/issues/415)) ([64aa7a7](https://github.com/vaam-apps/vsms/commit/64aa7a7abc0723311339fac1e8e2ee96688fe18e))

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
