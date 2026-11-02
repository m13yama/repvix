#!/usr/bin/env bash
# Install a prebuilt repvix binary from GitHub Releases. Requires Bash 3.2+.

usage() {
    cat <<'EOF'
Usage: install.sh [--version VERSION] [--install-dir DIRECTORY]

  --version VERSION        Release version, with or without v (default: latest)
  --install-dir DIRECTORY   Installation directory (default: ~/.local/bin)
  -h, --help                Show this help

Environment: REPVIX_VERSION and REPVIX_INSTALL_DIR set the same defaults.
EOF
}

die() {
    printf 'repvix installer: %s\n' "$*" >&2
    exit 1
}

download() {
    curl --fail --silent --show-error --location --proto '=https' \
        --proto-redir '=https' --tlsv1.2 --retry 3 --connect-timeout 10 \
        --max-time 120 "$@"
}

# Keep execution at the end so a partially downloaded script cannot install.
main() (
    set -euo pipefail

    version=${REPVIX_VERSION:-latest}
    install_dir=${REPVIX_INSTALL_DIR:-}
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version|--install-dir)
                [ "$#" -ge 2 ] && [ -n "$2" ] || die "$1 requires a value"
                case "$1" in
                    --version) version=$2 ;;
                    --install-dir) install_dir=$2 ;;
                esac
                shift 2
                ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown argument: $1 (see --help)" ;;
        esac
    done

    if [ -z "$install_dir" ]; then
        [ -n "${HOME:-}" ] || die 'HOME is not set; use --install-dir'
        install_dir=$HOME/.local/bin
    fi

    for command in curl uname tar mktemp mkdir rm install mv; do
        command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
    done
    if command -v sha256sum >/dev/null 2>&1; then
        checksum_command=(sha256sum)
    elif command -v shasum >/dev/null 2>&1; then
        checksum_command=(shasum -a 256)
    else
        die 'SHA-256 verification requires sha256sum or shasum'
    fi

    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux) platform=unknown-linux-musl ;;
        Darwin) platform=apple-darwin ;;
        *) die "unsupported operating system: $os (supported: Linux and macOS)" ;;
    esac
    case "$arch" in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) die "unsupported architecture: $arch (supported: x86_64 and ARM64)" ;;
    esac

    releases=https://github.com/m13yama/repvix/releases
    if [ "$version" = latest ]; then
        # Resolve once so the archive and checksum always come from the same tag.
        release_url=$(download --output /dev/null --write-out '%{url_effective}' "$releases/latest") \
            || die 'could not find the latest release'
        case "$release_url" in
            "$releases/tag/"*) version=${release_url##*/} ;;
            *) die "unexpected release URL: $release_url" ;;
        esac
    fi
    version=v${version#v}
    [[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] \
        || die "invalid release version: $version"

    asset=repvix-$arch-$platform.tar.gz
    base_url=$releases/download/$version
    temp_dir=$(mktemp -d "${TMPDIR:-/tmp}/repvix.XXXXXXXX")
    staged_binary=
    trap 'rm -rf "$temp_dir"; if [ -n "$staged_binary" ]; then rm -f "$staged_binary"; fi' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    printf 'Downloading repvix %s for %s-%s...\n' "$version" "$arch" "$platform"
    download --output "$temp_dir/$asset" "$base_url/$asset" \
        || die "could not download $asset from $version; check that the release provides this platform"
    download --output "$temp_dir/$asset.sha256" "$base_url/$asset.sha256" \
        || die "could not download the checksum for $asset"

    read -r expected_hash checksum_name < "$temp_dir/$asset.sha256" \
        || die 'invalid checksum file'
    [[ "$expected_hash" =~ ^[0-9a-fA-F]{64}$ ]] && [ "$checksum_name" = "$asset" ] \
        || die 'invalid checksum file'
    actual_hash=$("${checksum_command[@]}" "$temp_dir/$asset")
    [ "${actual_hash%% *}" = "$expected_hash" ] || die 'SHA-256 checksum mismatch; installation aborted'

    tar -xzf "$temp_dir/$asset" -C "$temp_dir" repvix \
        || die 'could not extract repvix from the release archive'
    [ -f "$temp_dir/repvix" ] && [ ! -L "$temp_dir/repvix" ] \
        || die 'release archive does not contain a regular repvix binary'

    mkdir -p "$install_dir" || die "could not create $install_dir; use --install-dir with a writable directory"
    install_dir=$(cd "$install_dir" && pwd -P)
    [ ! -d "$install_dir/repvix" ] || die "$install_dir/repvix is a directory"
    # Stage on the destination filesystem so upgrades replace the binary atomically.
    staged_binary=$(mktemp "$install_dir/.repvix.XXXXXXXX")
    install -m 755 "$temp_dir/repvix" "$staged_binary"
    mv -f "$staged_binary" "$install_dir/repvix"
    staged_binary=

    printf 'Installed repvix %s to %s/repvix\n' "$version" "$install_dir"
    case ":${PATH:-}:" in
        *":$install_dir:"*) ;;
        *)
            # Keep $PATH literal so the user can paste this into their profile.
            # shellcheck disable=SC2016
            printf '\nAdd this directory to PATH in your shell profile:\n  export PATH=%q:"$PATH"\n' "$install_dir"
            ;;
    esac
)

main "$@"
