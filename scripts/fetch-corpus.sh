#!/usr/bin/env bash
# Shallow-clones the repositories in corpus/manifest.toml at their pinned commits into
# ${XDG_CACHE_HOME:-~/.cache}/dumbj/corpus/<name>. Safe to re-run; prints the directory.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cache=${XDG_CACHE_HOME:-$HOME/.cache}/dumbj/corpus
mkdir -p "$cache"

name= url=
while IFS= read -r line; do
    case $line in
        'name = '*) name=${line#name = \"}; name=${name%\"} ;;
        'url = '*) url=${line#url = \"}; url=${url%\"} ;;
        'commit = '*)
            commit=${line#commit = \"}; commit=${commit%\"}
            dir=$cache/$name
            if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)" != "$commit" ]; then
                echo "fetching $name at $commit" >&2
                mkdir -p "$dir"
                git -C "$dir" init -q
                git -C "$dir" fetch -q --depth 1 "$url" "$commit"
                git -C "$dir" checkout -q --detach FETCH_HEAD
            fi
            ;;
    esac
done <"$root/corpus/manifest.toml"
echo "$cache"
