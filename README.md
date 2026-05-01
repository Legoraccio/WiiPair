# WiiPair

Bridges Bluetooth Wii controllers to virtual Xbox 360 pads on the desktop, so
that games like Clone Hero, Steam, or anything that speaks XInput see them as
standard controllers — no emulator required.

The original motivation: play Clone Hero with a Guitar Hero / Rock Band Wii
guitar.

## Features

- Continuous Bluetooth scan with automatic pairing (no manual Windows
  pairing dance — handles the legacy 1+2 PIN trick internally).
- Auto-connect on power-on, auto-disconnect on power-off, with a 2 s
  inactivity watchdog that catches Wiimotes that go silent.
- Virtual Xbox 360 pad output via [ViGEmBus](https://github.com/nefarius/ViGEmBus).
- Per-extension UI panels showing live button state.
- Guitar Hero / Rock Band guitars mapped Xplorer-style for Clone Hero
  (frets → A/B/X/Y/LB, strum → D-pad, whammy → RX axis).

## Supported devices

| Device                                         | Status     | Notes                                           |
| ---------------------------------------------- | ---------- | ----------------------------------------------- |
| Wii Remote (`RVL-CNT-01`)                      | ✅ Full    | Buttons, accelerometer, IR camera (extended).   |
| Wii Remote Plus (`RVL-CNT-01-TR`)              | ✅ Full    | Same as Wii Remote.                             |
| Nunchuk                                        | ✅ Full    | Analog stick, C, Z, accelerometer.              |
| Classic Controller / Pro                       | ✅ Full    | Full pad layout (A/B/X/Y, ZL/ZR, L/R, +/−, Home, D-pad). |
| Guitar Hero / Rock Band guitar (Wii)           | ✅ Full    | 5 frets, strum ↑/↓, whammy bar, +/−. Xplorer-compatible. |
| Guitar Hero / Rock Band drums (Wii)            | ✅ Full    | 5 pads + bass pedal + +/−.                     |
| DJ Hero turntable                              | ⚠️ ID only | Identified, no data parsing yet.                |
| Wii Motion Plus                                | ⚠️ ID only | Identified, no gyro parsing yet.                |
| uDraw Tablet, Taiko TaTaCon                    | ⚠️ ID only | Identified by extension ID.                     |
| Wii Balance Board                              | ❌         | Separate Bluetooth device (not an extension).   |

## Platform support

| OS                  | Status                                                                     |
| ------------------- | -------------------------------------------------------------------------- |
| **Windows 10 / 11** | ✅ Full — BT scan, auto-pair (Win32 `BluetoothAPIs`), ViGEm output.        |
| **Linux**           | ⚠️ Compiles. BT scanner and virtual gamepad backends are stubs.            |
| **macOS**           | ⚠️ Compiles. BT scanner and output backends are stubs.                     |

The Linux backend will use BlueZ for scanning and `uinput` for the virtual
gamepad. The macOS backend will likely start with a keyboard-mapping fallback
via `CGEvent`, since a virtual gamepad on modern macOS requires a signed
DriverKit driver.

## Build

### All platforms

Install the Rust toolchain (stable, 1.80 or later) from
[rustup.rs](https://rustup.rs).

### Windows

Prerequisites:

- A Bluetooth radio (built-in laptop BT, or a USB BT 2.1+ EDR dongle —
  CSR/Broadcom chips work most reliably with the Wiimote).
- The **ViGEmBus** driver. Download `ViGEmBus_Setup_*_x64.msi` from the
  [ViGEmBus releases page](https://github.com/nefarius/ViGEmBus/releases),
  install it, and reboot. Without it, WiiPair can still read the Wiimote
  but won't expose a virtual XInput pad to games.

Build and run:

```sh
cargo run -p wiipair-ui
```

Release build (single-binary in `target/release/wiipair.exe`):

```sh
cargo build --release -p wiipair-ui
```

To verify ViGEmBus is healthy: connect a Wiimote in WiiPair, then open
`joy.cpl` (Run → `joy.cpl`). You should see both:

- "Nintendo RVL-CNT-01" — the Wiimote as raw HID
- "Controller (XBOX 360 For Windows)" — the virtual ViGEm pad WiiPair created

If only the first one appears, ViGEmBus isn't producing the virtual pad —
reinstall it, or check for HidHide/HidGuardian conflicts.

### Linux

Install build dependencies (Debian/Ubuntu):

```sh
sudo apt install pkg-config libudev-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libgl1-mesa-dev libssl-dev
```

Build:

```sh
cargo build -p wiipair-ui
```

Current state: the build succeeds and the UI runs, but the BT scanner and
virtual gamepad output are no-ops. As a workaround, you can pair a Wiimote
manually with `bluetoothctl`; once it appears under `/dev/hidraw*`, the daemon
will pick it up and show its inputs in the UI (no virtual gamepad output yet).

### macOS

Build:

```sh
cargo build -p wiipair-ui
```

Same caveat as Linux — the platform backends are stubs. Wiimote reading via
HID works once the device is paired by macOS, but no virtual gamepad output
or auto-pair is implemented yet.

## Pairing a Wiimote (Windows)

1. Run `wiipair`.
2. Press **1+2** on the Wiimote — its 4 LEDs blink in sequence.
3. Within ~5 s, the BT scan finds it, completes the legacy-pair handshake
   (PIN = Wiimote's MAC reversed) and enables the HID service.
4. The Wiimote then appears in the UI; LED 1 lights up steady to confirm
   the host has claimed it. The first input report flips the row to
   "● connected" and a virtual Xbox 360 pad is plugged via ViGEmBus.

If auto-pair fails on a particular dongle/driver combo, fall back to manual
pairing via *Settings → Bluetooth & devices → Add device → Bluetooth*. Press
1+2 on the Wiimote when Windows prompts; choose "no PIN" / "without code".

## License

Released under the **MIT License** — see [LICENSE](LICENSE).

You are free to use, modify, redistribute, and embed WiiPair in commercial or
proprietary projects. The only requirement is including the (very short) MIT
copyright notice with substantial copies of the source.

## Acknowledgements

- [WiiBrew](https://wiibrew.org/) — protocol documentation for Wiimote and
  every extension.
- [Linux `hid-wiimote`](https://github.com/torvalds/linux/tree/master/drivers/hid)
  — reference for extension data bit layouts.
- [ViGEmBus](https://github.com/nefarius/ViGEmBus) — virtual controller
  driver that makes the XInput emulation possible.
- [`hidapi-rs`](https://github.com/ruabmbua/hidapi-rs),
  [`vigem-client`](https://github.com/CasualX/vigem-client),
  [`eframe`/`egui`](https://github.com/emilk/egui) — the Rust crates this
  project leans on.
