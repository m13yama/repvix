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
  - name: api
    url: git@github.com:acme/api.git
    path: services
```

The destinations are `../sources/apps/frontend` and
`../sources/services/api`. Absolute repository `path` values and `..` are
rejected so they cannot escape `base_path`.

There is intentionally no tag feature.

## Install

```sh
cargo install --path .
```

## Quick start

```sh
mkdir my-workspace
cd my-workspace

repvix init --base-path ../sources
repvix add git@github.com:acme/frontend.git --path apps
repvix add git@github.com:acme/api.git --path services
repvix clone
repvix status
```

Every repository entry explicitly contains `name`, `url`, and `path`. When
`--path` is omitted from `add`, repvix writes `path: .`; it does not omit the
field. `name` must match the repository name derived from `url`, preventing a
configuration edit from renaming the cloned directory.

## Import existing repositories

An existing repository below `workspace.base_path` can be imported directly:

```sh
repvix import ./src/existing-repository
```

The command verifies the Git top-level directory, reads the `origin` URL, and
writes all required repository fields. The directory name must match the
repository name in the origin URL. For a repository without an `origin`, pass
the URL explicitly:

```sh
repvix import ./src/existing-repository \
  --url git@github.com:acme/existing-repository.git
```

Version 1 configuration files remain readable. Importing an already configured
local destination replaces its local-path URL with the origin URL and saves the
configuration in the explicit version 2 format.

Available workspace commands:

```text
repvix list
repvix status
repvix branch feature/example
repvix checkout main
repvix sync [--rebase]
repvix exec -- cargo test
```

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
```
