#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
install_dir=${HERDR_FORGE_INSTALL_DIR:-"${HOME}/.local/bin"}
binary="$repo_dir/target/release/herdr-forge"
destination="$install_dir/herdr-forge"

cd "$repo_dir"
cargo build --release --locked --bin herdr-forge

mkdir -p "$install_dir"
temporary="$destination.tmp.$$"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
cp "$binary" "$temporary"
chmod 755 "$temporary"
mv "$temporary" "$destination"
trap - EXIT HUP INT TERM

printf 'installed %s\n' "$destination"
printf 'config: %s\n' "${XDG_CONFIG_HOME:-${HOME}/.config}/herdr-forge/config.toml"
printf 'state:  %s\n' "${XDG_STATE_HOME:-${HOME}/.local/state}/herdr-forge"
