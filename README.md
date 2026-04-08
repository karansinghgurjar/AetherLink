# AetherLink

AetherLink is a remote desktop system built around a Rust-based Windows host and a Flutter-based Android client. The current codebase focuses on a practical end-to-end prototype that already supports encrypted screen streaming, live input injection, per-session tuning, file transfer, clipboard sync, device trust flows, and an optional relay mode for indirect connectivity.

The project is structured as a full stack remote-control workflow:

- `host-rust`: Windows host application and server runtime
- `remote_client`: Android client built with Flutter
- `relay-rust`: optional Rust relay service for brokered sessions
- `docs`: architecture notes, baseline status, demo checklist, and resume summary
- `scripts`: convenience build scripts for host and Android release builds

## What Has Been Built So Far

The current repository is no longer a toy skeleton. It contains a working prototype with the following implemented capabilities:

- TLS-protected host/client transport
- Optional token-based authentication for direct sessions
- Relay-aware connection flow with host registration and client matchmaking
- Monitor-aware desktop capture and coordinate mapping
- Direct input control from Android to Windows
- Runtime session controls for resolution, FPS, JPEG quality, monitor selection, and view-only mode
- Incremental delta streaming in addition to full keyframes
- Clipboard fetch and clipboard push flows
- File transfer with chunk acknowledgements, cancellation support, checksum validation, and safe rename-on-conflict behavior
- Android saved-host management and preference persistence
- Pairing and trusted-device authentication foundations using Android keystore-backed signing
- Host diagnostics and client diagnostics/log export
- Panic hotkey support on the host to terminate active sessions quickly
- Initial host-side audio capture and Android audio playback plumbing

## Architecture Overview

### 1. Windows Host (`host-rust`)

The Windows host is the core execution engine. It is responsible for:

- capturing the desktop
- encoding frames as JPEG keyframes or delta updates
- exposing monitor inventory to the client
- accepting control messages over TLS
- injecting keyboard and mouse events through Win32 APIs
- handling clipboard operations
- receiving files in chunks and verifying SHA-256 integrity
- maintaining runtime session configuration
- supporting panic-stop behavior
- optionally registering with the relay service

The host can run in:

- GUI mode
- direct CLI server mode
- relay-host mode

Important host modules currently present in the source:

- `screen_capture.rs`: screen capture and monitor enumeration
- `input.rs`: Windows input injection
- `server.rs`: session lifecycle, transport, control handling, file transfer, and stream loop
- `delta_stream.rs`: delta-frame planning and payload generation
- `clipboard.rs` / `clipboard_sync.rs`: clipboard interaction and sync behavior
- `panic_hotkey.rs`: emergency disconnect handling
- `relay_client.rs`: relay-side host session integration
- `trust_store.rs`: trusted-device storage and pairing state
- `audio_capture.rs`: host audio capture path

### 2. Android Client (`remote_client`)

The Flutter app is the operator-facing controller. It handles:

- establishing a TLS connection to the host or relay
- sending session settings and control messages
- decoding keyframes and delta updates
- rendering the remote desktop stream
- sending touch-driven mouse movement and clicks
- sending keyboard shortcuts and text input
- browsing and transferring files
- handling clipboard sync and local clipboard writes
- saving host presets and user preferences
- tracking diagnostics, reconnect state, and trust/pairing status
- receiving PCM audio packets for playback on Android

The client currently includes:

- saved hosts
- reconnect handling
- diagnostics panel
- trust/pairing UI
- transfer progress and cancellation
- monitor selection UI
- clipboard mode controls
- optional relay mode

### 3. Relay Service (`relay-rust`)

The relay is a lightweight TLS broker for cases where the client should not connect to the host directly. Its current responsibilities are:

- accepting TLS connections from both hosts and clients
- registering a host by `host_id`
- validating relay access tokens
- matching a waiting client with a registered host
- forwarding framed traffic bidirectionally once the session is established

This keeps the protocol simple while making NAT-friendly or indirect session flows possible in later deployments.

## Transport and Protocol

AetherLink uses a compact framed binary protocol over TCP/TLS.

Each message is:

- `1 byte` message type
- `4 bytes` big-endian payload length
- `N bytes` payload

Current message categories in the codebase include:

- video frames
- video keyframes
- video delta frames
- control JSON messages
- audio packets

Control messages currently cover:

- auth
- session settings
- mouse movement and scrolling
- left and right click
- key down and key up
- clipboard set/get
- clipboard sync mode changes
- resync requests
- pairing requests and proofs
- trusted-device auth responses
- relay connection negotiation
- file transfer start, chunks, finish, and cancel

## Feature Detail

### Screen Streaming

The host captures the selected monitor and streams it to the client. The session can be tuned live with:

- target width
- frame rate
- JPEG quality
- monitor index
- delta-stream enable/disable
- view-only mode

The client can request a resync if delta reconstruction becomes invalid, allowing recovery without restarting the whole session.

### Input Injection

The Android app translates gestures into remote desktop actions:

- tap for click
- long press for right click
- drag for pointer motion
- mouse wheel events
- software keyboard shortcuts including common combinations such as `Ctrl+C`, `Ctrl+V`, `Ctrl+A`, arrow keys, `Alt+Tab`, and more

### File Transfer

The file transfer path is more than a raw upload. It currently includes:

- `file_start` negotiation
- chunk-level acknowledgements
- transfer cancellation
- SHA-256 verification
- file-size validation
- retry behavior on delayed acknowledgements
- safe output naming if the destination filename already exists

Received files are written to a local receive directory on the host.

### Clipboard Sync

Clipboard functionality currently supports:

