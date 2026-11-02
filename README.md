# repvix

`repvix` is a small Rust CLI for managing multiple Git repositories as one workspace.

## Path model

`workspace.base_path` controls where repositories are cloned.
It may be absolute or relative to the directory containing `repvix.yaml`.

Each repository may also have a `path`, but that path is always a **parent
directory below `base_path`**. The repository's final directory name is always
derived from its remote URL and cannot be changed.

```text
destination = workspace.base_path / repo.path / repository-name
```

For example:

```yaml
version: 2
workspace:
  name: example
  base_path: ../sources
repos:
  - name: frontend
    url: git@github.com:acme/frontend.git
    path: apps
    branch: develop
  - name: api
    url: git@github.com:acme/api.git
    path: services
    branch: release/next
```

The destinations are `../sources/apps/frontend` and
`../sources/services/api`. Absolute repository `path` values and `..` are
rejected so they cannot escape `base_path`.

There is intentionally no tag feature.

## Install

Install the latest release on Linux or macOS (x86_64 or ARM64):

```sh
curl -fsSL https://github.com/m13yama/repvix/releases/latest/download/install.sh | bash
```

The installer downloads a prebuilt binary, verifies its SHA-256 checksum, and
installs it to `~/.local/bin`. Rust and `sudo` are not required. Linux binaries
are statically linked with musl. You need Bash, curl, tar, and either
`sha256sum` (Linux) or `shasum` (macOS); Git is required to use repvix.

If `~/.local/bin` is not already on your `PATH`, add this to your shell profile:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Run the installer again to upgrade. To select a release or installation directory:

```sh
curl -fsSL https://github.com/m13yama/repvix/releases/latest/download/install.sh \
  | bash -s -- --version v1.2.0 --install-dir "$HOME/bin"
```

`REPVIX_VERSION` and `REPVIX_INSTALL_DIR` can also be passed as environment
variables to `bash`. Use `bash -s -- --help` to see the options. Release assets
must have been published by the workflow below; the original v1.0.0 and v1.1.0
releases do not contain prebuilt binaries or the installer.

To build from a local checkout instead:

```sh
cargo install --locked --path .
```

## Quick start

```sh
mkdir my-workspace
cd my-workspace

repvix init --base-path ../sources
repvix add git@github.com:acme/frontend.git --path apps --branch develop
repvix add git@github.com:acme/api.git --path services --branch release/next
repvix clone
repvix status
```

Every repository entry explicitly contains `name`, `url`, and `path`. When
`--path` is omitted from `add`, repvix writes `path: .`; it does not omit the
field. `name` must match the repository name derived from `url`, preventing a
configuration edit from renaming the cloned directory.

## Prepare repository branches

Each repository can specify its own optional `branch` in `repvix.yaml`.
`add --branch BRANCH` (or `-b BRANCH`) saves it in the configuration. Running
`repvix clone` clones missing repositories on their configured branches and
switches existing repositories to those branches. To change a configured
branch, edit `branch` in the YAML and run `repvix clone` again.

Without `branch`, new clones use the remote's default branch and existing
repositories are left on their current branch. Existing configuration files
without this field continue to work.

Branch switching uses `git switch` without forcing or resetting changes. If
local changes prevent switching, or a branch is unavailable, the command
reports the failure, continues with the other repositories, and exits non-zero.
It does not pull or fetch existing repositories; a newly published remote
branch must first be fetched there. Switching existing repositories requires
Git 2.23 or newer.

`repvix branch NAME` and `repvix checkout BRANCH` remain temporary operations
across all repositories; they do not change the branches saved in the YAML.

## Import existing repositories

An existing repository below `workspace.base_path` can be imported directly:

```sh
repvix import ./src/existing-repository
```

Import multiple repositories by listing all of their paths. Each repository is
processed even if another one fails, and the command prints a result table and
summary before returning a non-zero status when failures occurred:

```sh
repvix import ./src/frontend ./src/api ./src/worker
```

To discover repositories recursively below `workspace.base_path`, use a regular
expression. The expression is matched against paths relative to the base path,
such as `apps/frontend`. Use `.*` to import every discovered repository:

```sh
repvix import --regex '.*'
repvix import --regex '^apps/'
```

The command verifies the Git top-level directory, reads the `origin` URL, and
writes all required repository fields. The directory name must match the
repository name in the origin URL. For a repository without an `origin`, pass
the URL explicitly. `--url` is only available when importing one repository:

```sh
repvix import ./src/existing-repository \
  --url git@github.com:acme/existing-repository.git
```

Import also records each repository's current branch, including when using
multiple paths or `--regex`. Re-importing updates the recorded branch. Use
`--branch` (or `-b`) to record a different branch without switching the local
repository; with multiple paths or a regex, it applies to every imported
repository:

```sh
repvix import ./src/existing-repository --branch release/next
```

A repository with a detached HEAD requires an explicit `--branch` so the saved
configuration can prepare a named branch later.

Version 1 configuration files remain readable. Importing an already configured
local destination replaces its local-path URL with the origin URL and saves the
configuration in the explicit version 2 format.

## Remove repositories from the configuration

`remove` only removes entries from `repvix.yaml`. Working directories and all
files inside them are always kept. Multiple names or destinations can be
specified, or repositories can be selected by their base-path-relative
destination:

```sh
repvix remove api web
repvix remove --regex '^apps/'
```

If any explicit name or destination is unknown, nothing is removed. After a
repository is removed from the configuration, `sync` reports its remaining Git
working directory as unconfigured and skips it.

Available workspace commands:

```text
repvix list
repvix status
repvix remove api
repvix branch feature/example
repvix checkout main
repvix sync [--rebase]
repvix exec -- cargo test
```

`list` prints repository names, workspace-relative destinations, configured
branches (`-` when unspecified), and remotes as an aligned table. `status`
provides one row per repository with its branch,
upstream divergence, working-tree state, change counts, and destination, then
prints a workspace summary. Interactive terminals use color to distinguish
clean, changed, and error states; set `NO_COLOR` or pipe the output to disable
ANSI styling.

Commands are executed directly rather than through a shell. Pipelines and
redirections therefore require an explicit shell such as `sh -c`.

## Stored commands

```sh
repvix command add test --description "Run tests" -- cargo test --all
repvix command list
repvix run test
repvix run test -- --nocapture
repvix command remove test
```

Stored commands live in `repvix.yaml` as a program and an argument array, so
arguments do not need to be reconstructed from a shell command string.

## Bash completion

Enable completion for the current shell:

```sh
source <(repvix completions bash)
```

Install it for the current user:

```sh
mkdir -p ~/.local/share/bash-completion/completions
repvix completions bash \
  > ~/.local/share/bash-completion/completions/repvix
```

The generated script is standalone and does not depend on helper functions
from the `bash-completion` package.

## Development

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
