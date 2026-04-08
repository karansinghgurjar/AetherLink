#[cfg(windows)]
use anyhow::{anyhow, Context, Result};
#[cfg(windows)]
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(windows)]
use base64::Engine;
#[cfg(windows)]
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
#[cfg(windows)]
use p256::pkcs8::DecodePublicKey;
#[cfg(windows)]
use rand::RngCore;
#[cfg(windows)]
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::collections::HashMap;
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
static STORE: OnceLock<Mutex<TrustStoreState>> = OnceLock::new();

#[cfg(windows)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub device_id: String,
    pub device_name: String,
    pub public_key_pem: String,
    pub public_key_fingerprint: String,
    pub added_at: String,
    pub last_seen: String,
    pub revoked: bool,
}

#[cfg(windows)]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingPairRequestStatus {
    Pending,
    Approved,
    Rejected,
    Completed,
}

#[cfg(windows)]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingPairRequest {
    pub device_id: String,
    pub device_name: String,
    pub public_key_pem: String,
    pub public_key_fingerprint: String,
    pub client_nonce_b64: String,
    pub requested_at: String,
    pub transport: String,
    pub host_id: Option<String>,
    pub status: PendingPairRequestStatus,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct PairChallenge {
    pub device_id: String,
    pub host_nonce_b64: String,
    pub challenge_b64: String,
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct ActivePairChallenge {
    request: PendingPairRequest,
    host_nonce_b64: String,
    challenge_b64: String,
    sent: bool,
}

#[cfg(windows)]
#[derive(Default, Serialize, Deserialize)]
struct PersistedTrustStore {
    trusted_devices: Vec<TrustedDevice>,
    pending_pair_requests: Vec<PendingPairRequest>,
}

#[cfg(windows)]
#[derive(Default)]
struct TrustStoreState {
    trusted_devices: Vec<TrustedDevice>,
    pending_requests: Vec<PendingPairRequest>,
    active_challenges: Vec<ActivePairChallenge>,
}

#[cfg(windows)]
fn now_isoish() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    secs.to_string()
}

#[cfg(windows)]
pub fn path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join("trusted_devices.json")))
        .unwrap_or_else(|| PathBuf::from("trusted_devices.json"))
}

#[cfg(windows)]
fn store() -> &'static Mutex<TrustStoreState> {
    STORE.get_or_init(|| Mutex::new(load_state().unwrap_or_default()))
}

