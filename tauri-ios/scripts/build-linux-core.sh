#!/usr/bin/env bash
# Builds LinuxRunner's core plus our embedding shim (tauri-ios/linux/lr_embed.c) as static libraries
# for iOS arm64. Needs macOS + Xcode + meson + ninja.
#   usage: build-linux-core.sh <LinuxRunner checkout> <output dir>
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
LR="$(cd "${1:?path to LinuxRunner checkout}" && pwd)"
OUT="${2:?output dir}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
WORK="$(mktemp -d)"

# Add our shim to LinuxRunner's meson build so it gets exactly the same flags and headers.
cp "$HERE/linux/lr_embed.c" "$HERE/linux/lr_embed.h" "$LR/"
if ! grep -q "lr_embed" "$LR/meson.build"; then
  printf "\n# added by VScode: embeddable terminal shim\nstatic_library('lr_embed', 'lr_embed.c', dependencies: ish)\n" >> "$LR/meson.build"
fi

cat > "$WORK/cross.txt" <<CROSS
[binaries]
c = 'clang'
ar = 'ar'

[host_machine]
system = 'darwin'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'

[built-in options]
c_args = ['-arch', 'arm64', '-isysroot', '$SDK', '-miphoneos-version-min=16.0']
c_link_args = ['-arch', 'arm64', '-isysroot', '$SDK', '-miphoneos-version-min=16.0']

[properties]
needs_exe_wrapper = true
CROSS

# Same environment trick the LinuxRunner Xcode project uses (app/xcode-meson.sh).
export CC_FOR_BUILD="env -u SDKROOT -u IPHONEOS_DEPLOYMENT_TARGET xcrun clang"
export CC="$CC_FOR_BUILD"

(cd "$LR" && meson setup "$WORK/build" --cross-file "$WORK/cross.txt" \
  -Dguest_arch=arm64 -Djit=false -Djit_emit=false -Dcli_aot= -Dvdso_required=false \
  -Ddefault_library=static --buildtype=release)
ninja -C "$WORK/build" liblr_embed.a libish.a libish_emu.a libfakefs.a

cp "$WORK/build/"liblr_embed.a "$WORK/build/"libish.a "$WORK/build/"libish_emu.a "$WORK/build/"libfakefs.a "$OUT/"
ls -la "$OUT"
