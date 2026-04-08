# AetherLink Handoff Context

## Project Summary

- Project name: `AetherLink`
- Root path: `F:\Projects\Remote Desktop`
- Product: secure remote desktop system
- Architecture:
  - Windows host in Rust
  - Android client in Flutter
  - Relay service in Rust

## Core Security Constraints

- Pairing must remain additive on top of the existing secure channel.
- Do not remove or replace:
  - TLS pinning
  - current TLS transport
  - token auth
- Android private key must remain non-exportable in Android Keystore.
- `host-rust/src/server.rs` was previously truncated historically but has been restored; do not rewrite it blindly.

## Main Subprojects

- Host: [host-rust](/F:/Projects/Remote%20Desktop/host-rust)
- Client: [remote_client](/F:/Projects/Remote%20Desktop/remote_client)
- Relay: [relay-rust](/F:/Projects/Remote%20Desktop/relay-rust)

## Important Paths

- Host source entry points:
  - [server.rs](/F:/Projects/Remote%20Desktop/host-rust/src/server.rs)
  - [ui.rs](/F:/Projects/Remote%20Desktop/host-rust/src/ui.rs)
  - [trust_store.rs](/F:/Projects/Remote%20Desktop/host-rust/src/trust_store.rs)
  - [config.rs](/F:/Projects/Remote%20Desktop/host-rust/src/config.rs)
  - [audio_capture.rs](/F:/Projects/Remote%20Desktop/host-rust/src/audio_capture.rs)
  - [main.rs](/F:/Projects/Remote%20Desktop/host-rust/src/main.rs)
- Client source entry points:
  - [main.dart](/F:/Projects/Remote%20Desktop/remote_client/lib/main.dart)
  - [remote_client.dart](/F:/Projects/Remote%20Desktop/remote_client/lib/remote_client.dart)
  - [MainActivity.kt](/F:/Projects/Remote%20Desktop/remote_client/android/app/src/main/kotlin/com/example/remote_client/MainActivity.kt)
- Relay:
  - [relay-rust/src/main.rs](/F:/Projects/Remote%20Desktop/relay-rust/src/main.rs)
- Host binary:
  - [host-rust.exe](/F:/Projects/Remote%20Desktop/host-rust/target_host_min/debug/host-rust.exe)
- Host desktop shortcut:
  - [AetherLink Host.lnk](/C:/Users/hp/Desktop/AetherLink%20Host.lnk)
- Android APK:
  - [app-debug.apk](/F:/Projects/Remote%20Desktop/remote_client/build/app/outputs/flutter-apk/app-debug.apk)
- Relay binary:
  - [relay-rust.exe](/F:/Projects/Remote%20Desktop/relay-rust/target/debug/relay-rust.exe)
- Host trust store file:
  - [trusted_devices.json](/F:/Projects/Remote%20Desktop/host-rust/target_host_min/debug/trusted_devices.json)

## What We Are Building

AetherLink is a secure remote desktop system with:

- direct TLS host-client connectivity
- relay connectivity for NAT/firewall traversal
- Android trusted-device pairing on top of TLS + token auth
- live desktop streaming with keyframes and delta updates
- remote input control
- clipboard sync
- file transfer
- host audio loopback streaming to Android
- multi-monitor aware capture/input
- saved hosts and persisted settings
- panic hotkey and host diagnostics

## Functionalities Implemented

### Security and Session Trust

- TLS transport is in place.
- TLS pinning is in place on the client.
- token auth is still in place
- trusted-device pairing implemented on top of existing auth
- Android Keystore trust identity implemented:
  - `getOrCreateDeviceIdentity`
  - `signChallenge`
  - `forgetLocalIdentity`
- persisted host trust store implemented:
  - trusted devices
  - pending pair requests
  - revoke / unrevoke
  - rename trusted device
- trusted reconnect works
- revoked device state propagates back to the phone

### Relay

- standalone relay server exists and runs
- host outbound registration to relay works
- client outbound relay connection works
- relay session matching works
- relay session stability improved:
  - longer health timeout on client
  - less aggressive idle reconnect behavior
- host UI can manage relay mode
- host UI `Start Server` now starts:
  - direct host server
  - relay worker
  - local relay server when using a local relay address

### Streaming and Input

- keyframe streaming
- delta frame streaming
- dirty-region updates
- inferred move rects
- resync request flow
- stream telemetry
- latest-frame-wins client stream pipeline implemented:
  - one newest compressed frame pending before decode
  - one newest frame pending before render submission
  - stale frame replacement instead of FIFO playback
- client decode/render path improved:
  - heavy frame decode moved off the main isolate into a worker isolate
  - render path switched away from per-frame PNG re-encode + `Image.memory`
  - latest RGBA frame now renders through a dedicated presenter / `RawImage` path
