#!/bin/sh
# Install a verified GitHub release without sudo.
set -eu

main() {
    install_dir=${MDMANAGER_INSTALL_DIR:-"$HOME/.local/bin"}
    os=$(uname -s)
    arch=$(uname -m)
    case "$arch" in
        x86_64|amd64) arch=x86_64 ;;
        aarch64|arm64) arch=aarch64 ;;
        *) echo "mdmanager: unsupported architecture: $arch" >&2; exit 1 ;;
    esac
    case "$os" in
        Linux) target=$arch-unknown-linux-musl ;;
        Darwin) target=$arch-apple-darwin ;;
        *) echo "mdmanager: supported systems are Linux and macOS" >&2; exit 1 ;;
    esac

    repo=https://github.com/manuelschipper/mdmanager
    version=${MDMANAGER_VERSION:-}
    if [ -z "$version" ]; then
        release_url=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$repo/releases/latest")
        version=${release_url##*/}
    fi
    case "$version" in
        v[0-9]*) ;;
        *) echo "mdmanager: expected a release tag such as v0.1.0" >&2; exit 1 ;;
    esac
    case "$version" in
        *[!A-Za-z0-9._-]*) echo "mdmanager: invalid release tag" >&2; exit 1 ;;
    esac
    base=$repo/releases/download/$version
    asset=mdmanager-$target.tar.gz
    download_dir=$(mktemp -d)
    trap 'rm -r "$download_dir"' EXIT
    trap 'exit 1' HUP INT TERM

    echo "Downloading mdmanager $version for $target..."
    curl -fsSL -o "$download_dir/$asset" "$base/$asset"
    curl -fsSL -o "$download_dir/sha256sums.txt" "$base/sha256sums.txt"
    expected=$(awk -v asset="$asset" '
        $2 == asset && length($1) == 64 && $1 !~ /[^0-9A-Fa-f]/ { print tolower($1) }
    ' "$download_dir/sha256sums.txt")
    case "$os" in
        Linux) actual=$(sha256sum "$download_dir/$asset") ;;
        Darwin) actual=$(shasum -a 256 "$download_dir/$asset") ;;
    esac
    actual=${actual%% *}
    if [ "$actual" != "$expected" ]; then
        echo "mdmanager: checksum verification failed; installation unchanged" >&2
        exit 1
    fi

    tar -xzf "$download_dir/$asset" -C "$download_dir" mdmanager
    test "$("$download_dir/mdmanager" --version)" = "mdmanager ${version#v}"
    mkdir -p "$install_dir"
    test ! -d "$install_dir/mdmanager"
    # Stage beside the destination so replacement also works while the TUI is running.
    staged_binary=$(mktemp "$install_dir/.mdmanager.XXXXXX")
    trap 'rm -r "$download_dir"; rm -f "$staged_binary"' EXIT
    cp "$download_dir/mdmanager" "$staged_binary"
    chmod 755 "$staged_binary"
    mv -f "$staged_binary" "$install_dir/mdmanager"
    echo "Installed mdmanager ${version#v} to $install_dir/mdmanager"
    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *) printf '\nAdd this directory to PATH in your shell profile:\n  export PATH="%s:$PATH"\n' "$install_dir" ;;
    esac
    printf '\nNext: mdmanager docs start\n'
}

main
