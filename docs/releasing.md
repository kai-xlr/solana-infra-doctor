# Releasing Solana Infra Doctor

This is the maintainer guide for cutting a release. Releases are published to
[crates.io](https://crates.io/crates/solana-infra-doctor) through **Trusted
Publishing** from a manually triggered GitHub Actions workflow.

## Security model

- Releases use **crates.io Trusted Publishing**. The publish workflow
  ([`.github/workflows/publish-crates.yml`](../.github/workflows/publish-crates.yml))
  requests a **short-lived crates.io token via OIDC** at run time using the
  official [`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action).
  The action revokes that token when the job finishes.
- **No permanent crates.io API token is stored in GitHub Secrets.** There is no
  `CRATES_IO_TOKEN` secret; `CARGO_REGISTRY_TOKEN` is populated only from the
  auth action's output, only for the publish step.
- The workflow is **manually triggered** (`workflow_dispatch`) and runs in the
  protected **`release`** GitHub Environment, which should require manual
  approval. It has `permissions: contents: read` and `id-token: write` only — it
  cannot push, create tags, or create GitHub releases.
- **Do not revoke the existing manual crates.io token** (on the maintainer's
  machine) until Trusted Publishing has successfully published at least one
  release — it is the recovery path.
- **Do not enable "Trusted Publishing Only"** until the workflow has proven
  itself with at least one successful release (see the last section).

## One-time setup (after this PR is merged)

### 1. Configure the trusted publisher on crates.io

1. Open the crate settings: `https://crates.io/crates/solana-infra-doctor/settings`.
2. Under **Trusted Publishing**, click **Add**.
3. Select **GitHub Actions**.
4. Enter exactly:
   - **GitHub owner:** `satyakwok`
   - **Repository:** `solana-infra-doctor`
   - **Workflow filename:** `publish-crates.yml`
   - **Environment:** `release`
5. **Save** the trusted publisher.
6. **Do not** enable **Trusted Publishing Only** yet.

> The workflow filename (`publish-crates.yml`) must stay stable after this is
> configured — renaming it breaks the trusted-publisher match.

### 2. Configure the `release` GitHub Environment

1. Open repository **Settings → Environments**.
2. Create or configure the **`release`** environment.
3. Add **required reviewers** (so a human approves each publish), if available.
4. Restrict **deployment branches** to `main`, if available.
5. **Do not** store a crates.io API token in the environment secrets.

## Per-release preparation (a normal PR)

1. Bump the version in `Cargo.toml`.
2. Update `Cargo.lock` if needed (`cargo build`).
3. Add a `CHANGELOG.md` entry for the new version.
4. **Date** the `CHANGELOG.md` entry (`## X.Y.Z - YYYY-MM-DD`, not `Unreleased`).
5. Open a release-preparation PR and merge it to `main`.
6. Confirm CI is green on `main`.
7. Confirm `main` is clean and synced locally.
8. Confirm the version is **not** already on crates.io.
9. Confirm the tag `vX.Y.Z` does **not** already exist.

The workflow re-checks all of the above before it will authenticate, so a
mistake fails safely before any token is requested.

## Publishing through GitHub Actions

1. Open the repository **Actions** tab.
2. Select the **Publish to crates.io** workflow.
3. Click **Run workflow**.
4. Enter the **version without the leading `v`** (e.g. `0.6.0`).
5. Approve the **`release`** environment deployment if prompted.
6. The run executes, in order: input/state validation → quality gates
   (`fmt`, `clippy`, `test`, coverage ≥ 95%, `git diff --check`) → package
   inspection → `cargo publish --dry-run` → OIDC authentication →
   `cargo publish` → post-publish verification on crates.io.

## After a successful publish

`cargo publish` is the only irreversible step. After it succeeds and the
workflow verifies the version on crates.io, do the tag and release **manually**
(the workflow intentionally does not):

```bash
git checkout main && git pull --ff-only origin main
git tag -a vX.Y.Z -m "Solana Infra Doctor vX.Y.Z"
git push origin vX.Y.Z
gh release create vX.Y.Z --title "Solana Infra Doctor vX.Y.Z" --notes "..."
```

Then verify the published artifact installs:

```bash
cargo install solana-infra-doctor --force
sol-doctor --version   # must print X.Y.Z
```

Create the annotated tag only **after** crates.io publish succeeds, and create
the GitHub release only **after** the tag exists.

## Rollback and failure handling

- **crates.io versions cannot be overwritten.** A published version is permanent
  (it can only be *yanked*, which hides it from new resolution but does not
  delete it).
- If the run fails **before** `cargo publish`, fix the issue and re-run — nothing
  was published.
- If `cargo publish` **succeeds but verification fails**, check crates.io
  directly before doing anything. The crate is likely published and just slow to
  index. **Do not blindly re-run** and **do not reuse the version number.**
- **Yank** a version only if there is a real reason (a broken or unsafe release).
- Never expose tokens in logs (do not add `set -x` / shell tracing or echo the
  token).

## Trusted Publishing Only (optional, later)

crates.io can be set to **Trusted Publishing Only**, which disables traditional
API-token publishing entirely.

- This is **optional**.
- Enable it **only after at least one successful Trusted Publishing release** has
  proven the workflow end to end.
- Before enabling it, keep a recovery plan: a way to mint a new API token from
  the crates.io UI in case the OIDC path breaks, since token publishing will be
  turned off.
