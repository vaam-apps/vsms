# Runbook: SDK Publishing & Release to Public Registries (crates.io & npm)

This runbook documents the release and publishing workflow for the official vsms SDKs:
- **Rust SDK**: `vsms-sdk-rust` on [crates.io](https://crates.io/crates/vsms-sdk-rust)
- **Node.js SDK**: `@vsms/sdk` on [npm](https://www.npmjs.com/package/@vsms/sdk)

---

## 1. Release Architecture & Triggers

SDK publishing is automated via `.github/workflows/release.yml`.

| Trigger | Workflow Behavior | Target |
|---|---|---|
| `git tag v*.*.*` (e.g. `v0.1.0`) | Full Release: builds and publishes container images, Helm charts, crates.io crate, and npm package | Production Registries |
| `push` to `main` | Continuous Integration / Staging images (no SDK publishing) | GHCR (`:main`, `:sha-*`) |
| `workflow_dispatch` | Manual smoke test: performs dry-runs of `cargo publish --dry-run` and `pnpm publish --dry-run` | None (Dry Run) |

---

## 2. Secrets & Registry Authentication

The automated release workflow requires the following repository secrets configured in GitHub repository settings (**Settings > Secrets and variables > Actions**):

### A. crates.io (`vsms-sdk-rust`)
- **Secret Name**: `CRATES_IO_TOKEN`
- **Source**: Generate an API token on [crates.io/settings/tokens](https://crates.io/settings/tokens) with `publish-update` / `publish-new` permissions for `vsms-sdk-rust`.

### B. npm (`@vsms/sdk`)
- **Secret Name**: `NPM_TOKEN`
- **Source**: Generate an Automation access token on [npmjs.com](https://www.npmjs.com/) with publish access to the `@vsms` organization scope.
- **Provenance**: The workflow publishes with `--provenance` via GitHub Actions OpenID Connect (`id-token: write` permission).

---

## 3. Pre-Release Verification (Local Dry-Run)

Before cutting a release tag, run local verification checks:

### 1. Rust SDK (`vsms-sdk-rust`)
```bash
# Ensure vendored schema is up to date
cargo xtask sdk-schema-check

# Run crate checks and dry-run packaging
cd sdks/rust/vsms-sdk-rust
cargo check
cargo test
cargo publish --dry-run
```

### 2. Node.js SDK (`@vsms/sdk`)
```bash
cd sdks/node/vsms-sdk-node
pnpm install
pnpm run build
pnpm run typecheck
node --test
pnpm pack --dry-run
pnpm publish --dry-run --no-git-checks
```

---

## 4. Cutting a Release

To release a new version of the SDKs:

1. **Bump versions** in:
   - `sdks/rust/vsms-sdk-rust/Cargo.toml` (`version = "x.y.z"`)
   - `sdks/node/vsms-sdk-node/package.json` (`"version": "x.y.z"`)
2. **Commit and merge** changes to `main`.
3. **Create and push a git tag**:
   ```bash
   git tag v0.1.0
   git push upstream v0.1.0
   ```
4. **Monitor GitHub Actions**:
   - Check the `release` workflow execution under the Actions tab.
   - Verify `publish-rust-sdk` completes and crate is live on `https://crates.io/crates/vsms-sdk-rust`.
   - Verify `publish-node-sdk` completes and package is live on `https://www.npmjs.com/package/@vsms/sdk`.
