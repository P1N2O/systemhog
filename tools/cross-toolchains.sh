#!/usr/bin/env bash
# Bootstrap ARM cross-compiler toolchains for glibc targets WITHOUT root,
# by downloading the Ubuntu .deb packages and extracting them into a local
# prefix. (With sudo you can instead: apt-get install -y \
#   gcc-aarch64-linux-gnu gcc-arm-linux-gnueabihf)
#
# Usage: tools/cross-toolchains.sh [PREFIX]
#   PREFIX defaults to $HOME/.local/toolchains.
#
# The cross glibc linker scripts reference absolute /usr/<triple> paths;
# after extraction they are patched to point into PREFIX. Host-side binutils
# libs (libopcodes etc.) live in the prefix too, so builds must set
# LD_LIBRARY_PATH=PREFIX/usr/lib/x86_64-linux-gnu (build-all.sh does this).

set -euo pipefail

PREFIX="${1:-$HOME/.local/toolchains}"
DEBDIR="$(mktemp -d)"
trap 'rm -rf "$DEBDIR"' EXIT
cd "$DEBDIR"

AARCH64="gcc-aarch64-linux-gnu gcc-15-aarch64-linux-gnu cpp-15-aarch64-linux-gnu \
gcc-15-aarch64-linux-gnu-base gcc-15-cross-base binutils-aarch64-linux-gnu \
libc6-dev-arm64-cross libc6-arm64-cross linux-libc-dev-arm64-cross \
libgcc-15-dev-arm64-cross libgcc-s1-arm64-cross libgomp1-arm64-cross \
libitm1-arm64-cross libatomic1-arm64-cross libasan8-arm64-cross \
liblsan0-arm64-cross libtsan2-arm64-cross libstdc++6-arm64-cross"

ARMHF="gcc-arm-linux-gnueabihf gcc-15-arm-linux-gnueabihf cpp-15-arm-linux-gnueabihf \
gcc-15-arm-linux-gnueabihf-base binutils-arm-linux-gnueabihf \
libc6-dev-armhf-cross libc6-armhf-cross linux-libc-dev-armhf-cross \
libgcc-15-dev-armhf-cross libgcc-s1-armhf-cross libgomp1-armhf-cross \
libatomic1-armhf-cross libasan8-armhf-cross libubsan1-armhf-cross \
libstdc++6-armhf-cross"

echo "downloading aarch64 toolchain..."
for p in $AARCH64; do apt-get download "$p" >/dev/null; done
echo "downloading armhf toolchain..."
for p in $ARMHF; do apt-get download "$p" >/dev/null; done

mkdir -p "$PREFIX"
for d in *.deb; do dpkg-deb -x "$d" "$PREFIX"; done

# Patch linker scripts (text files only!) to point at the real prefix.
patch_scripts() { # $1 = triple dir
    local triple="$1"
    local dir="$PREFIX/usr/$triple"
    local f
    for f in "$dir"/lib/*.so; do
        [ -f "$f" ] || continue
        if file "$f" | grep -q "ASCII text"; then
            sed -i "s|/usr/$triple|$PREFIX/usr/$triple|g" "$f"
        fi
    done
}
patch_scripts aarch64-linux-gnu
patch_scripts arm-linux-gnueabihf

echo
echo "toolchains ready in $PREFIX"
"$PREFIX/usr/bin/aarch64-linux-gnu-gcc" --version | head -1
"$PREFIX/usr/bin/arm-linux-gnueabihf-gcc" --version | head -1
echo
echo "build with:"
echo "  export LD_LIBRARY_PATH=$PREFIX/usr/lib/x86_64-linux-gnu"
echo "  RUSTFLAGS=\"-C linker=$PREFIX/usr/bin/aarch64-linux-gnu-gcc\" cargo build --release --target aarch64-unknown-linux-gnu"
