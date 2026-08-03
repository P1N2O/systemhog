#!/usr/bin/env bash
# Build systemhog for every Linux target the local toolchain supports.
#
#   musl targets   -> fully static binaries; linked with the rust-lld
#                     bundled with rustup (no C toolchain needed).
#   glibc targets  -> dynamically linked (standard glibc practice);
#                     ARM cross compilers come from tools/cross-toolchains.sh
#                     (or apt: gcc-aarch64-linux-gnu gcc-arm-linux-gnueabihf).
#
# Windows/macOS builds are done natively (or via .github/workflows/release.yml).
set -euo pipefail

cd "$(dirname "$0")"
TC="${SYSTEMHOG_TOOLCHAIN_DIR:-$HOME/.local/toolchains}"

# target | linker (empty = default cc) | needs prefix LD_LIBRARY_PATH
TARGETS=(
    "x86_64-unknown-linux-musl|rust-lld|0"
    "x86_64-unknown-linux-gnu||0"
    "aarch64-unknown-linux-musl|rust-lld|0"
    "aarch64-unknown-linux-gnu|$TC/usr/bin/aarch64-linux-gnu-gcc|1"
    "armv7-unknown-linux-musleabihf|rust-lld|0"
    "armv7-unknown-linux-gnueabihf|$TC/usr/bin/arm-linux-gnueabihf-gcc|1"
    "i686-unknown-linux-musl|rust-lld|0"
    "riscv64gc-unknown-linux-musl|rust-lld|0"
)

mkdir -p dist

# riscv64gc-unknown-linux-musl's target spec always emits -lgcc_s (no
# libgcc shipped) and skips libunwind; an empty stub + explicit -lunwind
# fix both for this pure-Rust crate.
STUB_DIR="$(mktemp -d)"
trap 'rm -rf "$STUB_DIR"' EXIT
ar cr "$STUB_DIR/libgcc_s.a"

for entry in "${TARGETS[@]}"; do
    IFS='|' read -r target linker need_ld <<<"$entry"
    rustup target add "$target" >/dev/null 2>&1 || true
    echo "==> building $target"

    FLAGS=""
    if [ -n "$linker" ]; then
        # rustc resolves `rust-lld` from its own sysroot; other linkers
        # must exist on PATH or as an absolute path.
        if [ "$linker" != "rust-lld" ] && ! command -v "$linker" >/dev/null 2>&1 && [ ! -x "$linker" ]; then
            echo "    SKIP: linker '$linker' not found"
            echo "          run tools/cross-toolchains.sh (or apt-get install gcc-aarch64-linux-gnu gcc-arm-linux-gnueabihf)"
            continue
        fi
        FLAGS="-C linker=$linker"
    fi
    if [ "$target" = "riscv64gc-unknown-linux-musl" ]; then
        FLAGS="$FLAGS -C link-self-contained=yes -C link-arg=-L$STUB_DIR -C link-arg=-lunwind"
    fi

    ENV=()
    if [ "$need_ld" = "1" ]; then
        ENV=(env LD_LIBRARY_PATH="$TC/usr/lib/x86_64-linux-gnu")
    fi
    if RUSTFLAGS="$FLAGS" "${ENV[@]}" cargo build --release --target "$target" 2>"dist/build-$target.log"; then
        cp "target/$target/release/systemhog" "dist/systemhog-$target"
        echo "    ok: $(du -h "dist/systemhog-$target" | cut -f1)  dist/systemhog-$target"
        rm -f "dist/build-$target.log"
    else
        echo "    FAILED (see dist/build-$target.log)"
    fi
done

echo
echo "done. artifacts in dist/:"
ls -1 dist/
