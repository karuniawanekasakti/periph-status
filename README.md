# Periph Status

A small tray app that shows the battery level of wireless peripherals connected to the computer
(mouse, keyboard, TWS/headset, controller), over Bluetooth or a supported 2.4 GHz receiver.
Built with Rust + Tauri v2. Windows 10/11 for now; macOS later.

## Features
- System tray icon. Left-click opens a popup with the device list and battery levels; right-click
  opens a menu (Open, Refresh now, Start at login, Quit).
- Windows notification when a device's battery drops to or below the threshold (default 20%).
  It fires once, and only re-arms after the battery rises to threshold + 5%.
- Optional start at login.
- Settings: low battery threshold, check interval, show disconnected devices.

## Windows setup (one time)
1. Install **Visual Studio Build Tools 2022** (or Community) with the components
   *MSVC v143 x64/x86 build tools* and a *Windows 11 SDK*.
2. Install **Rust**: https://rustup.rs (default toolchain `x86_64-pc-windows-msvc`).
3. WebView2 ships with Windows 11. On Windows 10, install the *WebView2 Runtime* if missing.
4. Install the Tauri CLI:
   ```powershell
   cargo install tauri-cli --version "^2" --locked
   ```

Node.js is **not** required: the UI is static HTML/CSS/JS in `ui/`.

## Running
Run every command from the `src-tauri` folder:

```powershell
cd periph-status\src-tauri

# Check battery reading only, without the UI (compare with Settings › Bluetooth & devices)
cargo run --example scan

# Unit tests
cargo test

# Development mode (tray + popup)
cargo tauri dev

# Build the NSIS installer → target\release\bundle\nsis\
cargo tauri build
```

### Alternative: build from WSL
Use this when a Windows antivirus (e.g. Acronis) quarantines cargo build scripts.
Build scripts run on Linux; the resulting `.exe` runs on Windows.

One time in WSL: `sudo apt install clang lld llvm`, then `cargo install cargo-xwin --locked`.

```bash
scripts/xwin.sh build --example scan     # then run ~/Project/periph-status/target/x86_64-pc-windows-msvc/debug/examples/scan.exe
scripts/xwin.sh build --example dump     # dump raw Bluetooth/PnP properties for debugging
scripts/xwin.sh build --release          # the app: .../release/periph-status.exe
scripts/installer.sh                     # NSIS installer: .../release/bundle/nsis/Periph Status_<version>_x64-setup.exe
```

`scripts/installer.sh` additionally needs `sudo apt install nsis` and
`cargo install tauri-cli --version "^2" --locked` in WSL. The installer is per-user (no admin
rights), installs to `%LOCALAPPDATA%\Periph Status`, and supports `/S` for a silent install.
It is not code-signed, so SmartScreen may warn on first run.

Note: disconnected devices still have the **last known** battery value stored by Windows (or the
receiver). `scan` shows it in parentheses; the popup labels it "last known".

## How it works
| File | Contents |
| --- | --- |
| `src-tauri/src/provider/win.rs` | Reads devices and battery through WinRT (the `windows` crate) |
| `src-tauri/src/provider/huawei.rs` | Huawei FreeBuds SPP protocol (left/right/case battery) |
| `src-tauri/src/provider/rongyuan.rs` | Rongyuan 2.4 GHz receiver protocol (HID feature reports) |
| `src-tauri/src/provider/mod.rs` | `BatteryProvider` trait + merging data per physical device |
| `src-tauri/src/monitor.rs` | Polling thread, snapshot, UI events, tooltip, notifications |
| `src-tauri/src/alerts.rs` | Notification logic (hysteresis) and tooltip text |
| `src-tauri/src/settings.rs` | Settings (`%APPDATA%\com.periphstatus.desktop\settings.json`) |
| `src-tauri/src/lib.rs` | Tray, popup, plugins and Tauri commands |
| `ui/` | Popup UI |

Battery reading on Windows:
1. Paired devices (Bluetooth classic + LE) are enumerated as *AssociationEndpoints*, including
   their connection state and `ContainerId`.
2. The battery level comes from the PnP property `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`, the
   same value Windows Settings shows. It is matched to devices through `ContainerId`.
3. For connected BLE devices without that property, the app reads the GATT Battery Service
   (`0x180F` / `0x2A19`) directly.
4. **Huawei FreeBuds**: Windows only gets a coarse level over Hands-Free (e.g. 100% while the
   earbuds are at 91%). For connected FreeBuds the app opens the Serial Port (RFCOMM) service and
   sends the Huawei battery request (`5A 00 09 00 01 08 01 00 02 00 03 00 FB B9`); the reply carries
   the global level plus left/right/case and charging flags. See `src-tauri/src/provider/huawei.rs`.
   Wire format as documented by [OpenFreebuds](https://github.com/melianmiko/OpenFreebuds) and
   [hrbtnog/freebuds-client](https://github.com/hrbtnog/freebuds-client) (independent implementation).
5. **Rongyuan-chip 2.4 GHz receivers (VID `3151`)**, used by many wireless mice/keyboards: the app
   sends the status query `SET_FEATURE [00 F7 00 …]` to vendor collection `FFFF:0002`, then reads
   `GET_FEATURE` report 0 (`[2]` keyboard battery, `[3]` mouse battery, `[4]`/`[5]` offline flags).
   See `src-tauri/src/provider/rongyuan.rs`. The protocol follows
   [qu1ncky/BatteryTray](https://github.com/qu1ncky/BatteryTray) via
   [krisgfx9/Wireless-Device-Battery-Monitor](https://github.com/krisgfx9/Wireless-Device-Battery-Monitor).

Other debug examples: `hidprobe` (list HID collections), `hidlisten` (passively listen to vendor
reports), `dongle` (query a receiver step by step), `huawei` (Huawei SPP battery query).

## Limitations
- Devices that don't report their battery to Windows show "—".
- Most TWS earbuds only report a single, coarse value through Windows. Separate left/right/case
  levels are only read for Huawei FreeBuds (tested on FreeBuds 6i).
- 2.4 GHz receivers other than the Rongyuan chip (e.g. Logitech Unifying/Bolt) are not supported yet.
- Rongyuan receivers report the mouse as "offline" while it sleeps; the level shown then is the
  last value the receiver remembers.
- In `cargo tauri dev`, notifications appear under PowerShell's name. Once installed with the
  installer, they appear under the app's name.

## License
[MIT](LICENSE)
