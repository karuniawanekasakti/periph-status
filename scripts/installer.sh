#!/usr/bin/env bash
# Build the Windows NSIS installer from WSL (Tauri cross-compilation via cargo-xwin).
# One-time setup in WSL:
#   sudo apt install clang lld llvm nsis
#   cargo install cargo-xwin --locked
#   cargo install tauri-cli --version "^2" --locked
# Output: $CARGO_TARGET_DIR/x86_64-pc-windows-msvc/release/bundle/nsis/*-setup.exe
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:/usr/lib/llvm-18/bin:$PATH"
export XWIN_ACCEPT_LICENSE=1
# Keep build output on the WSL filesystem (faster, and not scanned by the Windows antivirus).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Project/periph-status/target}"

# The Tauri CLI on a Linux host always looks up the appindicator library through pkg-config
# when the tray feature is enabled, even for Windows targets (the value is only used for
# AppImage/deb bundles). Provide a stub .pc file instead of installing the GTK dev stack.
if ! pkg-config --exists ayatana-appindicator3-0.1 appindicator3-0.1 2>/dev/null; then
  stub="$CARGO_TARGET_DIR/pkgconfig-stub"
  mkdir -p "$stub"
  cat > "$stub/ayatana-appindicator3-0.1.pc" <<'PC'
libdir=/usr/lib/x86_64-linux-gnu
Name: ayatana-appindicator3-0.1
Description: Stub for Tauri Windows cross-builds (unused for NSIS bundles)
Version: 0.5.90
Libs: -L${libdir}
PC
  export PKG_CONFIG_PATH="$stub${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi

cd "$here/src-tauri"
exec cargo tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc "$@"