- frame freshness instrumentation added:
  - host capture timestamp now travels with video frames
  - sampled host capture / encode / send timing logs added
  - sampled client receive / replace / decode / render timing logs added
- monitor inventory/status sync
- phone touch input works through relay
- remote landscape fullscreen mode added on phone

### Clipboard

- phone -> host clipboard works
- host -> phone clipboard retrieval works
- manual mode transport works
- auto apply to Android system clipboard is mode-dependent

### File Transfer

- file start / chunk / finish protocol exists
- unique `transfer_id` per transfer
- chunk `seq` and `offset` validation
- checksum validation
- cancel path
- retry/ack logic added for relay reliability
- latest runtime result: small file transfer completed successfully after the recent integrity/ack fixes

### Audio

- host WASAPI loopback capture implemented
- Android playback implemented via `AudioTrack`
- host-side lazy audio capture startup added
- host-side WASAPI loopback direction bug fixed
- current state: audio packets are being received on the phone and audio is audible on the phone
- current behavior: audio still plays locally on the laptop as well as on the phone; this is expected with loopback capture and is not yet a “remote-only audio output” mode

### Host UI and Config

- host UI pairing section
- host UI trusted-devices management
- host UI relay controls
- host UI scrollable layout
- host config backward-compatible with older config schema
- cert/key path normalization fixed for host UI startup
- desktop shortcut still points to the current built host executable

## Runtime Validation Status

### Verified Working

- relay connection
- stable relay session
- stream rendering
- touch/input over relay
- trusted pairing flow
- trusted reconnect
- revocation flow
- clipboard over relay
- file transfer over relay
- audio over relay
- host UI relay integration

### Important Runtime Notes

- Physical phone testing has been done through ADB reverse using:
  - `adb reverse tcp:7000 tcp:7000`
- For the phone in current test setup, relay connection uses:
  - `Host = 127.0.0.1`
  - `Port = 7000`
  - `Use Relay Mode = ON`
  - `Relay Host ID = default-host`
  - `Token = demo-token`
- If the phone suddenly reports:
  - `Connection refused`
  - `host offline`
  while relay was previously working, first verify that ADB reverse still exists.

## Recommended Testing Settings

- Host UI:
  - `Enable relay worker = checked`
  - `Relay = 127.0.0.1:7000`
  - `Relay Host ID = default-host`
- Phone:
  - `Host = 127.0.0.1`
  - `Port = 7000`
  - `Use Relay Mode = ON`
  - `Relay Host ID = default-host`
  - `Token = demo-token`
- Suggested session settings for stability:
  - resolution `Low (960)`
  - `15 FPS`
  - JPEG `60`
  - audio on/off depending on test case

## Build / Validation Commands

### Host

```powershell
cd "F:\Projects\Remote Desktop\host-rust"
& "$env:USERPROFILE\.cargo\bin\cargo.exe" check
& "$env:USERPROFILE\.cargo\bin\cargo.exe" build --bin host-rust -j 1 --target-dir target_host_min
```

### Relay

```powershell
cd "F:\Projects\Remote Desktop\relay-rust"
& "$env:USERPROFILE\.cargo\bin\cargo.exe" run -- --addr 0.0.0.0:7000 --cert "F:\Projects\Remote Desktop\certs\server.crt" --key "F:\Projects\Remote Desktop\certs\server.key"
```

### Client

```powershell
cd "F:\Projects\Remote Desktop\remote_client"
dart analyze lib\remote_client.dart lib\main.dart
flutter build apk --debug
```

### Install APK Directly To Phone

```powershell
$adb = "$env:LOCALAPPDATA\Android\Sdk\platform-tools\adb.exe"
& $adb devices
& $adb install -r "F:\Projects\Remote Desktop\remote_client\build\app\outputs\flutter-apk\app-debug.apk"
& $adb reverse tcp:7000 tcp:7000
```

## Current Workflow Conventions

- After Android-side changes:
  - rebuild APK
  - install directly to the phone if ADB is connected
- After host-side changes:
  - rebuild Windows host app
  - keep desktop shortcut pointing to the updated binary
- If relay testing breaks unexpectedly:
  - verify relay process is alive
  - verify host relay worker is alive
  - verify `adb reverse tcp:7000 tcp:7000` is still active

## Remaining Work Checklist

### Immediate Runtime Validation Still Worth Doing

- 10-15 minute long-session stability pass
- repeated audio off/on toggle test
- combined stress pass:
  - stream
  - input
  - clipboard
  - file transfer
  - audio
- save/load connection sanity recheck
- host restart / reconnect test

### Engineering / Polish Still Needed

