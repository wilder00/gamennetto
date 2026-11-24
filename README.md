# Gamennetto (ガメネット)

Use your phone or another PC as a second screen (extend or mirror) over Wi-Fi/LAN or USB.
Written in Rust.

## Status
Early development. PC to PC mirroring and extend mode work.

## Install
Download `Gamennetto-X.Y.Z-setup.exe` from
[Releases](https://github.com/wilder00/gamennetto/releases/latest) and run it
(no administrator rights needed). It adds Start menu shortcuts and puts
`gamennetto` on PATH. Update later with `gamennetto update`.
Releases are automated: see [docs/VERSIONING.md](docs/VERSIONING.md).

### Share a screen between two PCs (same Wi-Fi/LAN)
On the PC whose screen you want to share:
```
cargo run --release -- host                   # asks for quality, shows a pairing code
cargo run --release -- host --allow-control   # also let the viewer use mouse and keyboard
```
On the other PC:
```
cargo run --release -- view                   # finds the host, asks for the pairing code
cargo run --release -- view 192.168.1.20 --code ABCD-EF12-3456
```
In the viewer, like a virtual machine window:
- **Click the picture** to control the host (only if it allowed control).
  Then everything goes to the host, including Alt+Tab, the Windows key and
  Alt+F4, and the mouse stays inside the window.
- **Ctrl+Alt** (pressed and released on their own) gives your mouse and
  keyboard back to your PC. So does switching away from the window.
- **F11** full screen on/off. **Ctrl+Alt+Q** closes the viewer.
- Ctrl+Alt+Del always stays on your PC (Windows doesn't allow otherwise).
- Keys still held when the viewer lets go or disconnects are released on the host.

The first time, Windows Firewall asks whether to allow Gamennetto on the
network: allow it on **private** networks. The viewer's title bar shows fps,
delay (capture to screen) and data rate.

### Use another PC as an extra screen (extend)
Install the virtual monitor driver once (asks for administrator permission):
```
cargo run --release -- display install
```
Then share a new, extra screen instead of a copy:
```
cargo run --release -- host --extend                   # sized to the viewer's screen
cargo run --release -- host --extend --size 1920x1080
```
The viewer opens full screen (F11 switches). The extra screen appears to the
right of your screens when a viewer connects, and is removed 10 s after it
leaves, or when the host stops. If it ever stays on by mistake:
```
cargo run --release -- reset-displays     # or press Win+P -> "PC screen only"
cargo run --release -- display status
cargo run --release -- display uninstall
```

### Tools
```
cargo run -- doctor          # detect GPU, hardware encoders, network, Android tools
cargo run -- doctor --json
cargo run -- capture-test    # capture the screen for 5 s, report fps, save a screenshot
cargo run -- encode-test     # capture + hardware H.264 encode to encode-test.h264
cargo run -- encode-test --width 1920 --mbps 10   # scaled to 1080p, 10 Mbit/s
```

`doctor` adapts to any machine: it lists what is installed, checks that each
encoder actually starts, and prefers the encoder on the GPU that draws the screen.

## Roadmap
1. `doctor` (hardware detection) -- done
2. PC to PC mirroring
   - a. screen capture -- done
   - b. hardware encode (H.264, low latency) -- done (no software fallback yet)
   - c. QUIC transport, pairing code, LAN discovery -- done
   - d. viewer window (hardware decode + display) -- done
   - e. latency measurement -- done (~7-9 ms on one PC; Wi-Fi not measured yet)
   - f. guest controls the host: mouse, keyboard -- done, not yet tested between two PCs
   - next: several viewers at once, software encoder fallback
3. Extend mode (virtual display), PC to PC -- done
   - uses the open-source Virtual Display Driver (see THIRD_PARTY.md)
   - the virtual monitor is detached from the desktop when not in use; a
     restart or `reset-displays` always brings the old layout back
   - later: our own driver (needs a code-signing certificate), several
     extra screens at once
4. Desktop UI: control panel in egui (Rust only, one program); the video
   viewer stays its own low-latency window
5. Android receiver (PC to phone)
6. Phone screen sharing to PC

## Technical rules
- Use the encoders/decoders built into Windows, the GPU and Android
  (Media Foundation, MediaCodec). Software fallback: Cisco's OpenH264 binary.
  This keeps H.264/HEVC patent licensing with the platform vendors.
- Dependencies must be MIT, Apache-2.0, BSD or similarly permissive, so a
  commercial licence stays possible. No GPL dependencies (e.g. FFmpeg built with x264).
- Other projects (Sunshine, Moonlight, spacedesk, Deskreen) may be studied,
  but their code must not be copied.

## Licence
Copyright © 2026 Wilder Trujillo

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option) any
later version. See [LICENSE](LICENSE).

A commercial licence is available on request from Wilder Trujillo.

The name "Gamennetto" and its logo are reserved by Wilder Trujillo. Modified
versions must use a different name.
