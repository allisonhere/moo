#!/bin/sh
# Installs a prebuilt moo release binary. Usage:
#   curl -fsSL https://raw.githubusercontent.com/allisonhere/moo/master/scripts/install.sh | sh
# Set MOO_VERSION (e.g. v0.9.7) to pin a release and MOO_INSTALL_DIR to change the destination.
set -eu

repo=allisonhere/moo
install_dir=${MOO_INSTALL_DIR:-"${HOME}/.local/bin"}

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) asset=moo-linux-x86_64 ;;
    Linux-aarch64 | Linux-arm64) asset=moo-linux-aarch64 ;;
    *)
        echo "error: no prebuilt moo for $(uname -s) $(uname -m); build from source with scripts/install-moo.sh" >&2
        exit 1
        ;;
esac

if [ -n "${MOO_VERSION:-}" ]; then
    base="https://github.com/$repo/releases/download/$MOO_VERSION"
else
    base="https://github.com/$repo/releases/latest/download"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"

expected=$(awk -v f="$asset" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
if [ -z "$expected" ]; then
    echo "error: $asset is not listed in SHA256SUMS" >&2
    exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
else
    actual=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
fi
if [ "$expected" != "$actual" ]; then
    echo "error: checksum mismatch for $asset" >&2
    exit 1
fi

mkdir -p "$install_dir"
chmod 755 "$tmp/$asset"
mv "$tmp/$asset" "$install_dir/moo"

printf 'installed %s\n' "$install_dir/moo"
printf 'config: %s\n' "${XDG_CONFIG_HOME:-${HOME}/.config}/moo/config.toml"
printf 'state:  %s\n' "${XDG_STATE_HOME:-${HOME}/.local/state}/moo"