#[cfg(windows)]
fn load_state() -> Result<TrustStoreState> {
    let path = path();
    if !path.exists() {
        return Ok(TrustStoreState::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let persisted: PersistedTrustStore = serde_json::from_str(&raw).context("parse trust store json")?;
    Ok(TrustStoreState {
        trusted_devices: persisted.trusted_devices,
        pending_requests: persisted.pending_pair_requests,
        active_challenges: Vec::new(),
    })
}

#[cfg(windows)]
fn save_state(state: &TrustStoreState) -> Result<()> {
    let persisted = PersistedTrustStore {
        trusted_devices: state.trusted_devices.clone(),
        pending_pair_requests: state.pending_requests.clone(),
    };
    let raw = serde_json::to_string_pretty(&persisted).context("serialize trust store")?;
    fs::write(path(), raw).context("write trusted_devices.json")?;
    Ok(())
}

#[cfg(windows)]
fn refresh_state(state: &mut TrustStoreState) -> Result<()> {
    let active_challenges = std::mem::take(&mut state.active_challenges);
    let loaded = load_state().unwrap_or_default();
    state.trusted_devices = loaded.trusted_devices;
    state.pending_requests = loaded.pending_requests;
    state.active_challenges = active_challenges;
    Ok(())
}

#[cfg(windows)]
fn build_signed_payload(domain: &str, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(domain.as_bytes());
    for part in parts {
        payload.extend_from_slice(&(part.len() as u32).to_be_bytes());
        payload.extend_from_slice(part);
    }
    payload
}

#[cfg(windows)]
pub fn build_pair_payload(
    device_id: &str,
    client_nonce_b64: &str,
    host_nonce_b64: &str,
    challenge_b64: &str,
) -> Result<Vec<u8>> {
    Ok(build_signed_payload(
        "AETHERLINK_PAIR_V1",
        &[
            device_id.as_bytes().to_vec(),
            BASE64.decode(client_nonce_b64).context("decode client nonce")?,
            BASE64.decode(host_nonce_b64).context("decode host nonce")?,
            BASE64.decode(challenge_b64).context("decode challenge")?,
        ],
    ))
}

#[cfg(windows)]
pub fn build_auth_payload(device_id: &str, nonce_b64: &str, session_context: &str) -> Result<Vec<u8>> {
    Ok(build_signed_payload(
        "AETHERLINK_AUTH_V1",
        &[
            device_id.as_bytes().to_vec(),
            BASE64.decode(nonce_b64).context("decode auth nonce")?,
            session_context.as_bytes().to_vec(),
        ],
    ))
}

#[cfg(windows)]
fn verify_signature(public_key_pem: &str, payload: &[u8], signature_b64: &str) -> Result<()> {
    let verifying_key = VerifyingKey::from_public_key_pem(public_key_pem).context("parse public key pem")?;
    let signature_bytes = BASE64.decode(signature_b64).context("decode signature")?;
    let signature = Signature::from_der(&signature_bytes)
        .or_else(|_| Signature::try_from(signature_bytes.as_slice()))
        .map_err(|_| anyhow!("unsupported ECDSA signature format"))?;
    verifying_key.verify(payload, &signature).context("verify signature")?;
    Ok(())
}

#[cfg(windows)]
fn fingerprint_for_public_key(public_key_pem: &str) -> Result<String> {
    let verifying_key = VerifyingKey::from_public_key_pem(public_key_pem).context("parse public key pem for fingerprint")?;
    let der = verifying_key.to_encoded_point(false);
    let digest = Sha256::digest(der.as_bytes());
    Ok(format!("sha256:{:x}", digest))
}

#[cfg(windows)]
pub fn list_pending_requests() -> Vec<PendingPairRequest> {
    store()
        .lock()
        .map(|mut state| {
            let _ = refresh_state(&mut state);
            state
                .pending_requests
                .iter()
                .filter(|request| request.status == PendingPairRequestStatus::Pending)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(windows)]
pub fn list_trusted_devices() -> Vec<TrustedDevice> {
    store()
        .lock()
        .map(|mut state| {
            let _ = refresh_state(&mut state);
            state.trusted_devices.clone()
        })
        .unwrap_or_default()
}

#[cfg(windows)]
pub fn submit_pair_request(
    device_id: String,
    device_name: String,
    public_key_pem: String,
    client_nonce_b64: String,
    transport: String,
    host_id: Option<String>,
) -> Result<()> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let public_key_fingerprint = fingerprint_for_public_key(&public_key_pem)?;
    if state.trusted_devices.iter().any(|device| device.device_id == device_id && !device.revoked) {
        return Err(anyhow!("device is already trusted"));
    }
    if let Some(request) = state.pending_requests.iter_mut().find(|request| request.device_id == device_id) {
        request.device_name = device_name;
        request.public_key_pem = public_key_pem.clone();
        request.public_key_fingerprint = public_key_fingerprint;
        request.client_nonce_b64 = client_nonce_b64;
        request.transport = transport;
        request.host_id = host_id;
        request.requested_at = now_isoish();
        request.status = PendingPairRequestStatus::Pending;
        save_state(&state)?;
        return Ok(());
    }
    state.pending_requests.push(PendingPairRequest {
        device_id,
        device_name,
        public_key_pem,
        public_key_fingerprint,
        client_nonce_b64,
        requested_at: now_isoish(),
        transport,
        host_id,
        status: PendingPairRequestStatus::Pending,
    });
    save_state(&state)?;
    Ok(())
}

#[cfg(windows)]
pub fn approve_pair_request(device_id: &str) -> Result<()> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let request = state
        .pending_requests
        .iter_mut()
        .find(|request| request.device_id == device_id)
        .ok_or_else(|| anyhow!("pending request not found"))?;
    request.status = PendingPairRequestStatus::Approved;
    save_state(&state)?;
    Ok(())
}

#[cfg(windows)]
pub fn reject_pair_request(device_id: &str) -> Result<()> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let request = state
        .pending_requests
        .iter_mut()
        .find(|request| request.device_id == device_id)
        .ok_or_else(|| anyhow!("pending request not found"))?;
    request.status = PendingPairRequestStatus::Rejected;
    save_state(&state)?;
    Ok(())
}

#[cfg(windows)]
pub fn pending_request_status(device_id: &str) -> Option<PendingPairRequestStatus> {
    store()
        .lock()
        .ok()
        .and_then(|mut state| {
            let _ = refresh_state(&mut state);
            state
                .pending_requests
                .iter()
                .find(|request| request.device_id == device_id)
                .map(|request| request.status.clone())
        })
}

#[cfg(windows)]
pub fn clear_pending_request(device_id: &str) -> Result<()> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    state.pending_requests.retain(|request| request.device_id != device_id);
    save_state(&state)?;
    Ok(())
}