- improve audio polish:
  - stutter/jitter tuning
  - cleaner mute/unmute behavior
  - optional remote-only audio mode if desired
- verify the new latest-frame-wins stream path under real cross-network load:
  - confirm latency reduction
  - confirm stale-frame dropping is happening as intended
  - confirm stream no longer behaves like still-image snapshots
- better relay observability:
  - clearer logs
  - clearer UI state
  - reconnect reason visibility
- benchmark capture:
  - FPS direct vs relay
  - file transfer throughput
  - reconnect timing
  - CPU usage on host
- release hardening:
  - release host build
  - release APK
  - config sanity cleanup
  - user-facing error polish

### Features Still Not Implemented or Not Fully Finished

- polished cross-network public relay deployment flow
- formal benchmark/evidence capture pack
- release packaging/tagging
- explicit user-selectable stream quality preset UX naming:
  - Responsive
  - Balanced
  - Quality
- optional host-side audio-output mode selection:
  - local only
  - remote only
  - both
- any portfolio/report/demo packaging

## Known Technical Caveats

- Physical phone relay testing currently depends on ADB reverse for local relay access.
- Manual clipboard pull does not necessarily auto-write into Android system clipboard.
- Audio currently mirrors host playback to the phone; it does not suppress host local playback.
- Relay and host are working for same-machine / same-dev-box testing; public cross-network deployment still needs a clean final deployment path.

## Good Next-Thread Starting Prompt

`Continue AetherLink from F:\Projects\Remote Desktop using HANDOFF_CONTEXT.md as source of truth. Pairing must remain layered on top of existing TLS pinning and token auth. First verify long-session stability, audio toggle stability, and combined stream/input/clipboard/file/audio stress, then move to benchmarking and release hardening.`

## Completed Checklist

### Core Product

- [x] Defined AetherLink as a Windows host + Android client remote desktop system
- [x] Established Rust host, Flutter client, and Rust relay architecture
- [x] Kept TLS transport, TLS pinning, and token auth as non-negotiable base layers

### Host Foundation

- [x] Restored and stabilized [server.rs](/F:/Projects/Remote%20Desktop/host-rust/src/server.rs)
- [x] Kept host `cargo check` passing
- [x] Built runnable Windows host binary at [host-rust.exe](/F:/Projects/Remote%20Desktop/host-rust/target_host_min/debug/host-rust.exe)
- [x] Created and maintained desktop shortcut at [AetherLink Host.lnk](/C:/Users/hp/Desktop/AetherLink%20Host.lnk)
- [x] Fixed host config backward compatibility for older `config.json`
- [x] Fixed host cert/key path resolution for UI startup

### Host UI

- [x] Added relay controls to host UI
- [x] Added pairing management UI to host UI
- [x] Added trusted device management UI to host UI
- [x] Made host UI scrollable so lower sections remain accessible
- [x] Integrated `Start Server` / `Stop Server` with relay worker lifecycle
- [x] Integrated local relay-server bring-up from host UI for local relay mode
- [x] Prevented false relay-server failure when a local relay is already running

### Relay

- [x] Added standalone relay service under [relay-rust](/F:/Projects/Remote%20Desktop/relay-rust)
- [x] Implemented host outbound relay registration
- [x] Implemented client outbound relay connect flow
- [x] Implemented relay session matching and session-ready flow
- [x] Stabilized relay liveness enough for feature testing
- [x] Verified relay-based stream connection works

### Android / Flutter Client

- [x] Added trusted identity integration via Android Keystore
- [x] Added relay connection settings and persistence in the client
- [x] Added trust state persistence in the client
- [x] Added trust / pairing panel in the client UI
- [x] Added landscape fullscreen remote-view behavior
- [x] Fixed earlier saved-host / save-current crash path
- [x] Kept latest APK buildable at [app-debug.apk](/F:/Projects/Remote%20Desktop/remote_client/build/app/outputs/flutter-apk/app-debug.apk)
- [x] Established direct APK install workflow to the physical phone over ADB

### Trusted Pairing

- [x] Implemented pair request flow
- [x] Implemented host approval flow
- [x] Implemented pair challenge / pair proof flow
- [x] Implemented trusted reconnect flow
- [x] Implemented revoke / unrevoke logic
- [x] Fixed cross-process trust visibility by persisting pending pair requests
- [x] Verified relay-mode pairing request appears in host UI
- [x] Verified approval succeeds and phone becomes trusted
- [x] Verified revoked state is reflected on reconnect

### Streaming / Input / Monitoring

- [x] Implemented keyframe streaming
- [x] Implemented delta streaming
- [x] Implemented resync request support
- [x] Implemented monitor inventory reporting
- [x] Implemented monitor-aware capture and coordinate mapping support
- [x] Added client-side touch/input logging for debugging
- [x] Verified stream rendering through relay works
- [x] Verified touch path is live enough for runtime testing

