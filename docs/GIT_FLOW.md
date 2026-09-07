# Git Flow

`main` holds final releases. `dev` contains the next version under development.

| Branch | Starts from | Merges into | Purpose |
| --- | --- | --- | --- |
| `codex/feature/<name>` | `dev` | `dev` | One feature, fix, or maintenance change |
| `codex/release/<version>` | `dev` | `main` | Stabilize and prepare a requested release |
| `codex/hotfix/<name>` | `main` | `main` | Fix an urgent issue in a released version |

After a release or hotfix, merge `main` back into `dev` so version updates and fixes reach future work. Use merge commits to preserve feature and release boundaries. Keep the existing published history and version tags intact.

## Everyday development

Start with a clean working tree. Preserve existing work on its own branch before starting another change.

```bash
git fetch origin
git switch dev
git pull --ff-only origin dev
git switch -c codex/feature/<name>
```

Make and commit the change with its relevant tests and documentation. Before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
git push -u origin codex/feature/<name>
```

Open the pull request against `dev`. Wait for CI, then merge with a merge commit. CI checks formatting, Clippy, and tests on Linux, macOS, and Windows. A feature merge does not publish an application release.

## Final releases

Only begin this process when a release has been requested.

1. Create `codex/release/<version>` from current `dev` and make only release preparation and stabilization changes there.
2. Update `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md`; add matching `docs/releases/vX.Y.Z.md` notes.
3. Open a pull request from the release branch to `main`. Wait for CI and merge with a merge commit.
4. Fetch the merged `main` and create an annotated `vX.Y.Z` tag on that merge commit. Push that tag to start packaging and publication.
5. Open a pull request from `main` back to `dev` and merge it so development includes the release changes.

The release workflow accepts only final `vX.Y.Z` tags on the first-parent history of `main`. It rejects prerelease tags and tags on unmerged feature or development commits. Existing final tags can still be rebuilt manually. Publication also requires the tag to match the package version, release notes to exist, and all platform builds and archive checks to pass.

Urgent fixes use `codex/hotfix/<name>` from `main`, including the patch-version bump and release notes, followed by the same release and merge-back process.

## Repository setup

`main` remains the default branch for people downloading the latest released source. Contributors target `dev` for ordinary pull requests. Both long-lived branches require pull requests and passing CI; force pushes and deletion are disabled. Pull requests into `main` must originate from a release or hotfix branch. Required human review is not imposed for this single-maintainer repository.

The local Git Flow configuration uses `main`, `dev`, the branch prefixes above, and `v` for version tags. These settings are optional for contributors using ordinary Git commands; the Git Flow extension is not required.

The initial migration preserves the existing `v0.1.3` release on `main`, records the pending media improvements and cursor-centered zoom in separate feature branches, and integrates them into `dev`. Release automation changes become active on `main` when the next release is merged there.
