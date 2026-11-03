# Contributing

[Back to the README](README.md)

## Development

Use Rust 1.85 or newer, Git, and Python 3. Run the checks from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
python3 -m unittest discover -s tests -p 'test_install.py' -v
```

## Publishing a release

The `Build and release` GitHub Actions workflow tests and packages all four
platforms on pull requests and pushes to `main`. Pushing a `v*` tag also creates
a GitHub Release containing `install.sh`, one `repvix-<target>.tar.gz` archive per
platform, and a matching `.sha256` file for each archive. All assets are uploaded
to a draft before it is published. Tags with a prerelease suffix are published
as prereleases and are available through `--version`.

To publish a release, update the version in
`Cargo.toml` (for example, to `1.2.0`), run `cargo check` to update `Cargo.lock`,
and commit those files together with the installer and workflow. Then push the
commit and its matching tag:

```sh
git push origin main
git tag -a v1.2.0 -m "Release v1.2.0"
git push origin v1.2.0
```

The tag must exactly match `v` plus the package version in `Cargo.toml`. Wait for
the workflow to finish before using the curl command. Existing published
releases are never overwritten; use a new version when publishing changes.