### Clipboard

- [x] Implemented phone-to-host clipboard send
- [x] Implemented host-to-phone clipboard request/pull
- [x] Implemented clipboard mode wiring
- [x] Verified clipboard transport over relay works in both directions

### File Transfer

- [x] Implemented file picker and upload flow on Android
- [x] Implemented transfer start / chunk / finish / cancel messages
- [x] Added unique `transfer_id` per upload
- [x] Added chunk `seq` and `offset` validation
- [x] Added chunk ack flow
- [x] Added retry handling for ack timeouts
- [x] Added host-side cleanup on cancel/failure
- [x] Verified successful small-file transfer over relay after integrity fixes

### Audio

- [x] Implemented Android audio playback via `AudioTrack`
- [x] Implemented host WASAPI loopback capture pipeline
- [x] Switched host audio capture to lazy start when audio is enabled
- [x] Fixed host WASAPI loopback initialization direction bug
- [x] Verified audio becomes audible on the phone

### Runtime Validation Completed

- [x] Relay connect smoke test
- [x] Stable relay streaming session test
- [x] Trusted pairing runtime test
- [x] Trusted reconnect runtime test
- [x] Revocation runtime test
- [x] Clipboard runtime test over relay
- [x] File transfer runtime test over relay
- [x] Audio runtime receipt test on phone

## Open Issues Checklist

### Runtime Stability

- [x] Run a 10-15 minute long-session relay stability pass
- [x] Verify no silent disconnects or false `host offline` states during long idle periods
- [x] Verify reconnect behavior after host restart is clean and repeatable

### Input / Control

- [x] Re-run a focused regression for mouse/touch interaction after recent relay/audio/file changes
- [x] Re-run keyboard input validation in a simple host target like Notepad
- [x] Confirm input remains correct while streaming + audio + clipboard are all active

### Audio

- [x] Re-test audio after the latest WASAPI loopback direction fix
- [x] Confirm `Host error from audio_capture: get audio capture client` no longer appears
- [x] Confirm phone logs show `audio packet received ...`
- [x] Check audio stability for 1-2 minutes continuously
- [x] Toggle audio off/on multiple times and confirm playback resumes cleanly
- [ ] Decide whether to implement a host audio output mode:
  - [ ] local only
  - [ ] remote only
  - [ ] both

### Clipboard

- [x] Re-run clipboard regression with stream and audio active
- [x] Verify host-to-phone auto clipboard mode behavior more explicitly
- [ ] Decide whether manual pull should also copy into Android system clipboard

### File Transfer

- [x] Re-run multiple small-file transfers in one session
- [x] Re-run a medium-size file transfer
- [x] Verify file transfer still works after a cancelled transfer in the same session
- [x] Verify file transfer while stream + audio are active together

### Trusted Pairing / Trust UX

- [ ] Re-run pairing approval flow after recent runtime changes
- [ ] Re-run trusted reconnect flow after recent runtime changes
- [ ] Re-run revoke -> reconnect rejection flow after recent runtime changes
- [ ] Verify host UI shows pending requests and trusted devices cleanly in all current scenarios
- [x] Consider adding clearer host UI indicators for trust-store file path and request counts

### Relay Hardening

- [x] Improve relay logging for:
  - [x] host registration
  - [x] client connect
  - [x] session creation
  - [x] session closure reason
  - [x] host removal reason
- [x] Improve host relay-worker logging for:
  - [x] ping/pong or heartbeat state
  - [x] reconnect attempts
  - [x] disconnect reasons
- [x] Validate same-network relay again after all recent fixes
- [ ] Validate cross-network relay on a public / non-ADB path

### Host UI / Packaging

- [ ] Confirm `Start Server` reliably starts direct server + relay worker + local relay server under all expected local cases
- [ ] Confirm `Stop Server` reliably stops all started child processes
- [ ] Verify the desktop shortcut still launches the latest binary after future rebuilds

### Regression / Benchmarking

- [ ] Run a combined stress pass:
  - [ ] stream
  - [ ] input
  - [ ] clipboard
  - [ ] file transfer
  - [ ] audio
- [ ] Capture benchmark data:
  - [ ] FPS direct vs relay
  - [ ] bandwidth with delta streaming
  - [ ] CPU usage on host
  - [ ] file transfer throughput
  - [ ] reconnect timing
- [ ] Capture evidence:
  - [ ] screenshots
  - [ ] logs
  - [ ] short demo clips

### Release Hardening

- [ ] Build a release-mode Windows host app
- [ ] Build a release APK
- [ ] Clean config defaults and error messages
- [ ] Re-check trusted device persistence and relay config persistence in release-style runs
