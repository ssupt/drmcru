#!/bin/bash
set -euo pipefail
version=""; output=""; cmdline=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --kernel) version="$2"; shift 2 ;;
        --generate|--uki) output="$2"; shift 2 ;;
        --cmdline) cmdline="$2"; shift 2 ;;
        -S|--splash|--uefistub|--ukiconfig) shift 2 ;;
        --no-cmdline|--no-ukify) shift ;;
        *) echo "Unexpected mkinitcpio option: $1" >&2; exit 1 ;;
    esac
done
[ ! -e "$TEST_ROOT/fail-build" ] || exit 42
count=0
[ ! -e "$TEST_ROOT/build-count" ] || count="$(cat "$TEST_ROOT/build-count")"
count=$((count+1)); printf '%s' "$count" > "$TEST_ROOT/build-count"
if [ -e "$TEST_ROOT/fail-second-build" ] && [ "$count" -eq 2 ]; then exit 42; fi
if [ -e "$TEST_ROOT/skip-build" ]; then exit 0; fi
payload="$(mktemp -d "$TEST_ROOT/payload.XXXXXX")"
trap 'rm -rf -- "$payload"' EXIT
FILES=()
source "$TEST_ROOT/mkinitcpio.conf"
shopt -s nullglob
for config in "$TEST_ROOT/mkinitcpio.conf.d/"*.conf; do source "$config"; done
for file in "${FILES[@]}"; do
    if [[ "$file" == "$TEST_ROOT/firmware-root/"* ]]; then
        relative="${file#"$TEST_ROOT/firmware-root/"}"
        mkdir -p "$payload/usr/lib/firmware/${relative%/*}"
        cp -- "$file" "$payload/usr/lib/firmware/$relative"
    fi
done
[ -z "$cmdline" ] || cp -- "$cmdline" "$payload/.cmdline"
if [ -e "$TEST_ROOT/wrong-cmdline" ]; then printf 'root=wrong' > "$payload/.cmdline"; fi
printf '%s' "$version" > "$payload/.uname"
if [ -e "$TEST_ROOT/wrong-firmware" ]; then
    find "$payload/usr/lib/firmware" -type f -exec sh -c 'printf stale > "$1"' sh {} \;
fi
tar -cf "$output" -C "$payload" .
if [ -e "$TEST_ROOT/bad-archive" ]; then printf 'invalid' > "$output"; fi
