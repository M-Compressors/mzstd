#!/usr/bin/env bash
# Installs the alef CLI this directory is generated with: the release pinned
# by `alef_version` in bindings/alef.toml, plus the fixes in
# bindings/alef-patches/ that are not released upstream yet.
#
#   bindings/scripts/install-alef.sh [install-root]
#
# The binary lands in <install-root>/bin/alef (default: bindings/target/alef).
# Rebuilding is skipped when that binary was built from the same version and
# patches. Delete a patch once the pinned release contains it, and drop this
# script for `cargo install alef --locked` once no patch is left.
set -euo pipefail

bindings="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
root="${1:-$bindings/target/alef}"
version="$(sed -n 's/^alef_version = "\(.*\)"$/\1/p' "$bindings/alef.toml")"
if [ -z "$version" ]; then
  echo "error: no alef_version in $bindings/alef.toml" >&2
  exit 1
fi

shopt -s nullglob
patches=("$bindings"/alef-patches/*.patch)
stamp="$version $(cat "${patches[@]}" /dev/null | shasum -a 256 | cut -d' ' -f1)"
if [ -x "$root/bin/alef" ] && [ "$(cat "$root/.stamp" 2>/dev/null)" = "$stamp" ]; then
  echo "alef $version with ${#patches[@]} patch(es) is already in $root"
  exit 0
fi

if [ "${#patches[@]}" -eq 0 ]; then
  cargo install alef --version "$version" --locked --root "$root" --force
else
  source="$(mktemp -d)"
  trap 'rm -rf "$source"' EXIT
  git clone --quiet --depth 1 --branch "v$version" https://github.com/xberg-io/alef "$source"
  for patch in "${patches[@]}"; do
    git -C "$source" apply "$patch"
  done
  cargo install --path "$source" --locked --root "$root" --force
fi
echo "$stamp" > "$root/.stamp"
echo "installed alef $version with ${#patches[@]} patch(es) into $root/bin"
