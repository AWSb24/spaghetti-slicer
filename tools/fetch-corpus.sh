#!/usr/bin/env bash
# Fetches the real-world test models listed in corpus/*.tsv and verifies
# their checksums. See corpus/README.md for sources and licenses.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repo="https://github.com/CreativeTools/3DBenchy.git"
revision="b542785f9642ab39db8a778a0ccc934b3ebd829a"
target="$root/corpus/3dbenchy"
manifest="$root/corpus/3dbenchy.tsv"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

git init -q "$work/src"
git -C "$work/src" remote add origin "$repo"
git -C "$work/src" fetch -q --depth 1 origin "$revision"
git -C "$work/src" checkout -q FETCH_HEAD

mkdir -p "$target"
count=0
while IFS=$'\t' read -r expected name source; do
  [ -n "$expected" ] || continue
  cp "$work/src/$source" "$target/$name"
  actual="$(sha256 "$target/$name")"
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $name: expected $expected, got $actual" >&2
    exit 1
  fi
  count=$((count + 1))
done < "$manifest"
echo "fetched and verified $count files into $target"