- pushing client clipboard text to the host
- requesting current host clipboard text
- host-to-client automatic clipboard updates in supported mode
- duplicate suppression using sync IDs and text hashing

### Pairing and Trusted Devices

The Android client includes device identity generation and signing via platform channels and the Android keystore. The host-side flow supports:

- pair request reception
- challenge/response verification
- trusted auth challenge handling

This is the foundation for a stronger device-trust model than password/token-only access.

### Relay Mode

The Android client can optionally connect through the relay instead of directly to the host. In that mode:

- the host registers itself with a `host_id`
- the client requests that `host_id`
- the relay validates access and bridges the two streams

### Diagnostics and Safety

The codebase includes several operational features useful during development and demos:

- host session logging
- client-side connection stage tracking
- reconnect attempts and backoff tracking
- copyable client logs
- panic hotkey support on the host
- guardrails around oversized transfers and malformed sequencing

## Current Status

This repository represents a working prototype / MVP with meaningful implementation depth. The major flows already built are:

- direct host-to-client remote desktop connection
- relay-assisted connection path
- live screen streaming
- remote input
- file transfer
- clipboard interaction
- trust/pairing scaffolding
- diagnostics and operational controls

Areas that still need broader real-world validation or additional hardening:

- production-ready certificate management
- broader multi-monitor testing on diverse hardware
- long-running reliability under unstable network conditions
- more complete audio testing
- polished installer/distribution workflow
- deeper security hardening before public deployment

## Repository Layout

```text
AetherLink/
|- host-rust/        Windows host in Rust
|- relay-rust/       Relay service in Rust
|- remote_client/    Flutter Android client
|- docs/             Supporting architecture and status notes
|- scripts/          Build helper scripts
|- README.md
```

## Local Development Setup

### Prerequisites

For the Windows host:

- Rust toolchain
- Windows machine with desktop access

For the Android client:

- Flutter SDK
- Android SDK / emulator or physical device

For relay testing:

- Rust toolchain
- reachable TLS cert/key pair

### Certificates

Local development expects certificate and key files, but private certs are intentionally not committed to this repository. Generate your own local TLS assets and point the host and relay config to them.

Default sample paths in the codebase assume:

```text
certs/server.crt
certs/server.key
```

## Running the Host

From the repository root:

```powershell
cd "F:\Projects\Remote Desktop\host-rust"
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo run -- --cli 0.0.0.0:6000 --cert "../certs/server.crt" --key "../certs/server.key"
```

If you want to enable direct token auth:

```powershell
cargo run -- --cli 0.0.0.0:6000 --token "your-token" --cert "../certs/server.crt" --key "../certs/server.key"
```

## Running the Relay

```powershell
cd "F:\Projects\Remote Desktop\relay-rust"
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo run -- --addr 0.0.0.0:7000 --cert "../certs/server.crt" --key "../certs/server.key"
```

## Running the Android Client

```powershell
cd "F:\Projects\Remote Desktop\remote_client"
$env:Path = "$env:USERPROFILE\flutter\bin;$env:Path"
flutter pub get
flutter run -d emulator-5554
```

Typical emulator connection values:

- Host: `10.0.2.2`
- Port: `6000`
- Token: blank unless enabled on the host

For a physical device:

- use the host machine IP on the local network, or
- use `adb reverse` when testing locally

## Configuration

The host creates a runtime config file locally. Sample values already exist in:

- `host-rust/config.sample.json`

Current host config fields include:

- `port`
- `auth_token`
- `cert_path`
- `key_path`
- `relay_enabled`
- `relay_addr`
- `relay_host_id`
- `relay_token`
- `default_monitor_index`
- `default_fps`
- `default_jpeg_quality`
- `default_target_width`
- `download_dir`
- `panic_hotkey_enabled`

The Android client stores:

- saved host entries
- last-used connection values
- session preferences
- trust metadata

These are persisted locally through `SharedPreferences`.

## Build Commands

### Host release build

```powershell
cd "F:\Projects\Remote Desktop\host-rust"
cargo build --release
```

### Relay release build

```powershell
cd "F:\Projects\Remote Desktop\relay-rust"
cargo build --release
```

### Android release APK

```powershell
cd "F:\Projects\Remote Desktop\remote_client"
flutter build apk --release
```

## Validation Checklist

Recommended manual validation for the current codebase:

- connect directly from Android to the host
- connect through the relay path
- change monitor selection and verify correct mapping
- verify click, drag, long-press, keyboard shortcuts, and scroll input
- test live settings updates
- upload a real file and verify checksum/result
- cancel a transfer mid-stream and confirm cleanup behavior
- push clipboard text to the host
- fetch clipboard text from the host
- exercise trusted-device pairing flow
- trigger the panic hotkey and confirm the session terminates
- test reconnect handling after disconnects

## Important Notes

- The host implementation is Windows-only.
- The client is currently focused on Android.
- TLS is present, but local development often uses non-production cert handling.
- Private keys, local certs, generated outputs, and machine-specific config are intentionally excluded from version control.
- This repo should be treated as an actively evolving prototype, not yet a production-hardened remote access product.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Baseline Freeze](docs/BASELINE.md)
- [Demo Checklist](docs/DEMO_CHECKLIST.md)
- [Resume Notes](docs/RESUME.md)

## Summary

So far, AetherLink has moved well beyond an initial experiment. The codebase contains a real working remote desktop stack with a custom transport protocol, a Rust Windows control host, a Flutter Android client, relay support, transfer utilities, clipboard sync, trust scaffolding, and operational diagnostics. The next stage is mostly about hardening, testing breadth, packaging, and security refinement rather than proving the core concept.
