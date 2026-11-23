# Gamennetto (ガメネット)

Use your phone or another PC as a second screen (extend or mirror) over Wi-Fi/LAN or USB.
Written in Rust.

## Status
Early development. Currently available:

```
cargo run -- doctor          # detect GPU, hardware encoders, network, Android tools
cargo run -- doctor --json
```

`doctor` adapts to any machine: it lists what is installed, checks that each
encoder actually starts, and prefers the encoder on the GPU that draws the screen.

## Roadmap
1. `doctor` (hardware detection) -- done
2. PC to PC mirroring (capture, encode, QUIC transport, decode)
3. Extend mode (virtual display), PC to PC
4. Android receiver (PC to phone)
5. Phone screen sharing to PC

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
