#!/usr/bin/env bash
# Build service images with Nix and load them into Docker, as
# tinytrader/<service>:nix. On Linux with Nix, Nix builds them directly; on a
# Mac (or without Nix) they are built inside Docker's own Linux VM, in a Nix
# container whose store is kept in the `tinytrader-nix` volume between runs,
# so no second VM is needed.
#   nix/build-images.sh console relay ...
# Builds what Git tracks in the checkout, including uncommitted changes.
#   NIX_BUILD_CORES  cores for the build (default 4)
set -euo pipefail
src="$(cd "$(dirname "$0")/.." && pwd)"
cores="${NIX_BUILD_CORES:-4}"
[ $# -gt 0 ] || { echo "usage: $0 SERVICE..." >&2; exit 2; }

for s in "$@"; do
  echo "== image $s" >&2
  if [ "$(uname -s)" = Linux ] && command -v nix >/dev/null; then
    "$(nix build --no-link --print-out-paths --cores "$cores" "$src#image-$s")" | docker load
  else
    # Only the stream reaches stdout; the build's log goes to stderr.
    docker run --rm \
      -v tinytrader-nix:/nix \
      -v "$src:/src:ro" \
      nixos/nix:2.35.2 sh -ec '
        git config --global --add safe.directory "*"
        out=$(nix --extra-experimental-features "nix-command flakes" \
          --option sandbox false --option filter-syscalls false \
          build --no-link --print-out-paths --max-jobs 1 --cores "$1" \
          "git+file:///src#image-$2")
        "$out"' sh "$cores" "$s" | docker load
  fi
done