#[cfg(windows)]
pub fn take_approved_pair_challenge(device_id: &str) -> Option<PairChallenge> {
    let mut state = store().lock().ok()?;
    let _ = refresh_state(&mut state);
    if let Some(challenge) = state
        .active_challenges
        .iter_mut()
        .find(|challenge| challenge.request.device_id == device_id && !challenge.sent)
    {
        challenge.sent = true;
        return Some(PairChallenge {
            device_id: challenge.request.device_id.clone(),
            host_nonce_b64: challenge.host_nonce_b64.clone(),
            challenge_b64: challenge.challenge_b64.clone(),
        });
    }
    let request = state
        .pending_requests
        .iter()
        .find(|request| {
            request.device_id == device_id && request.status == PendingPairRequestStatus::Approved
        })?
        .clone();
    let mut host_nonce = [0u8; 32];
    let mut challenge = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut host_nonce);
    rand::thread_rng().fill_bytes(&mut challenge);
    state.active_challenges.push(ActivePairChallenge {
        request: request.clone(),
        host_nonce_b64: BASE64.encode(host_nonce),
        challenge_b64: BASE64.encode(challenge),
        sent: true,
    });
    let challenge = state.active_challenges.last()?;
    Some(PairChallenge {
        device_id: challenge.request.device_id.clone(),
        host_nonce_b64: challenge.host_nonce_b64.clone(),
        challenge_b64: challenge.challenge_b64.clone(),
    })
}

#[cfg(windows)]
pub fn verify_pair_proof(device_id: &str, signature_b64: &str) -> Result<TrustedDevice> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let idx = state
        .active_challenges
        .iter()
        .position(|challenge| challenge.request.device_id == device_id)
        .ok_or_else(|| anyhow!("no active challenge for device"))?;
    let challenge = state.active_challenges.remove(idx);
    let payload = build_pair_payload(
        &challenge.request.device_id,
        &challenge.request.client_nonce_b64,
        &challenge.host_nonce_b64,
        &challenge.challenge_b64,
    )?;
    verify_signature(&challenge.request.public_key_pem, &payload, signature_b64)?;
    let fingerprint = fingerprint_for_public_key(&challenge.request.public_key_pem)?;
    let now = now_isoish();
    let trusted = TrustedDevice {
        device_id: challenge.request.device_id,
        device_name: challenge.request.device_name,
        public_key_pem: challenge.request.public_key_pem,
        public_key_fingerprint: fingerprint,
        added_at: now.clone(),
        last_seen: now,
        revoked: false,
    };
    state.trusted_devices.retain(|device| device.device_id != trusted.device_id);
    state.trusted_devices.push(trusted.clone());
    state.pending_requests.retain(|request| request.device_id != trusted.device_id);
    save_state(&state)?;
    Ok(trusted)
}

#[cfg(windows)]
pub fn verify_trusted_auth(
    device_id: &str,
    nonce_b64: &str,
    session_context: &str,
    signature_b64: &str,
) -> Result<TrustedDevice> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let idx = state
        .trusted_devices
        .iter()
        .position(|device| device.device_id == device_id)
        .ok_or_else(|| anyhow!("unknown trusted device"))?;
    let device = state.trusted_devices[idx].clone();
    if device.revoked {
        return Err(anyhow!("trusted device is revoked"));
    }
    let payload = build_auth_payload(device_id, nonce_b64, session_context)?;
    verify_signature(&device.public_key_pem, &payload, signature_b64)?;
    state.trusted_devices[idx].last_seen = now_isoish();
    save_state(&state)?;
    Ok(state.trusted_devices[idx].clone())
}

#[cfg(windows)]
pub fn set_revoked(device_id: &str, revoked: bool) -> Result<()> {
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let device = state
        .trusted_devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
        .ok_or_else(|| anyhow!("trusted device not found"))?;
    device.revoked = revoked;
    save_state(&state)
}

#[cfg(windows)]
pub fn rename_device(device_id: &str, new_name: &str) -> Result<()> {
    let trimmed = new_name.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("device name cannot be empty"));
    }
    let mut state = store().lock().map_err(|_| anyhow!("trust store mutex poisoned"))?;
    refresh_state(&mut state)?;
    let device = state
        .trusted_devices
        .iter_mut()
        .find(|device| device.device_id == device_id)
        .ok_or_else(|| anyhow!("trusted device not found"))?;
    device.device_name = trimmed.to_string();
    save_state(&state)
}

#[cfg(windows)]
pub fn pending_request_count() -> usize {
    store()
        .lock()
        .map(|mut state| {
            let _ = refresh_state(&mut state);
            state
                .pending_requests
                .iter()
                .filter(|request| request.status == PendingPairRequestStatus::Pending)
                .count()
        })
        .unwrap_or(0)
}

#[cfg(windows)]
pub fn trusted_device_summary() -> HashMap<String, bool> {
    store()
        .lock()
        .map(|state| state.trusted_devices.iter().map(|device| (device.device_id.clone(), device.revoked)).collect())
        .unwrap_or_default()
}
