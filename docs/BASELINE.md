# Baseline Freeze

Baseline marker: `v0.1-working-base`

## What currently works

- Rust Windows host with TLS transport
- Flutter Android client
- Optional token auth
- Saved hosts
- Monitor-aware streaming and input
- Runtime session settings
- Clipboard send/fetch
- File transfer with checksum verification
- Panic hotkey
- Host config and diagnostics

## What still requires manual local testing

- Physical phone workflow with `adb reverse`
- Real multi-monitor switching across all target displays
- Long-running reconnect behavior on unstable networks
- Release build smoke tests on Windows host and Android APK
