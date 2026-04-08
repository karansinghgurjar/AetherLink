# Resume Notes

## Short Summary

Built AetherLink, a secure remote desktop system with a Rust Windows host and a Flutter Android client, featuring TLS transport, optional token authentication, monitor-aware streaming/input, runtime quality controls, file transfer, and clipboard sync.

## Strong Resume Bullets

- Built a secure remote desktop platform using Rust and Flutter with TLS-encrypted transport and optional token-based session authentication.
- Designed a custom binary protocol for JPEG video streaming, control messages, clipboard sync, and chunked file transfer with SHA-256 verification.
- Implemented monitor-aware screen capture and pointer mapping across multiple displays on Windows using Win32 APIs.
- Added runtime session controls for monitor selection, frame rate, resolution, JPEG quality, and view-only mode.
- Developed host-side safety controls including a global panic hotkey and session-level input disabling.

## Interview Talking Points

- Why Rust for the host: memory safety, explicit concurrency, systems-level control
- Why Flutter for the client: fast UI iteration and Android delivery
- Protocol tradeoffs: simple framed TCP/TLS instead of WebRTC for predictable control
- Performance tradeoffs: JPEG quality, FPS, and target width tuning
- Reliability work: reconnect handling, diagnostics, checksum validation
