#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Modelled on warp-terminal `script/portable/build-portable.sh` and `package-release.sh`
# @ 5bb7bcc6a (build with the release tag embedded, verify, stage and archive); the script is
# new.
#
# Builds the portable MatterLite archive for Linux x64 or macOS arm64 (MatterLite.md §16):
#
#   script/build-portable.sh            # v<crate version>-dev.<commit>
#   script/build-portable.sh v0.3.0     # a release
#
# 1. Builds `matterlite` with the `rlto` profile (thin LTO, symbols stripped) and
#    GIT_RELEASE_TAG set (the version is compiled in).
# 2. Verifies the binary: the tag is in it (the archive name can't lie) and it is large enough
#    to hold the embedded assets. On macOS it is signed ad hoc (Apple Silicon runs nothing
#    unsigned).
# 3. Stages the binary, an empty `data` folder (which makes it portable) with a README, and
#    the licences; checks that no user data got into `data`.
# 4. Writes dist/matterlite-<tag>-<platform>.tar.gz (the executable bit survives) and prints
#    its SHA-256. OUT_DIR overrides dist.

set -euo pipefail
cd "$(dirname "$0")/.."

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) platform=linux-x64 ;;
    Darwin-arm64) platform=macos-arm64 ;;
    *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 2 ;;
esac

tag=${1:-}
if [ -z "$tag" ]; then
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/matterlite/Cargo.toml | head -1)
    tag="v$version-dev.$(git rev-parse --short=7 HEAD)"
fi
if ! [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]]; then
    echo "tag '$tag' isn't v<semver> (e.g. v0.3.0 or v0.3.0-rc.1)" >&2
    exit 2
fi
out=${OUT_DIR:-dist}
# The emoji images alone are 6.4 MB; a binary without the embedded assets is far smaller.
min_size=$((12 * 1024 * 1024))
echo "== MatterLite $tag for $platform"

GIT_RELEASE_TAG=$tag cargo build --profile rlto --locked -p matterlite
bin=target/rlto/matterlite

if ! grep -qaF -- "$tag" "$bin"; then
    echo "the built binary doesn't contain '$tag': it wasn't rebuilt with the tag" >&2
    exit 1
fi
size=$(wc -c < "$bin" | tr -d ' ')
if [ "$size" -lt "$min_size" ]; then
    echo "the binary is only $size bytes: the assets aren't embedded" >&2
    exit 1
fi
echo "   verified: version $tag, $((size / 1024 / 1024)) MB"

name="matterlite-$tag-$platform"
stage="$out/$name"
rm -rf "$stage"
mkdir -p "$stage/data"
cp "$bin" "$stage/"
if [ "$platform" = macos-arm64 ]; then
    codesign --force --sign - "$stage/matterlite"
fi
printf '%s\n' \
    "MatterLite keeps its settings, cache and logs in this folder (portable mode)." \
    "Delete the folder to use the per-user folders of your system instead." \
    > "$stage/data/README.txt"
cp LICENSE LICENSE-MIT LICENSE-APACHE README.md PROVENANCE.md "$stage/"

# No user data from test runs in the archive.
extra=$(find "$stage/data" -mindepth 1 ! -name README.txt)
if [ -n "$extra" ]; then
    echo "unexpected files in the staged data folder:" >&2
    echo "$extra" >&2
    exit 1
fi

archive="$out/$name.tar.gz"
tar -C "$out" -czf "$archive" "$name"
if command -v sha256sum > /dev/null; then
    hash=$(sha256sum "$archive" | cut -d' ' -f1)
else
    hash=$(shasum -a 256 "$archive" | cut -d' ' -f1)
fi
echo "== $archive"
echo "   sha256 $hash"
