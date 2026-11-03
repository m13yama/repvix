<p align="center">
  <img src="assets/icon.png" width="128" height="128" alt="repvix icon: repository nodes connected in a V">
</p>

# repvix

Manage multiple Git repositories as one workspace. A small CLI written in Rust,
configured with a single `repvix.yaml`.

[![Build](https://github.com/m13yama/repvix/actions/workflows/release.yml/badge.svg)](https://github.com/m13yama/repvix/actions/workflows/release.yml)
[![Release](https://img.shields.io/github/v/release/m13yama/repvix)](https://github.com/m13yama/repvix/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

- Clone repositories and prepare a different branch for each one.
- Check status, pull changes, and switch branches across your workspace.
- Run a command in every repository, or save it as a reusable task.

[Install](#install) · [Quick start](#quick-start) · [Commands](#commands) · [Usage guide](docs/guide.md)

## Install

**Linux and macOS · x86_64 and ARM64**

```sh
curl -fsSL https://github.com/m13yama/repvix/releases/latest/download/install.sh | bash
```

Installs a prebuilt binary to `~/.local/bin` and verifies its SHA-256 checksum.
Requires Git, Bash, curl, tar, and `sha256sum` or `shasum`. No Rust or `sudo` needed.
Run the same command to upgrade.

If `repvix` is not found, add this to your shell profile:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

To build from a local checkout with Rust 1.85 or newer:

```sh
cargo install --locked --path .
```

See [installation options](docs/guide.md#install) for a specific version or custom directory.

## Quick start

Try a workspace with two public Rust projects, or substitute your own Git URLs:

```sh
mkdir my-workspace
cd my-workspace

repvix init
repvix add https://github.com/sharkdp/bat.git
repvix add https://github.com/sharkdp/fd.git
repvix clone
repvix status
```

This creates `repvix.yaml` and clones into `repos/bat` and `repos/fd`.
Run workspace commands from this directory or any of its subdirectories.

Already have local repositories? Point a new workspace at their parent directory:

```sh
repvix init --base-path /path/to/existing/repos
repvix import --regex '.*'
```

Run this alternative in a directory without an existing `repvix.yaml`.
Import records each repository's origin URL and current branch.

## Commands

| Command | What it does |
| --- | --- |
| `repvix list` | Show configured repositories, paths, branches, and remotes |
| `repvix status` | Show branches, local changes, and ahead/behind counts |
| `repvix clone` | Clone missing repositories and switch to configured branches |
| `repvix sync` | Run `git pull` in each repository; add `--rebase` to rebase |
| `repvix branch feature/login` | Create and check out a branch in every repository |
| `repvix checkout main` | Switch every repository to an existing branch |
| `repvix exec -- cargo test` | Run a command in every repository |
| `repvix remove bat` | Remove a configuration entry, keeping its files |

Save a command for later:

```sh
repvix command add test -- cargo test
repvix run test
```

Use `repvix --help` or `repvix <command> --help` for all options.

## Configuration

`repvix.yaml` stores your repositories, paths, optional branches, and saved commands.

- `workspace.base_path` is the clone root, relative to the config file or absolute.
- `add --path tools` places a repository at `<base_path>/tools/<repository-name>`.
- `add --branch develop` saves the branch that `repvix clone` will prepare.

`clone` can switch existing repositories to their configured branches; it does
not fetch or pull them. Without a configured branch, existing checkouts stay on
their current branch. `branch` and `checkout` do not change the saved configuration.

See the [usage guide](docs/guide.md) for a YAML example, branch behavior,
import options, stored commands, and Bash completion.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development checks and release instructions.

## License

[MIT](LICENSE)
