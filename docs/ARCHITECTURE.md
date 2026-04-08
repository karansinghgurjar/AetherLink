# AetherLink Architecture

## Components

- Flutter Android client
- Rust Windows host
- TLS socket connection over a small binary framing protocol

## Transport

- Video frames: message type `0x01`
- Control JSON: message type `0x02`
- Every message is framed as:
  - 1 byte type
  - 4 bytes big-endian payload length
  - payload bytes

## Session Flow

1. Client opens TLS socket to host.
2. Client sends auth message.
3. Host authenticates token if configured.
4. Host sends monitor inventory and session status.
5. Client sends settings message.
6. Host streams JPEG frames and accepts control events.

## Input Path

- Client normalizes touch coordinates to `[0, 1]`
- Host maps them into the selected monitor bounds
- Mouse and keyboard events are injected with Win32 `SendInput`

## File Transfer

1. Client sends `file_start` with filename, size, and SHA-256.
2. Client sends base64-encoded `file_chunk` messages.
3. Host writes chunks to disk, reports progress, verifies checksum, and emits a final result event.

## Clipboard

- `clipboard_set`: client pushes text to host clipboard
- `clipboard_get`: client requests host clipboard
- `clipboard_data`: host returns clipboard text

## Configuration

Host runtime defaults are loaded from `host-rust/config.json`.
Client host entries and last-used settings are stored in `SharedPreferences`.
