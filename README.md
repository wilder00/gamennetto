# Gamennetto (ガメネット)

Use your phone or another PC as a second screen (extend or mirror) over Wi-Fi/LAN or USB.
Written in Rust.

## Status
Early development. PC to PC mirroring and extend mode work.

## Install
Download `Gamennetto-X.Y.Z-setup.exe` from
[Releases](https://github.com/wilder00/gamennetto/releases/latest) and run it
on both PCs (no administrator rights needed). It adds Start menu entries and
puts `gamennetto` on PATH.

**How to use it: [docs/USER_GUIDE.md](docs/USER_GUIDE.md)** (mirror, extend,
controlling the other PC, updates, troubleshooting).

## Quick reference
On the PC that shares its screen (the host):
```
gamennetto host                          # mirror; asks for quality, shows a pairing code
gamennetto host --allow-control          # also let the viewer use mouse and keyboard
gamennetto display install               # once: driver for extend mode (asks for admin)
gamennetto host --extend --allow-control # the viewer becomes an extra screen
gamennetto reset-displays                # if the extra screen ever stays on
```
On the other PC (the viewer):
```
gamennetto view                          # finds the host; the first time, approve it
gamennetto view 192.168.1.20 --code 482913
```
The first time two PCs connect, type the host's 6-digit code on the viewer, or
press Enter and approve on the host by comparing a number. After that they
remember each other and connect directly (`gamennetto devices` lists them).
In the viewer: **click** the picture to control the host, **Ctrl+Alt** to get
your mouse and keyboard back, **F11** full screen, **Ctrl+Alt+S** picture size
(fit / fill / actual), **Ctrl+Alt+Q** close.

Other commands:
```
gamennetto update            # install the newest version
gamennetto doctor            # what this PC can do: GPU, hardware encoders, network
gamennetto display status    # the virtual monitor driver
gamennetto devices           # approved PCs; `devices remove NAME` forgets one
gamennetto --help            # everything else
```

## Building from source
Needs Rust (stable, MSVC) on Windows 10/11. Any command above works as
`cargo run --release -- <command>`, e.g. `cargo run --release -- host`.
```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- capture-test    # capture the screen for 5 s, report fps, save a screenshot
cargo run -- encode-test     # capture + hardware H.264 encode to encode-test.h264
```
Releases are automated from Conventional Commits: see
[docs/VERSIONING.md](docs/VERSIONING.md).

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
