# Security Notes

## Server Certificate Fingerprint (SHA-256)

9F:E9:2B:BC:11:F7:B8:1F:8E:0B:EE:EE:39:88:C9:7E:5F:B3:CC:CB:FD:A2:BB:3B:3A:D1:B4:1B:90:EE:46:59

This fingerprint is pinned in:
- remote_client/lib/remote_client.dart (`kPinnedServerSha256Fingerprint`)

Update both places together if you rotate certificates.
