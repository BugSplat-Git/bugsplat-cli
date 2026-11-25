#!/usr/bin/env bash
# Checks that a Linux release archive's `bugsplat` is statically linked, then runs it in a
# container of each image, with no network, to show it needs nothing from the distribution.
# Usage: linux-container-test.sh <archive (.tar.gz or .tar.xz)> <target triple> <image>...
set -euo pipefail

archive=$1
target=$2
shift 2
fixtures=$(cd "$(dirname "$0")/../../crates/bugsplat-debugid/tests/fixtures" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir "$work/archive" "$work/elf"
tar -xf "$archive" -C "$work/archive"
bin=$(find "$work/archive" -type f -name bugsplat | head -n 1)
[[ -n $bin ]] || { echo "no bugsplat binary in $archive" >&2; exit 1; }

file "$bin"
headers=$(readelf --program-headers --dynamic --wide "$bin")
if grep -E 'INTERP|NEEDED' <<< "$headers"; then
  echo "FAIL: $target binary asks for a loader or a shared library" >&2
  exit 1
fi

# The only C++ in bugsplat is symbolic's Swift demangler, and only a Swift symbol reaches it.
printf 'void greet(void) __asm__("$s4main5greetyyF");\nvoid greet(void) {}\n' |
  cc -shared -nostdlib -x c - -o "$work/elf/libswift.so"

for image in "$@"; do
  docker run --rm --network none \
    --volume "$(dirname "$bin"):/bugsplat:ro" \
    --volume "$fixtures:/fixtures:ro" \
    --volume "$work/elf:/elf:ro" \
    --env TARGET="$target" \
    --env BUGSPLAT_TOKEN_STORE=file \
    --env BUGSPLAT_CONFIG_DIR=/tmp/config \
    "$image" sh -euc '
      . /etc/os-release
      echo "== $PRETTY_NAME, $(uname -m)"
      bugsplat=/bugsplat/bugsplat

      $bugsplat --version | grep -F "($TARGET)"
      $bugsplat symbols info /fixtures/elfy/hello.elf | grep -F 85fe216fc7dd441f04c237310a56081fbf23c082

      $bugsplat symbols dump --output /tmp/sym /fixtures/elfy/hello.elf
      head -n 1 /tmp/sym/hello.sym | grep -Fx "MODULE Linux x86_64 6F21FE85DDC71F4404C237310A56081F0 hello.elf"

      $bugsplat symbols dump --output /tmp/sym /elf/libswift.so
      grep -E "^PUBLIC .* greet\(\)$" /tmp/sym/libswift.so.sym

      # Two ELF files the build did not choose: the image'"'"'s shell, and bugsplat itself.
      sh=$(readlink -f /bin/sh)
      $bugsplat symbols dump --output /tmp/sym "$sh" /bugsplat/bugsplat
      for sym in "/tmp/sym/$(basename "$sh").sym" /tmp/sym/bugsplat.sym; do
        echo "$sym: $(head -n 1 "$sym"), $(grep -c "^PUBLIC " "$sym") PUBLIC records"
        grep -q "^PUBLIC " "$sym"
      done
      grep -q "^PUBLIC .* bugsplat_dumpsyms::" /tmp/sym/bugsplat.sym
    '
  echo "ran in $image: $target"
done
