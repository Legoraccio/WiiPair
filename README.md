# WiiPair

![WiiPair — a Wiimote-shaped monolith on a moonscape, surrounded by curious primates with Wii instruments and a pile of controllers](assets/banner.png)

> ⚠️ **Beta software.** WiiPair handles low-level Bluetooth and virtual-driver
> plumbing on three operating systems. Some BT chipsets and OS combinations
> still produce edge-case behaviour we haven't seen yet — see
> [Troubleshooting](#troubleshooting). Bug reports with the in-app log
> attached are very welcome.

Bridges Bluetooth Wii controllers to virtual Xbox 360 pads on the desktop, so
that games like Clone Hero, Steam, or anything that speaks XInput see them as
standard controllers — no emulator required.

The original motivation: play Clone Hero with a Guitar Hero / Rock Band Wii
guitar.

## Features

- **Auto-pair** for Wiimotes via the legacy 1+2 PIN trick — no manual OS
  pairing dance.
- **Auto-(re)connect** on power-on, auto-disconnect on power-off, with a 2 s
  inactivity watchdog that catches Wiimotes going silent.
- **Up to 4 controllers** simultaneously, each assigned its own player LED
  and XInput slot.
- **Switchable mapping profiles** per device — Auto / Wiimote ↔ Xbox /
  Guitar ↔ Xplorer / Drums ↔ Xplorer / Classic ↔ Xbox, plus keyboard
  fallbacks (Wiimote ↔ Keyboard / Guitar ↔ Keyboard) for macOS or for
  games that don't speak XInput.
- **Identify pulse**: a 0.6 s rumble + LED blink to tell which physical
  Wiimote a row in the UI corresponds to.
- **Forget** removes the device from WiiPair *and* unpairs it from the OS
  Bluetooth registry, so it doesn't auto-rejoin on the next inquiry.
- **Persistent device list** — known Wiimotes appear as offline
  placeholders across restarts, with the right extension icon, and
  reconnect themselves the moment they power on.
- **Per-extension UI panels** showing live button state — frets, strum,
  whammy bar, drum pads, classic-pad layout, Nunchuk stick, plus tilt
  disc, IR-dot canvas and battery percentage.
- **Filterable log** with timestamps stamped at the moment of the event
  (not at UI read time), and a persistent banner for stack-level errors
  (ViGEmBus missing, BT scanner disabled, …).

## Supported devices

| Device                                  | Status     | Notes                                                    |
| --------------------------------------- | ---------- | -------------------------------------------------------- |
| Wii Remote (`RVL-CNT-01`)               | ✅ Full    | Buttons, accelerometer, IR camera (extended).            |
| Wii Remote Plus (`RVL-CNT-01-TR`)       | ✅ Full    | Same as Wii Remote. See [Troubleshooting](#troubleshooting) — Windows often needs the device unpaired before each session. |
| Nunchuk                                 | ✅ Full    | Analog stick, C, Z, accelerometer.                       |
| Classic Controller / Pro                | ✅ Full    | Full pad layout (A/B/X/Y, ZL/ZR, L/R, +/−, Home, D-pad). |
| Guitar Hero / Rock Band guitar (Wii)    | ✅ Full    | 5 frets, strum ↑/↓, whammy bar, +/−. Xplorer-compatible. |
| Guitar Hero / Rock Band drums (Wii)     | ✅ Full    | 5 pads + bass pedal + +/−.                               |
| DJ Hero turntable                       | ⚠️ ID only | Identified, no data parsing yet.                         |
| Wii Motion Plus                         | ⚠️ ID only | Identified, no gyro parsing yet.                         |
| uDraw Tablet, Taiko TaTaCon             | ⚠️ ID only | Identified by extension ID.                              |
| Wii Balance Board                       | ❌         | Separate Bluetooth device (not an extension).            |

## Platform support

| OS                  | Status                                                                     |
| ------------------- | -------------------------------------------------------------------------- |
| **Windows 10 / 11** | ✅ Full — BT scan, auto-pair (Win32 `BluetoothAPIs`), ViGEm output.        |
| **Linux**           | ✅ Full — BlueZ DBus scanner + auto-pair + `uinput` Xbox 360 output.       |
| **macOS**           | ⚠️ Partial — manual pairing via System Settings; CGEvent keyboard output. |

macOS is keyboard-only because publishing a real virtual XInput pad on
modern macOS requires a signed DriverKit driver, which isn't realistic
for an open-source project. The keyboard-mapping profiles cover Clone
Hero and most browser games out of the box.

## Install (pre-built binaries)

Each tagged release publishes ready-to-run archives on the GitHub
[Releases page](https://github.com/Legoraccio/WiiPair/releases). Pick
the one for your platform.

### Windows

1. Install the **ViGEmBus** driver from the
   [ViGEmBus releases page](https://github.com/nefarius/ViGEmBus/releases)
   (`ViGEmBus_Setup_*_x64.msi`). Reboot. Without it WiiPair can still
   read the Wiimote but no game will see a virtual XInput pad — and
   the app will pop a dialog at startup linking you back to the
   download page.
2. Download `wiipair-vX.Y.Z-x86_64-windows.zip` from the WiiPair
   releases page and extract anywhere (e.g. `C:\Tools\WiiPair\`).
3. Double-click `wiipair.exe`. SmartScreen will warn that the
   publisher is unknown the first time — choose *More info → Run
   anyway*. The binary is unsigned because Authenticode certificates
   are paid; the source is open and the workflow that built the
   release is in `.github/workflows/release.yml`.

### Linux

Tested on Ubuntu 22.04+, Debian 12+, Mint 21+, Bluefin / Silverblue
F38+. On older distros the bundled binary may fail to start because
of glibc version mismatches — build from source instead.

1. Install the runtime libraries:
   ```sh
   sudo apt install libdbus-1-3 libudev1 libxkbcommon0 libxkbcommon-x11-0 libgl1
   ```
   (On Fedora/Bluefin/Silverblue derivatives the equivalent packages
   are usually preinstalled with the desktop environment.)
2. Download `wiipair-vX.Y.Z-x86_64-linux.tar.gz`, extract, and install
   the udev rule shipped in the bundle:
   ```sh
   tar xzf wiipair-*-x86_64-linux.tar.gz
   cd wiipair-*-x86_64-linux
   sudo cp docs/udev/99-wiipair.rules /etc/udev/rules.d/
   sudo udevadm control --reload && sudo udevadm trigger
   sudo usermod -aG input "$USER"
   ```
3. Log out and back in (so the new `input` group sticks), then run
   `./wiipair`. If you forget the udev rule the app pops a dialog
   at startup explaining how to fix it.
4. *(Optional)* Install the desktop entry so WiiPair shows up in your
   application menu with its icon:
   ```sh
   sudo install -Dm644 docs/desktop/wiipair.png \
     /usr/share/icons/hicolor/512x512/apps/wiipair.png
   sudo install -Dm644 docs/desktop/wiipair.desktop \
     /usr/share/applications/wiipair.desktop
   sudo install -m755 wiipair /usr/local/bin/wiipair
   sudo gtk-update-icon-cache /usr/share/icons/hicolor || true
   ```
   The `.desktop` entry expects `wiipair` to be on `$PATH`; the third
   line above copies the binary into `/usr/local/bin/`. Edit `Exec=`
   in the `.desktop` file if you'd rather keep the binary elsewhere.
5. If you have the kernel `hid-wiimote` driver loaded
   (`lsmod | grep wiimote`), blacklist it so the kernel doesn't claim
   paired Wiimotes before WiiPair sees them:
   ```sh
   echo blacklist hid-wiimote | sudo tee /etc/modprobe.d/wiipair.conf
   sudo reboot
   ```

### macOS

> **Status:** keyboard-mapping output only. No pre-built bundles for
> macOS yet — build from source as documented below.

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
    libgl1-mesa-dev libssl-dev libdbus-1-dev
```

Build:

```sh
cargo build --release -p wiipair-ui
```

Install the udev rule so your user can write to `/dev/uinput` (needed for
the virtual Xbox 360 pad) and read `/dev/hidraw*` for paired Wiimotes:

```sh
sudo cp docs/udev/99-wiipair.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
sudo usermod -aG input "$USER"   # then log out and back in
```

Conflict with the kernel's `hid-wiimote` driver: the kernel claims paired
Wiimotes and exposes them as a synthetic keyboard/mouse, which conflicts
with us reading via hidapi. Either blacklist the module
(`echo blacklist hid-wiimote | sudo tee /etc/modprobe.d/wiipair.conf` then
reboot) or unbind individual devices from `/sys/bus/hid/drivers/wiimote/unbind`
after they pair.

Run:

```sh
./target/release/wiipair
```

### macOS

Build:

```sh
cargo build --release -p wiipair-ui
```

Pair the Wiimote manually via *System Settings → Bluetooth*: press 1+2 on
the Wiimote to make it discoverable, then click "Connect" next to the
"Nintendo RVL-CNT-01" entry. Once paired the WiiPair UI picks it up via
hidapi.

macOS output is **keyboard-only** (synthesised via Quartz CGEvent) — the
device card defaults to `Wiimote → Keyboard`. To enable keyboard injection,
allow the WiiPair binary under *System Settings → Privacy & Security →
Accessibility*. The default keymap targets Clone Hero / browser games:

| Wiimote button | Key       |
| -------------- | --------- |
| D-pad          | Arrow keys |
| A              | Z         |
| B              | X         |
| 1 / 2          | Q / W     |
| + / −          | Enter / Esc |
| Home           | Space     |

Guitar (Xplorer-keyboard) profile maps frets to A/S/D/F/G, strum to
arrow up/down, +/− to Enter/Esc.

## Pairing a Wiimote

### Windows / Linux (auto-pair)

1. Run `wiipair`.
2. Click **Scan for new devices (30 s)** in the top-right of the UI to
   open a 30-second discovery window.
3. Press **1+2** on the Wiimote — its 4 LEDs blink in sequence 1→2→3→4.
   Within a few seconds the BT scan finds it, completes the legacy-pair
   handshake (PIN = Wiimote's MAC reversed), and enables the HID profile.
4. The Wiimote appears in the UI; one player LED lights up steady to
   confirm the host has claimed it. The first input report flips the row
   to "● connected" and a virtual Xbox 360 pad is plugged via the
   platform's output backend.

If auto-pair fails for a particular dongle/driver combo, fall back to
manual pairing — see [Troubleshooting](#troubleshooting) below.

### macOS (manual)

macOS doesn't expose the BlueZ-style agent API that lets a user-space
app supply the legacy PIN, so pairing has to go through *System Settings →
Bluetooth*. Press 1+2 on the Wiimote, click "Connect" on the
"Nintendo RVL-CNT-01" entry that appears, and once paired the WiiPair UI
picks it up.

## Using the UI

- **Connect / Disconnect** — toggles the HID handle. Disconnect sets a
  sticky flag so auto-retry stays out of the way until you click Connect
  again.
- **Identify** — vibrates the controller for ~0.6 s with its player LED
  flashing, so you can tell which row corresponds to which physical
  device.
- **Forget** — disconnects, drops the device from the saved list, *and*
  unpairs it from the OS Bluetooth registry so it doesn't auto-rejoin.
  A confirmation dialog protects you from misclicks.
- **Profile dropdown** in the device card footer — switch the mapping
  layout on the fly. The new profile applies immediately to the
  already-plugged virtual pad.
- **Click on the MAC** in the device header — copies it to the clipboard
  (useful when you need to feed it to `bluetoothctl` or Windows BT
  settings).
- **Log filter checkboxes** — Info / Warn / Error toggle visibility per
  level. With everything unchecked the log shows all lines.

## Troubleshooting

### Wii Remote Plus (`RVL-CNT-01-TR`) and stuck-pairing recovery — Windows

The Wii Remote Plus has two known Windows-side quirks that surface
after a power-cycle:

* **Stale SDP cache** — Windows holds onto BT service entries from the
  previous session. `BluetoothSetServiceState(HID)` fails with
  `ERROR_INVALID_PARAMETER` (0x57) until the device is removed and
  re-paired.
* **Stuck auth state** — the BT registry holds a half-paired entry
  (`paired=false, connected=true`). `BluetoothAuthenticateDeviceEx`
  refuses to start with `ERROR_GEN_FAILURE` (0x1F) and Windows can
  take well over a minute to clear it on its own.

**WiiPair detects both signatures and auto-recovers.** When the
scanner spots either error *during an active "Scan for new devices"
window*, it:

1. Logs `[BT] AA:BB:…: stale SDP cache detected` *or*
   `stuck auth state detected (ERROR_GEN_FAILURE)`.
2. Calls `BluetoothRemoveDevice` to drop the stale pairing.
3. Forces an immediate re-scan so the next inquiry sees the Wiimote
   as fresh and re-pairs it from scratch using the legacy 1+2 PIN.

You just need to keep holding 1+2 on the Wiimote (the LEDs blinking in
sequence 1→2→3→4) for a couple of extra seconds while the recovery
runs. No need to open Windows BT settings.

Auto-recovery only fires while a manual scan window is active — outside
of it the daemon won't depair a "good but offline" device, since
without 1+2 held the next inquiry won't find it again. If you see one
of those log lines outside a scan window, click **Scan for new
devices** and press 1+2 to trigger the recovery.

The original Wii Remote (`RVL-CNT-01`) doesn't usually trip either
quirk on most Windows builds.

### Pairing hangs

If a Wiimote sits stuck on "*[BT] pairing …*" for 20+ seconds, WiiPair
pops a recovery dialog. The OS Bluetooth driver has wedged inside the
auth call and there's nothing user-space can do to unstick it. Follow
the steps in the dialog (toggle Bluetooth off/on in the OS, pull the
Wiimote batteries for 30 s, then re-scan). If it still hangs, close and
re-open WiiPair — a fresh process clears whatever stale state the OS BT
stack has accumulated.

### Bluetooth radio compatibility

Not every Bluetooth radio plays nicely with the Wiimote's quirky
legacy-2.0 profile. In rough order of "most reliable" to "most painful":

- **CSR / Broadcom BT 2.1+EDR USB dongles** — generally the most
  reliable for both auto-pair and sustained reporting.
- **Modern Intel AX-series adapters** — usually fine on Windows 11; some
  driver combos drop reports during inquiry windows. WiiPair pauses
  inquiry while a device is connected to mitigate this.
- **Realtek BT chipsets** — mixed; some refuse the legacy PIN. Use
  manual pairing.
- **MediaTek / no-name dongles** — often unable to complete the legacy
  pairing exchange. Try another dongle.

If your dongle keeps failing, the manual-pairing fallback (Windows BT
settings → Add device → choose "without code") works on virtually any
combo. Once Windows has paired the device, WiiPair picks it up via
hidapi without needing the auto-pair path.

### Third-party / clone Wiimotes

Hyperkin and various unbranded "Wii Remote-compatible" controllers
mostly work, but some refuse the legacy PIN exchange and need manual
pairing. A handful of clones don't expose the standard extension IDs
on the `0xa400fa` register, so extension auto-detection fails — the
controller still works as a bare Wiimote.

### "Virtual controller output unavailable" (Windows)

ViGEmBus isn't installed or the driver isn't running. WiiPair pops a
dedicated install dialog at startup (with a button to the download
page) when the probe fails outright. If the message appears later
(during a connect cycle), the daemon now retries `output_for_profile`
every ~3 s in the background and clears the red error as soon as
ViGEmBus comes back — log line `virtual gamepad ready for AA:BB:…
(recovered after retry)` confirms the recovery.

If the retry never succeeds: reinstall ViGEmBus from the
[releases page](https://github.com/nefarius/ViGEmBus/releases) and
reboot. If you have **HidHide** installed, check that it isn't hiding
the Wiimote's raw HID — that confuses ViGEm's plug routine.

### Reports stalling for ~1 s

A "*report gap: NNN ms*" warning in the log usually means the BT
controller dropped into a sniff window. WiiPair sends a 5 Hz keepalive
on every connected Wiimote to suppress this. If you still see frequent
gaps and you have multiple Wiimotes connected at once, try disabling
discovery while you're playing (the in-app inquiry pauses automatically
when at least one device is connected, but if the user is actively
clicking *Scan for new devices* during play it'll briefly steal the
radio).

### Linux: pad doesn't appear in games

Make sure your user is in the `input` group (`groups | grep input`)
and that the udev rule from `docs/udev/99-wiipair.rules` is installed.
Some games cache the controller list at startup — restart the game
after launching WiiPair.

If `hid-wiimote` is loaded, the kernel will claim the device first and
expose it as a synthetic keyboard/mouse. Either blacklist the module or
unbind the specific device from `/sys/bus/hid/drivers/wiimote/unbind`
after pairing.

### macOS: keys don't work

WiiPair needs **Accessibility** permission to inject keyboard events.
*System Settings → Privacy & Security → Accessibility* → toggle
WiiPair on. You may need to remove and re-add the binary if you've
rebuilt it, since macOS keys the permission on the binary's signature.

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
  driver that makes the Windows XInput emulation possible.
- [`hidapi-rs`](https://github.com/ruabmbua/hidapi-rs),
  [`vigem-client`](https://github.com/CasualX/vigem-client),
  [`bluer`](https://github.com/bluez/bluer) (Linux BlueZ DBus),
  [`evdev`](https://github.com/emberian/evdev) (Linux uinput),
  [`core-graphics`](https://github.com/servo/core-foundation-rs) (macOS
  CGEvent),
  [`eframe`/`egui`](https://github.com/emilk/egui) — the Rust crates this
  project leans on.
