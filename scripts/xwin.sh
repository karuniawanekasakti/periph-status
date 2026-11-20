#!/usr/bin/env bash
# Cross-compile for Windows from WSL (cargo-xwin) without running build scripts on Windows.
# Examples:
#   scripts/xwin.sh build --example scan
#   scripts/xwin.sh build --release
# Output: $CARGO_TARGET_DIR/x86_64-pc-windows-msvc/<debug|release>/
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:/usr/lib/llvm-18/bin:$PATH"
export XWIN_ACCEPT_LICENSE=1
# Keep build output on the WSL filesystem (faster, and not scanned by the Windows antivirus).
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/Project/periph-status/target}"
cmd="$1"; shift
cd "$here/src-tauri"
exec cargo xwin "$cmd" --target x86_64-pc-windows-msvc "$@"
