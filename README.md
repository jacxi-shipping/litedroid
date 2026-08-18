# LiteDroid

LiteDroid is a Linux/WSL Android-emulator control application written in Rust. It provisions and launches the official Android Emulator backend, while providing a native CLI, daemon, and egui control panel.

## Requirements

- Linux or WSL 2 with Ubuntu
- Internet access for the Android SDK and system-image download
- Hardware virtualization enabled; WSL users need access to `/dev/kvm`
- WSLg for GUI windows on Windows 11

Native Windows builds are not supported. Run all project commands from Linux or WSL.

## First-Time Setup

From the repository root, run:

```bash
chmod +x scripts/linux/setup.sh
./scripts/linux/setup.sh
```

The script installs:

- Rust build requirements and Java
- Android SDK command-line tools
- Android Emulator and platform tools
- Android API 34 x86_64 system image
- The `LiteDroid-API34` Android Virtual Device
- Linux libraries required by the Android Emulator window

SDK files are stored under `~/.local/share/litedroid/android-sdk`.

## Run the Emulator

Create a device profile once:

```bash
./target/release/litedroid device create LiteDroid --cpu 4 --ram 2048
```

Start the graphical control panel:

```bash
./target/release/litedroid-gui
```

On startup, the GUI starts `litedroid-daemon` when it is not already running. Once it discovers a device profile, it requests a boot automatically. The daemon starts the official Android Emulator AVD.

Two windows may appear:

1. **LiteDroid**: the control panel for device lifecycle and diagnostics.
2. **Android Emulator**: the interactive Android display, including the Android-side buttons and device controls.

The LiteDroid control panel is not an embedded Android screen. Interact with Android in the separate Android Emulator window.

## CLI Workflow

Start the daemon manually when running headless:

```bash
./target/release/litedroid-daemon &
```

Then control a device:

```bash
./target/release/litedroid start --device LiteDroid
./target/release/litedroid status
./target/release/litedroid stop
```

Useful commands:

```bash
./target/release/litedroid devices
./target/release/litedroid doctor
./target/release/litedroid stats
./target/release/litedroid logs --follow
```

## Android Debug Bridge

After Android completes its first boot, check that ADB sees the device:

```bash
adb devices -l
```

Expected output includes an `emulator-5554` device in the `device` state. A first boot can initially show `offline`; leave the Android Emulator window running while it finishes setup.

If `adb` is not on your shell path, use:

```bash
~/.local/share/litedroid/android-sdk/platform-tools/adb devices -l
```

## WSL GUI Support

Verify that WSLg is active:

```bash
echo "$DISPLAY"
echo "$WAYLAND_DISPLAY"
```

Typical WSLg values are `:0` and `wayland-0`. If both are empty, update WSL from PowerShell and restart it:

```powershell
wsl --update
wsl --shutdown
```

Open a new WSL terminal afterward. The Android Emulator needs WSLg or another X11-compatible display server to show its window.

## Troubleshooting

### Android Emulator stays gray during first boot

Leave the Android Emulator running for a few minutes on its first launch. If a previous launch was interrupted, reset the AVD once:

```bash
~/.local/share/litedroid/android-sdk/emulator/emulator \
  -avd LiteDroid-API34 -wipe-data -no-snapshot -gpu swiftshader_indirect
```

`-wipe-data` deletes only the virtual device's user data; it does not redownload the system image.

### GUI reports that the daemon is offline

Start it manually and relaunch the GUI:

```bash
./target/release/litedroid-daemon &
./target/release/litedroid-gui
```

Daemon logs are written to:

```text
~/.local/share/litedroid/logs
```

### KVM is unavailable

Check the host:

```bash
ls -l /dev/kvm
groups
```

Your Linux user must be a member of the `kvm` group. On WSL, enable virtualization support in Windows and restart WSL after changing group membership.

## Development

Build and test the workspace:

```bash
cargo build --workspace
cargo test --workspace
```

The workspace contains:

- `apps/cli`: command-line interface
- `apps/daemon`: IPC service and Android Emulator lifecycle manager
- `apps/gui`: egui control panel
- `crates/*`: shared core, configuration, IPC, diagnostics, and virtualization components