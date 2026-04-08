# AetherLink Demo Checklist

## Setup

- Start Windows host on port `6000`
- Confirm TLS cert/key paths are valid
- Launch Android emulator or connect physical device
- Open AetherLink client

## Demo Sequence

1. Show saved host entry and connect.
2. Show monitor inventory in diagnostics.
3. Stream selected monitor.
4. Tap to click, drag to move, long-press to right-click.
5. Open keyboard panel and send text plus special keys.
6. Change session settings live.
7. Send clipboard to host.
8. Fetch clipboard from host.
9. Transfer a sample file and show progress/result.
10. Trigger panic hotkey on host.
11. Show reconnect/disconnect behavior.

## Validation Targets

- No auth/TLS error in normal flow
- Correct monitor mapping
- File saved with correct checksum
- Clipboard round-trip works
- Panic hotkey terminates session
