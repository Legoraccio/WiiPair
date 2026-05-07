# WiiPair

![WiiPair — a Wiimote-shaped monolith on a moonscape, surrounded by curious primates with Wii instruments and a pile of controllers](assets/banner.jpg)

Bridges Bluetooth Wii controllers (Wiimote, Nunchuk, Classic, GH/RB guitar/drums)
to virtual Xbox 360 pads on the desktop, so games like Clone Hero, Steam, or
anything that speaks XInput see them as standard controllers — no emulator
required.

> ⚠️ **Beta.** Bluetooth chipset and OS combinations still surface edge cases.
> Bug reports with the in-app log attached are welcome — see
> [Troubleshooting](#troubleshooting).

## Features

- **Auto-pair** Wiimotes via the legacy 1+2 PIN trick — no manual OS pairing.
- **Auto-(re)connect** on power-on, auto-disconnect on power-off, with a 2 s
  inactivity watchdog.
- **Up to 4 controllers** simultaneously, each with its own player LED and
  XInput slot.
- **Switchable mapping profiles** per device — Wiimote/Guitar/Drums/Classic
  → Xbox / Xplorer, plus keyboard fallbacks for macOS.
- **Identify pulse** — 0.6 s rumble + LED blink to match a UI row to a
  physical Wiimote.
- **Forget** removes the device from WiiPair *and* unpairs it from the OS.
- **Persistent device list** — known Wiimotes appear as offline placeholders
  across restarts and reconnect when powered on.
- **Live per-extension UI** — frets, strum, whammy, drum pads, classic-pad,
  Nunchuk stick, tilt disc, IR-dot canvas, battery.
- **Filterable log** with persistent banner for stack-level errors.

## Compatibility

| Device                                  | Status     | Notes                                                    |
| --------------------------------------- | ---------- | -------------------------------------------------------- |
| Wii Remote (`RVL-CNT-01`)               | ✅ Full    | Buttons, accelerometer, IR camera (extended).            |
| Wii Remote Plus (`RVL-CNT-01-TR`)       | ✅ Full    | Same as Wii Remote. Has Windows-side quirks — see [Troubleshooting](#troubleshooting). |
| Nunchuk                                 | ✅ Full    | Analog stick, C, Z, accelerometer.                       |
| Classic Controller / Pro                | ✅ Full    | Full pad layout (A/B/X/Y, ZL/ZR, L/R, +/−, Home, D-pad). |
| GH / RB guitar (Wii)                    | ✅ Full    | 5 frets, strum ↑/↓, whammy, +/−. Xplorer-compatible.     |
| GH / RB drums (Wii)                     | ✅ Full    | 5 pads + bass pedal + +/−.                               |
| DJ Hero turntable, Wii Motion Plus      | ⚠️ ID only | Identified, no data parsing yet.                         |
| uDraw Tablet, Taiko TaTaCon             | ⚠️ ID only | Identified by extension ID.                              |
| Wii Balance Board                       | ❌         | Separate Bluetooth device (not an extension).            |

| OS                  | Status                                                                  |
| ------------------- | ----------------------------------------------------------------------- |
| **Windows 10 / 11** | ✅ Full — BT scan, auto-pair (Win32 BluetoothAPIs), ViGEm output.       |
| **Linux**           | ✅ Full — BlueZ DBus auto-pair, uinput Xbox 360 output.                 |
| **macOS**           | ⚠️ Partial — manual pairing via System Settings; CGEvent keyboard only. |

macOS is keyboard-only because publishing a virtual XInput pad on modern
macOS requires a signed DriverKit driver. The keyboard profiles cover Clone
Hero and most browser games out of the box.

## Install

Pre-built archives for tagged releases live on the
[Releases page](https://github.com/Legoraccio/WiiPair/releases).

### Windows

1. Install **ViGEmBus** from the
   [ViGEmBus releases](https://github.com/nefarius/ViGEmBus/releases)
   (`ViGEmBus_Setup_*_x64.msi`) and reboot. Without it WiiPair can read
   the Wiimote but games won't see a virtual pad.
2. Download `wiipair-vX.Y.Z-x86_64-windows.zip`, extract, run `wiipair.exe`.
   SmartScreen will warn the publisher is unknown the first time
   (Authenticode certificates are paid; the source and release workflow
   are open).

### Linux

Tested on Ubuntu 22.04+, Debian 12+, Mint 21+, Bluefin/Silverblue F38+.
On older distros, build from source (glibc mismatches).

1. Install runtime libraries (Debian/Ubuntu):
   ```sh
   sudo apt install libdbus-1-3 libudev1 libxkbcommon0 libxkbcommon-x11-0 libgl1
   ```
2. Download `wiipair-vX.Y.Z-x86_64-linux.tar.gz`, extract, install the
   bundled udev rule and add yourself to the `input` group:
   ```sh
   sudo cp docs/udev/99-wiipair.rules /etc/udev/rules.d/
   sudo udevadm control --reload && sudo udevadm trigger
   sudo usermod -aG input "$USER"
   ```
   Log out and back in so the group sticks, then run `./wiipair`.
3. If `lsmod | grep wiimote` shows `hid-wiimote` is loaded, blacklist it
   so the kernel doesn't claim Wiimotes before WiiPair sees them:
   ```sh
   echo blacklist hid-wiimote | sudo tee /etc/modprobe.d/wiipair.conf
   sudo reboot
   ```
4. *(Optional)* Desktop integration — install icon, `.desktop` entry, and
   the binary to `$PATH`:
   ```sh
   sudo install -Dm644 assets/icon.png /usr/share/icons/hicolor/512x512/apps/wiipair.png
   sudo install -Dm644 docs/desktop/wiipair.desktop /usr/share/applications/
   sudo install -m755 wiipair /usr/local/bin/
   sudo gtk-update-icon-cache /usr/share/icons/hicolor || true
   ```

### macOS

No pre-built bundle yet — build from source (below). Pairing goes through
*System Settings → Bluetooth*; output is keyboard-only via CGEvent.

## Build from source

Install the Rust toolchain (stable, 1.80+) from [rustup.rs](https://rustup.rs).

```sh
cargo build --release -p wiipair-ui
```

**Linux** also needs build deps:
```sh
sudo apt install pkg-config libudev-dev libxkbcommon-dev libxkbcommon-x11-dev \
    libgl1-mesa-dev libssl-dev libdbus-1-dev
```
Then install the udev rule and (if loaded) blacklist `hid-wiimote` as in
[Install → Linux](#linux) above.

**Windows** also needs the ViGEmBus driver — see
[Install → Windows](#windows). To verify it's healthy, connect a Wiimote in
WiiPair and open `joy.cpl`; you should see both *Nintendo RVL-CNT-01* (raw
HID) and *Controller (XBOX 360 For Windows)* (the virtual ViGEm pad). If
only the first appears, reinstall ViGEmBus and check for HidHide conflicts.

**macOS** needs Accessibility permission for keyboard injection: *System
Settings → Privacy & Security → Accessibility* → toggle WiiPair on. The
default keymap (Wiimote → Keyboard profile) targets Clone Hero / browser
games:

| Wiimote button | Key         |
| -------------- | ----------- |
| D-pad          | Arrow keys  |
| A / B          | Z / X       |
| 1 / 2          | Q / W       |
| + / −          | Enter / Esc |
| Home           | Space       |

Guitar profile maps frets to A/S/D/F/G, strum to arrow up/down, +/− to
Enter/Esc.

## Pairing

**Windows / Linux (auto-pair)**: run `wiipair`, click **Scan for new devices
(30 s)**, press **1+2** on the Wiimote (LEDs blink 1→2→3→4). The scanner
finds it, completes the legacy-pair handshake (PIN = MAC reversed), enables
HID. One player LED lights up steady when the host claims it; the row flips
to "● connected" on the first input report and a virtual XInput pad is
plugged.

**macOS (manual)**: pair via *System Settings → Bluetooth*. Press 1+2 on
the Wiimote, click "Connect" on the *Nintendo RVL-CNT-01* entry; WiiPair
picks it up via hidapi.

If auto-pair fails for a particular dongle, fall back to manual pairing
through the OS Bluetooth settings — see [Troubleshooting](#troubleshooting).

## UI

- **Connect / Disconnect** toggles the HID handle. Disconnect is sticky:
  auto-retry stays off until you click Connect again.
- **Identify** rumbles + LED-flashes the device for ~0.6 s.
- **Forget** disconnects, drops the device from the saved list, *and*
  unpairs it from the OS (with confirmation).
- **Profile dropdown** in the device card footer switches mapping live —
  the new profile applies to the already-plugged virtual pad.
- **Click on the MAC** in the device header copies it to the clipboard.
- **Log filter checkboxes** — Info / Warn / Error toggle visibility per
  level; everything unchecked shows all.

## Troubleshooting

### Wii Remote Plus stuck pairing — Windows

Wii Remote Plus has two known Windows-side quirks after a power-cycle:
a stale SDP cache (`SetServiceState` returns `ERROR_INVALID_PARAMETER`)
and a stuck half-paired registry entry (`AuthenticateDeviceEx` returns
`ERROR_GEN_FAILURE`). **WiiPair detects both and auto-recovers** during
an active "Scan for new devices" window: it unpairs the stale entry and
forces a fresh inquiry. Just keep holding 1+2 on the Wiimote for a couple
of extra seconds while the recovery runs. If you see the recovery log
line outside a scan window, click *Scan for new devices* and press 1+2
to trigger it. The original Wii Remote (`RVL-CNT-01`) doesn't usually
trip these.

### Pairing hangs

If a device is stuck on "*pairing…*" for 20+ s, WiiPair pops a recovery
dialog (toggle Bluetooth off/on, pull the batteries for 30 s, re-scan).
If the dialog won't clear it, restart WiiPair — a fresh process clears
whatever stale state the OS BT stack accumulated.

### Bluetooth radio compatibility

Reliability roughly: **CSR / Broadcom 2.1+EDR USB dongles** > **Intel
AX-series** > **Realtek** > **MediaTek / no-name**. WiiPair pauses
inquiry while a device is connected to mitigate report drops. If your
dongle keeps failing auto-pair, manual pairing through the OS Bluetooth
settings (choose "without code") works on virtually any combo — once
the OS has paired the device, WiiPair picks it up via hidapi.

### Third-party / clone Wiimotes

Hyperkin and unbranded clones mostly work; some refuse the legacy PIN
(use manual pairing) and a handful don't expose standard extension IDs
(extension auto-detect fails, but the bare Wiimote still works).

### "Virtual controller output unavailable" — Windows

ViGEmBus isn't installed or running. WiiPair pops a dedicated install
dialog at startup; later disconnect/reconnect cycles retry every ~3 s
and clear the error when ViGEmBus comes back. If it never recovers,
reinstall from the [ViGEmBus releases](https://github.com/nefarius/ViGEmBus/releases)
and check for **HidHide** hiding the Wiimote's raw HID.

### Report gaps

A *report gap: NNN ms* warning means the BT controller dropped into a
sniff window. WiiPair sends a 5 Hz keepalive to suppress this. If gaps
persist with multiple Wiimotes connected, avoid clicking *Scan for new
devices* during play (it briefly steals the radio).

### Linux: pad doesn't appear in games

Check you're in the `input` group (`groups | grep input`) and the udev
rule is installed. Some games cache the controller list at startup —
restart the game after launching WiiPair. Also confirm `hid-wiimote`
isn't loaded (see [Install → Linux](#linux)).

### macOS: keys don't work

Allow WiiPair under *System Settings → Privacy & Security → Accessibility*.
Re-add the binary if you've rebuilt — macOS keys the permission to the
binary's signature.

## License

MIT — see [LICENSE](LICENSE).

## Acknowledgements

[WiiBrew](https://wiibrew.org/) for the protocol docs;
[Linux `hid-wiimote`](https://github.com/torvalds/linux/tree/master/drivers/hid)
for extension data layouts;
[ViGEmBus](https://github.com/nefarius/ViGEmBus) for the Windows virtual
pad driver; and the Rust ecosystem this leans on —
[`hidapi-rs`](https://github.com/ruabmbua/hidapi-rs),
[`vigem-client`](https://github.com/CasualX/vigem-client),
[`bluer`](https://github.com/bluez/bluer),
[`evdev`](https://github.com/emberian/evdev),
[`core-graphics`](https://github.com/servo/core-foundation-rs),
[`eframe`/`egui`](https://github.com/emilk/egui).
