# Releasing

Releases are made by [release-plz](https://release-plz.dev) (`release-plz.toml`,
`.github/workflows/release.yml`), and crates.io trusts the workflow through
[trusted publishing](https://crates.io/docs/trusted-publishing): no token is stored in the
repository.

## An ordinary release

1. Every push to `main` opens or updates a pull request labelled `release`. It bumps the version
   of each crate that changed (cargo-semver-checks against the published version decides between a
   patch and a minor bump) and adds its `CHANGELOG.md` entry. Edit the changelog in that pull
   request if the generated text needs it.
2. A pull request opened with the workflow's own token does not start CI. Start it from the
   Actions tab (CI, "Run workflow", on the release branch) before merging.
3. Merge it. The release workflow publishes the changed crates to crates.io in dependency order,
   tags each `<crate>-v<version>` and makes a GitHub release.

No other push publishes anything.

## One-time setup

Trusted publishing cannot create a crate, so the first version of each crate is published by
hand, once.

1. Sign in to [crates.io](https://crates.io) with the GitHub account and verify the e-mail address
   (crates.io refuses to publish without one).
2. Make an API token with the `publish-new` scope, limited to the crates `printscan*`, with a short
   expiry, and run `cargo login` with it.
3. From a clean checkout of `main` at a commit whose CI passed:

   ```sh
   cargo publish --workspace --locked --dry-run
   cargo publish --workspace --locked
   ```

   Cargo publishes the four crates in dependency order (`printscan`, `printscan-wsd`,
   `printscan-ipp`, `printscan-windows`) and waits for each to appear in the index.
4. Tag the release: `git tag printscan-v0.1.0 && git tag printscan-wsd-v0.1.0 && git tag printscan-ipp-v0.1.0 && git tag printscan-windows-v0.1.0`, then `git push origin --tags`.
5. On crates.io, for each crate: Settings, Trusted Publishing, add a GitHub publisher with owner
   `b1tninja`, repository `printscan`, workflow `release.yml` and environment `release`.
6. Revoke the token from step 2 (and `cargo logout`).
7. On GitHub, in the repository's Settings, Actions, General: allow GitHub Actions to create and
   approve pull requests, so release-plz can open its pull request.

## Crates not released yet

`printscan-sane` is experimental: its `Cargo.toml` says `publish = false` and `release-plz.toml` has
`release = false` for it, so no release pull request bumps, tags or publishes it, and
`cargo publish --workspace` skips it. To release it, remove both, and publish its first version by
hand as in the one-time setup (trusted publishing cannot create a crate), then add its trusted
publisher.

## Yanking

`cargo yank --version <version> <crate>` with a token of the `yank` scope. A yanked version stays
downloadable for lockfiles that already name it; publish a fixed version as well.
