#[cfg(windows)]
use anyhow::{anyhow, Context, Result};
#[cfg(windows)]
use base64::engine::general_purpose::STANDARD as BASE64;
#[cfg(windows)]
use base64::Engine;
#[cfg(windows)]
use rand::RngCore;
#[cfg(windows)]
use serde::Deserialize;
#[cfg(windows)]
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::collections::{HashMap, VecDeque};
#[cfg(windows)]
use std::fs::{self, File};
#[cfg(windows)]
use std::io::BufReader;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(windows)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(windows)]
use std::time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::TcpListener;
#[cfg(windows)]
use tokio::select;
#[cfg(windows)]
use tokio::sync::{mpsc, watch};
#[cfg(windows)]
use tokio::time::{sleep, timeout, Duration};
#[cfg(windows)]
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
#[cfg(windows)]
use tokio_rustls::{rustls, TlsAcceptor};

#[cfg(windows)]
use crate::{
    audio_capture, clipboard, delta_stream, input, panic_hotkey, protocol, screen_capture,
    session_config::{ClipboardMode, SessionConfig}, trust_store,
};

#[cfg(windows)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlMessage {
    Auth { token: String },
    Settings {
        target_width: Option<u32>,
        fps: Option<u32>,
        jpeg_quality: Option<u8>,
        view_only: Option<bool>,
        monitor_index: Option<u32>,
        clipboard_mode: Option<String>,
        delta_stream_enabled: Option<bool>,
        audio_enabled: Option<bool>,
    },
    MouseMove { x: f64, y: f64 },
    MouseScroll { delta: i32 },
    LeftClick,
    RightClick,
    KeyDown { vk: u16 },
    KeyUp { vk: u16 },
    ClipboardSet {
        text: String,
        sync_id: Option<String>,
        source: Option<String>,
    },
    ClipboardGet,
    ClipboardMode { mode: String },
    ResyncRequest,
    PairRequest {
        device_id: String,
        device_name: String,
        public_key_pem: String,
        client_nonce_b64: String,
    },
    PairProof {
        device_id: String,
        signature_b64: String,
    },
    TrustedAuth {
        device_id: String,
        nonce_b64: String,
        session_context: String,
        signature_b64: String,
    },
    RelayConnectClient {
        host_id: String,
        token: Option<String>,
        device_id: Option<String>,
    },
    FileStart {
        transfer_id: String,
        filename: String,
        size: usize,
        sha256: Option<String>,
    },
    FileChunk {
        transfer_id: String,
        seq: u64,
        offset: usize,
        data: String,
    },
    FileFinish {
        transfer_id: String,
        chunk_count: u64,
    },
    FileCancel {
        transfer_id: Option<String>,
    },
}

#[cfg(windows)]
const AUTH_TIMEOUT_SECS: u64 = 10;
#[cfg(windows)]
const MAX_CONNECTIONS_PER_WINDOW: usize = 20;
#[cfg(windows)]
const CONNECTION_WINDOW_SECS: u64 = 60;
#[cfg(windows)]
const DEFAULT_CERT_PATH: &str = "../certs/server.crt";
#[cfg(windows)]
const DEFAULT_KEY_PATH: &str = "../certs/server.key";
#[cfg(windows)]
const MAX_FILE_SIZE_BYTES: usize = 100 * 1024 * 1024;
#[cfg(windows)]
const MAX_LOG_ENTRIES: usize = 200;

#[cfg(windows)]
static SERVER_LOGS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

#[cfg(windows)]
impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert_path: DEFAULT_CERT_PATH.to_string(),
            key_path: DEFAULT_KEY_PATH.to_string(),
        }
    }
}

#[cfg(windows)]
pub fn recent_logs(limit: usize) -> Vec<String> {
    let Some(buffer) = SERVER_LOGS.get() else {
        return Vec::new();
    };
    let Ok(buffer) = buffer.lock() else {
        return Vec::new();
    };
    buffer
        .iter()
        .rev()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(windows)]
fn log_event(message: impl Into<String>) {
    let line = message.into();
    eprintln!("{line}");

    let buffer = SERVER_LOGS.get_or_init(|| Mutex::new(VecDeque::new()));
    if let Ok(mut buffer) = buffer.lock() {
        buffer.push_back(line);
        while buffer.len() > MAX_LOG_ENTRIES {
            buffer.pop_front();
        }
    }
}

#[cfg(windows)]
struct FileTransferState {
    transfer_id: String,
    file: Option<File>,
    filename: String,
    temp_path: PathBuf,
    final_path: PathBuf,
    total_size: usize,
    bytes_received: usize,
    expected_sha256: Option<String>,
    hasher: Sha256,
    last_progress_percent: u8,
    expected_seq: u64,
    last_acked_seq: Option<u64>,
    last_acked_offset: usize,
    last_acked_len: usize,
}

#[cfg(windows)]
#[derive(Default)]
struct ClipboardSyncState {
    recent_sync_ids: VecDeque<String>,
    last_applied_hash: Option<String>,
    last_applied_at: Option<Instant>,
    last_sent_hash: Option<String>,
}

#[cfg(windows)]
#[derive(Default)]
struct StreamTelemetry {
    keyframes_sent: u64,
    delta_frames_sent: u64,
    resync_requests: u64,
    inferred_move_frames: u64,
    last_patch_count: usize,
    last_move_count: usize,
    last_changed_ratio: f32,
}

#[cfg(windows)]
pub async fn run_server(addr: &str) -> Result<()> {
    run_server_with_auth_and_tls(addr, None, TlsConfig::default()).await
}

#[cfg(windows)]
pub async fn run_server_with_auth(addr: &str, auth_token: Option<String>) -> Result<()> {
    run_server_with_auth_and_tls(addr, auth_token, TlsConfig::default()).await
}

#[cfg(windows)]
pub async fn run_server_with_auth_and_tls(
    addr: &str,
    auth_token: Option<String>,
    tls: TlsConfig,
) -> Result<()> {
    let (_stop_tx, stop_rx) = watch::channel(false);
    run_server_until_with_auth(addr, stop_rx, auth_token, tls).await
}

#[cfg(windows)]
pub async fn run_server_until(addr: &str, stop_rx: watch::Receiver<bool>) -> Result<()> {
    run_server_until_with_auth(addr, stop_rx, None, TlsConfig::default()).await
}

#[cfg(windows)]
pub async fn run_server_until_with_auth(
    addr: &str,
    mut stop_rx: watch::Receiver<bool>,
    auth_token: Option<String>,
    tls: TlsConfig,
) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind {addr}"))?;
    log_event(format!("Server listening on {addr}"));

    let tls_acceptor = load_tls_acceptor(&tls).with_context(|| {
        format!(
            "Failed to load TLS config (cert: {}, key: {})",
            tls.cert_path, tls.key_path
        )
    })?;

    let normalized_auth = auth_token.and_then(|t| {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    });

    let mut accept_times: VecDeque<Instant> = VecDeque::new();
    let blocked_until_epoch_secs = Arc::new(AtomicU64::new(0));
    let (panic_tx, panic_rx) = watch::channel(0u64);
    if panic_hotkey_enabled() {
        panic_hotkey::start_panic_hotkey_thread(panic_tx, blocked_until_epoch_secs.clone());
    }

    loop {
        let now_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::from_secs(0))
            .as_secs();
        if now_epoch < blocked_until_epoch_secs.load(Ordering::SeqCst) {
            select! {
                _ = sleep(Duration::from_millis(200)) => {}
                changed = stop_rx.changed() => {
                    if changed.is_ok() && *stop_rx.borrow() {
                        break;
                    }
                }
            }
            continue;
        }

        select! {
            changed = stop_rx.changed() => {
                if changed.is_ok() && *stop_rx.borrow() {
                    break;
                }
            }
            accept_result = listener.accept() => {
                let (tcp_stream, peer_addr) = accept_result.context("Failed to accept client")?;
                if !peer_addr.ip().is_loopback() {
                    let now = Instant::now();
                    while let Some(oldest) = accept_times.front() {
                        if now.duration_since(*oldest) > StdDuration::from_secs(CONNECTION_WINDOW_SECS) {
                            accept_times.pop_front();
                        } else {
                            break;
                        }
                    }
                    if accept_times.len() >= MAX_CONNECTIONS_PER_WINDOW {
                        log_event(format!(
                            "Connection from {peer_addr} rejected by rate limit (>{} per {}s)",
                            MAX_CONNECTIONS_PER_WINDOW,
                            CONNECTION_WINDOW_SECS,
                        ));
                        continue;
                    }
                    accept_times.push_back(now);
                }

                let tls_stream = match timeout(Duration::from_secs(AUTH_TIMEOUT_SECS), tls_acceptor.accept(tcp_stream)).await {
                    Ok(Ok(stream)) => stream,
                    Ok(Err(err)) => {
                        log_event(format!("TLS handshake failed for {peer_addr}: {err}"));
                        continue;
                    }
                    Err(_) => {
                        log_event(format!("TLS handshake timed out for {peer_addr}"));
                        continue;
                    }
                };

                let client_stop_rx = stop_rx.clone();
                let client_panic_rx = panic_rx.clone();
                run_session_over_stream(
                    tls_stream,
                    peer_addr,
                    client_stop_rx,
                    client_panic_rx,
                    normalized_auth.clone(),
                )
                .await;
            }
        }
    }

    log_event(format!("Server stopped on {addr}"));
    Ok(())
}

#[cfg(windows)]
fn load_tls_acceptor(tls: &TlsConfig) -> Result<TlsAcceptor> {
    let cert_file = File::open(&tls.cert_path).with_context(|| format!("open {}", tls.cert_path))?;
    let mut cert_reader = BufReader::new(cert_file);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("read certificate chain")?;
    if cert_chain.is_empty() {
        return Err(anyhow!("certificate chain is empty"));
    }

    let key_file = File::open(&tls.key_path).with_context(|| format!("open {}", tls.key_path))?;
    let mut key_reader = BufReader::new(key_file);
    let private_key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .context("read private key")?
        .ok_or_else(|| anyhow!("no private key found"))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)
        .context("build rustls server config")?;

    Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(windows)]
pub async fn run_session_over_stream<S>(
    stream: S,
    peer_addr: std::net::SocketAddr,
    stop_rx: watch::Receiver<bool>,
    mut panic_rx: watch::Receiver<u64>,
    required_token: Option<String>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut read_half, mut write_half) = tokio::io::split(stream);
    let panic_generation = *panic_rx.borrow();

    if let Some(expected_token) = required_token {
        let auth_message = match timeout(
            Duration::from_secs(AUTH_TIMEOUT_SECS),
            protocol::read_message(&mut read_half),
        )
        .await
        {
            Ok(result) => match result {
                Ok(Some(message)) => message,
                Ok(None) => {
                    log_event(format!("Client {peer_addr} disconnected before auth"));
                    return;
                }
                Err(err) => {
                    log_event(format!("Failed reading auth from {peer_addr}: {err}"));
                    return;
                }
            },
            Err(_) => {
                log_event(format!(
                    "Client {peer_addr} failed auth: timed out after {AUTH_TIMEOUT_SECS}s"
                ));
                return;
            }
        };

        let (msg_type, payload) = auth_message;
        if msg_type != protocol::MSG_CONTROL_INPUT {
            log_event(format!("Client {peer_addr} failed auth: expected control message"));
            return;
        }

        let auth = match serde_json::from_slice::<ControlMessage>(&payload) {
            Ok(ControlMessage::Auth { token }) => token,
            Ok(_) => {
                log_event(format!(
                    "Client {peer_addr} failed auth: first control message must be auth"
                ));
                return;
            }
            Err(err) => {
                log_event(format!("Client {peer_addr} failed auth JSON parse: {err}"));
                return;
            }
        };

        if auth != expected_token {
            log_event(format!("Client {peer_addr} failed auth: token mismatch"));
            return;
        }
    }

    log_event(format!("Client {peer_addr} authenticated and connected"));

    let shared_config = Arc::new(Mutex::new(SessionConfig::from_runtime_defaults()));
    let clipboard_sync_state = Arc::new(Mutex::new(ClipboardSyncState::default()));
    let force_keyframe = Arc::new(AtomicBool::new(true));
    let mut input_stop_rx = stop_rx.clone();
    let mut write_stop_rx = stop_rx.clone();
    let mut input_panic_rx = panic_rx.clone();
    let input_config = shared_config.clone();
    let input_force_keyframe = force_keyframe.clone();
    let input_clipboard_sync_state = clipboard_sync_state.clone();
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (clipboard_event_tx, mut clipboard_event_rx) = mpsc::unbounded_channel::<()>();
    let mut audio_rx: Option<mpsc::UnboundedReceiver<Result<audio_capture::AudioPacket, String>>> =
        None;
    let mut audio_stop: Option<Arc<AtomicBool>> = None;
    let mut audio_capture_running = false;
    let _clipboard_listener = match clipboard::start_clipboard_listener(clipboard_event_tx) {
        Ok(listener) => Some(listener),
        Err(err) => {
            log_event(format!("Clipboard listener unavailable for {peer_addr}: {err}"));
            None
        }
    };
    let input_control_tx = control_tx.clone();
    let resync_requests = Arc::new(AtomicU64::new(0));
    let input_resync_requests = resync_requests.clone();
    let pending_pair_device = Arc::new(Mutex::new(None::<String>));
    let input_pending_pair_device = pending_pair_device.clone();
    let auth_nonce_b64 = random_b64(32);
    let auth_session_context = format!(
        "{}:{}:{}",
        peer_addr,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        generate_sync_id("trusted"),
    );
    let input_auth_nonce_b64 = auth_nonce_b64.clone();
    let input_auth_session_context = auth_session_context.clone();

    if let Ok(snapshot) = shared_config.lock().map(|guard| guard.clone()) {
        let _ = send_control_event(&control_tx, monitor_inventory_event());
        let _ = send_control_event(&control_tx, session_status_event(&snapshot));
    }
    let _ = send_control_event(
        &control_tx,
        serde_json::json!({
            "type": "trusted_auth_challenge",
            "nonce_b64": auth_nonce_b64,
            "session_context": auth_session_context,
        }),
    );

    let input_task = tokio::spawn(async move {
        let mut file_transfers: HashMap<String, FileTransferState> = HashMap::new();
        loop {
            let message = match select! {
                changed = input_stop_rx.changed() => {
                    if changed.is_ok() && *input_stop_rx.borrow() {
                        return;
                    }
                    continue;
                }
                changed = input_panic_rx.changed() => {
                    if changed.is_ok() && *input_panic_rx.borrow() != panic_generation {
                        log_event(format!("[panic] input loop terminated for {peer_addr}"));
                        return;
                    }
                    continue;
                }
                msg = protocol::read_message(&mut read_half) => msg
            } {
                Ok(Some(message)) => message,
                Ok(None) => {
                    log_event(format!("Client {peer_addr} closed input stream"));
                    break;
                }
                Err(err) => {
                    log_event(format!("Failed reading from {peer_addr}: {err}"));
                    break;
                }
            };

            let (msg_type, payload) = message;
            if msg_type != protocol::MSG_CONTROL_INPUT {
                continue;
            }

            let command: ControlMessage = match serde_json::from_slice(&payload) {
                Ok(cmd) => cmd,
                Err(err) => {
                    log_event(format!("Invalid control JSON from {peer_addr}: {err}"));
                    continue;
                }
            };

            let mut cfg_guard = match input_config.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    log_event("Session config mutex poisoned");
                    break;
                }
            };

            match command {
                ControlMessage::Auth { .. } => {}
                ControlMessage::Settings {
                    target_width,
                    fps,
                    jpeg_quality,
                    view_only,
                    monitor_index,
                    clipboard_mode,
                    delta_stream_enabled,
                    audio_enabled,
                } => {
                    let parsed_clipboard_mode =
                        clipboard_mode.as_deref().and_then(ClipboardMode::from_wire);
                    cfg_guard.apply_partial(
                        target_width,
                        fps,
                        jpeg_quality,
                        view_only,
                        monitor_index,
                        parsed_clipboard_mode,
                        delta_stream_enabled,
                        audio_enabled,
                    );
                    let snapshot = cfg_guard.clone();
                    drop(cfg_guard);
                    input_force_keyframe.store(true, Ordering::SeqCst);
                    log_event(format!(
                        "Session settings updated for {peer_addr}: monitor {}, {} FPS, JPEG {}, width {}, view_only={}, clipboard_mode={}, delta_stream={}, audio={}",
                        snapshot.monitor_index,
                        snapshot.fps,
                        snapshot.jpeg_quality,
                        snapshot.target_width.map(|value| value.to_string()).unwrap_or_else(|| "native".to_string()),
                        snapshot.view_only,
                        snapshot.clipboard_mode.as_wire(),
                        snapshot.delta_stream_enabled,
                        snapshot.audio_enabled,
                    ));
                    let _ = send_control_event(&input_control_tx, session_status_event(&snapshot));
                }
                ControlMessage::ClipboardSet { text, sync_id, source } => {
                    drop(cfg_guard);
                    let text_hash = hash_clipboard_text(&text);
                    {
                        let mut state = match input_clipboard_sync_state.lock() {
                            Ok(state) => state,
                            Err(_) => {
                                log_event("Clipboard sync state mutex poisoned");
                                continue;
                            }
                        };
                        if let Some(sync_id) = sync_id.as_deref() {
                            if state.recent_sync_ids.iter().any(|known| known == sync_id) {
                                log_event(format!(
                                    "Clipboard update from {peer_addr} ignored: duplicate sync_id={sync_id}"
                                ));
                                continue;
                            }
                            remember_sync_id(&mut state.recent_sync_ids, sync_id.to_string());
                        }
                        state.last_applied_hash = Some(text_hash);
                        state.last_applied_at = Some(Instant::now());
                    }
                    if let Err(err) = clipboard::set_clipboard_text(&text) {
                        log_event(format!("Failed to set clipboard: {err}"));
                    } else {
                        log_event(format!(
                            "Clipboard updated from {peer_addr} ({} chars, source={})",
                            text.len(),
                            source.unwrap_or_else(|| "client".to_string())
                        ));
                    }
                }
                ControlMessage::ClipboardGet => {
                    drop(cfg_guard);
                    match clipboard::get_clipboard_text() {
                        Ok(Some(text)) => {
                            let sync_id = generate_sync_id("host");
                            if let Ok(mut state) = input_clipboard_sync_state.lock() {
                                remember_sync_id(&mut state.recent_sync_ids, sync_id.clone());
                                state.last_sent_hash = Some(hash_clipboard_text(&text));
                            }
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "clipboard_data",
                                    "text": text,
                                    "source": "host",
                                    "sync_id": sync_id,
                                }),
                            );
                            log_event(format!("Clipboard requested by {peer_addr}"));
                        }
                        Ok(None) => {
                            log_event(format!(
                                "Clipboard requested by {peer_addr} but clipboard had no text"
                            ));
                        }
                        Err(err) => {
                            log_event(format!("Failed to read clipboard: {err}"));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "host_error",
                                    "source": "clipboard_get",
                                    "message": err.to_string(),
                                }),
                            );
                        }
                    }
                }
                ControlMessage::ClipboardMode { mode } => {
                    let parsed = ClipboardMode::from_wire(&mode);
                    if let Some(mode) = parsed {
                        cfg_guard.clipboard_mode = mode;
                        let snapshot = cfg_guard.clone();
                        drop(cfg_guard);
                        let _ = send_control_event(&input_control_tx, session_status_event(&snapshot));
                        log_event(format!(
                            "Clipboard mode updated for {peer_addr}: {}",
                            snapshot.clipboard_mode.as_wire()
                        ));
                    } else {
                        drop(cfg_guard);
                        let _ = send_control_event(
                            &input_control_tx,
                            serde_json::json!({
                                "type": "host_error",
                                "source": "clipboard_mode",
                                "message": format!("Unsupported clipboard mode: {mode}"),
                            }),
                        );
                    }
                }
                ControlMessage::ResyncRequest => {
                    drop(cfg_guard);
                    input_force_keyframe.store(true, Ordering::SeqCst);
                    input_resync_requests.fetch_add(1, Ordering::SeqCst);
                    log_event(format!("Client {peer_addr} requested frame resync"));
                }
                ControlMessage::PairRequest {
                    device_id,
                    device_name,
                    public_key_pem,
                    client_nonce_b64,
                } => {
                    drop(cfg_guard);
                    log_event(format!("Pair request from {peer_addr}: {device_name} ({device_id})"));
                    match trust_store::submit_pair_request(
                        device_id.clone(),
                        device_name,
                        public_key_pem,
                        client_nonce_b64,
                        "direct_or_relay".to_string(),
                        None,
                    ) {
                        Ok(_) => {
                            if let Ok(mut current) = input_pending_pair_device.lock() {
                                *current = Some(device_id.clone());
                            }
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "pair_result",
                                    "ok": false,
                                    "message": "waiting_for_host_approval",
                                    "device_id": device_id,
                                }),
                            );
                        }
                        Err(err) => {
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "pair_result",
                                    "ok": false,
                                    "message": err.to_string(),
                                    "device_id": device_id,
                                }),
                            );
                        }
                    }
                }
                ControlMessage::PairProof {
                    device_id,
                    signature_b64,
                } => {
                    drop(cfg_guard);
                    match trust_store::verify_pair_proof(&device_id, &signature_b64) {
                        Ok(device) => {
                            log_event(format!(
                                "Trusted device paired for {peer_addr}: {} ({})",
                                device.device_name, device.device_id
                            ));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "pair_result",
                                    "ok": true,
                                    "message": "trusted",
                                    "device_id": device.device_id,
                                    "device_name": device.device_name,
                                    "fingerprint": device.public_key_fingerprint,
                                }),
                            );
                        }
                        Err(err) => {
                            log_event(format!(
                                "Pair proof failed for {peer_addr}: {device_id}: {err}"
                            ));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "pair_result",
                                    "ok": false,
                                    "message": err.to_string(),
                                    "device_id": device_id,
                                }),
                            );
                        }
                    }
                }
                ControlMessage::TrustedAuth {
                    device_id,
                    nonce_b64,
                    session_context,
                    signature_b64,
                } => {
                    drop(cfg_guard);
                    if nonce_b64 != input_auth_nonce_b64
                        || session_context != input_auth_session_context
                    {
                        let _ = send_control_event(
                            &input_control_tx,
                            serde_json::json!({
                                "type": "trusted_auth_result",
                                "ok": false,
                                "device_id": device_id,
                                "message": "challenge mismatch",
                            }),
                        );
                        continue;
                    }
                    match trust_store::verify_trusted_auth(
                        &device_id,
                        &nonce_b64,
                        &session_context,
                        &signature_b64,
                    ) {
                        Ok(device) => {
                            log_event(format!(
                                "Trusted auth accepted for {peer_addr}: {} ({})",
                                device.device_name, device.device_id
                            ));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "trusted_auth_result",
                                    "ok": true,
                                    "device_id": device.device_id,
                                    "device_name": device.device_name,
                                    "message": "trusted_auth_ok",
                                }),
                            );
                        }
                        Err(err) => {
                            let revoked = err.to_string().contains("revoked");
                            log_event(format!(
                                "Trusted auth rejected for {peer_addr}: {device_id}: {err}"
                            ));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "trusted_auth_result",
                                    "ok": false,
                                    "device_id": device_id,
                                    "revoked": revoked,
                                    "message": err.to_string(),
                                }),
                            );
                        }
                    }
                }
                ControlMessage::RelayConnectClient {
                    host_id,
                    token,
                    device_id,
                } => {
                    drop(cfg_guard);
                    log_event(format!("Relay connect request from {peer_addr} for host {host_id}"));
                    let _ = send_control_event(
                        &input_control_tx,
                        serde_json::json!({
                            "type": "host_error",
                            "source": "relay_connect_client",
                            "message": format!(
                                "Relay/NAT traversal is scaffolded but not yet active for host {host_id} (token_present={}, device_id={})",
                                token.as_ref().map(|t| !t.is_empty()).unwrap_or(false),
                                device_id.unwrap_or_else(|| "unknown".to_string())
                            ),
                        }),
                    );
                }
                ControlMessage::FileStart {
                    transfer_id,
                    filename,
                    size,
                    sha256,
                } => {
                    drop(cfg_guard);
                    let expected_sha256_for_log = sha256.clone();
                    if let Some(existing) = file_transfers.remove(&transfer_id) {
                        cleanup_file_transfer(existing);
                    }
                    match start_file_transfer(&transfer_id, &filename, size, sha256) {
                        Ok(state) => {
                            let started_path = state.final_path.display().to_string();
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "file_transfer_started",
                                    "transfer_id": transfer_id,
                                    "filename": filename,
                                    "saved_path": started_path,
                                    "size": size,
                                }),
                            );
                            file_transfers.insert(transfer_id.clone(), state);
                            log_event(format!(
                                "File transfer started from {peer_addr}: transfer_id={transfer_id} filename={filename} expected_size={size} expected_sha256={}",
                                expected_sha256_for_log.as_deref().unwrap_or("none")
                            ));
                        }
                        Err(err) => {
                            log_event(format!("Failed to start file transfer: {err}"));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "file_transfer_result",
                                    "success": false,
                                    "transfer_id": transfer_id,
                                    "error": err.to_string(),
                                }),
                            );
                        }
                    }
                }
                ControlMessage::FileChunk {
                    transfer_id,
                    seq,
                    offset,
                    data,
                } => {
                    drop(cfg_guard);
                    match file_transfers.get_mut(&transfer_id) {
                        Some(state) => {
                            let decoded = match BASE64.decode(data.as_bytes()) {
                                Ok(bytes) => bytes,
                                Err(err) => {
                                    log_event(format!(
                                        "Failed to decode file chunk: transfer_id={transfer_id} seq={seq} offset={offset} err={err}"
                                    ));
                                    if let Some(state) = file_transfers.remove(&transfer_id) {
                                        cleanup_file_transfer(state);
                                    }
                                    continue;
                                }
                            };
                            log_event(format!(
                                "File chunk received: transfer_id={transfer_id} seq={seq} offset={offset} raw_len={} bytes_received_before={}",
                                decoded.len(),
                                state.bytes_received
                            ));
                            let is_duplicate_retry = state.last_acked_seq == Some(seq)
                                && state.last_acked_offset == offset
                                && state.last_acked_len == decoded.len()
                                && state.expected_seq == seq + 1;
                            if is_duplicate_retry {
                                log_event(format!(
                                    "File chunk retry detected: transfer_id={transfer_id} seq={seq} offset={offset} bytes_received_current={}",
                                    state.bytes_received
                                ));
                                let _ = send_control_event(
                                    &input_control_tx,
                                    serde_json::json!({
                                        "type": "file_chunk_ack",
                                        "transfer_id": transfer_id,
                                        "seq": seq,
                                        "offset": offset,
                                        "bytes_received": state.bytes_received,
                                        "duplicate": true,
                                    }),
                                );
                                continue;
                            }
                            if let Err(err) = write_file_chunk(state, seq, offset, &decoded) {
                                log_event(format!("Failed writing file chunk: transfer_id={transfer_id} err={err}"));
                                let _ = send_control_event(
                                    &input_control_tx,
                                    serde_json::json!({
                                        "type": "file_transfer_result",
                                        "success": false,
                                        "transfer_id": transfer_id,
                                        "filename": state.filename,
                                        "error": err.to_string(),
                                    }),
                                );
                                if let Some(state) = file_transfers.remove(&transfer_id) {
                                    cleanup_file_transfer(state);
                                }
                                continue;
                            }
                            log_event(format!(
                                "File chunk accepted: transfer_id={transfer_id} seq={seq} offset={offset} bytes_received_after={}",
                                state.bytes_received
                            ));
                            let _ = send_control_event(
                                &input_control_tx,
                                serde_json::json!({
                                    "type": "file_chunk_ack",
                                    "transfer_id": transfer_id,
                                    "seq": seq,
                                    "offset": offset,
                                    "bytes_received": state.bytes_received,
                                }),
                            );
                            if let Some(percent) = current_progress_percent(state) {
                                if percent != state.last_progress_percent {
                                    state.last_progress_percent = percent;
                                    let _ = send_control_event(
                                        &input_control_tx,
                                        serde_json::json!({
                                            "type": "file_transfer_progress",
                                            "transfer_id": transfer_id,
                                            "filename": state.filename,
                                            "progress_percent": percent,
                                        }),
                                    );
                                }
                            }
                        }
                        None => {
                            log_event(format!(
                                "Ignoring file chunk from {peer_addr}: unknown transfer_id={transfer_id} seq={seq} offset={offset}"
                            ));
                        }
                    }
                }
                ControlMessage::FileFinish {
                    transfer_id,
                    chunk_count,
                } => {
                    drop(cfg_guard);
                    match file_transfers.remove(&transfer_id) {
                        Some(mut state) => {
                            log_event(format!(
                                "File transfer finish received: transfer_id={transfer_id} chunk_count={chunk_count} expected_size={} actual_size={} expected_sha256={} expected_seq={}",
                                state.total_size,
                                state.bytes_received,
                                state.expected_sha256.clone().unwrap_or_else(|| "none".to_string()),
                                state.expected_seq
                            ));
                            match finalize_file_transfer(&mut state) {
                                Ok(result) => {
                                    log_event(format!(
                                        "File transfer completed: transfer_id={} filename={} bytes_received={} checksum_ok={}",
                                        result.transfer_id, result.filename, result.bytes_received, result.checksum_ok
                                    ));
                                    let _ = send_control_event(
                                        &input_control_tx,
                                        serde_json::json!({
                                            "type": "file_transfer_result",
                                            "success": result.success,
                                            "transfer_id": result.transfer_id,
                                            "filename": result.filename,
                                            "saved_path": result.saved_path,
                                            "bytes_received": result.bytes_received,
                                            "checksum_ok": result.checksum_ok,
                                            "error": result.error,
                                        }),
                                    );
                                }
                                Err(err) => {
                                    log_event(format!("Failed finalizing transfer: transfer_id={transfer_id} err={err}"));
                                    cleanup_file_transfer(state);
                                    let _ = send_control_event(
                                        &input_control_tx,
                                        serde_json::json!({
                                            "type": "file_transfer_result",
                                            "success": false,
                                            "transfer_id": transfer_id,
                                            "error": err.to_string(),
                                        }),
                                    );
                                }
                            }
                        }
                        None => {
                            log_event(format!(
                                "Ignoring file finish from {peer_addr}: unknown transfer_id={transfer_id}"
                            ));
                        }
                    }
                }
                ControlMessage::FileCancel { transfer_id } => {
                    drop(cfg_guard);
                    let transfer_id_for_result = transfer_id.clone();
                    if let Some(transfer_id) = transfer_id {
                        if let Some(state) = file_transfers.remove(&transfer_id) {
                            cleanup_file_transfer(state);
                            log_event(format!("File transfer cancelled: transfer_id={transfer_id}"));
                        }
                    } else {
                        for (_, state) in file_transfers.drain() {
                            cleanup_file_transfer(state);
                        }
                        log_event(format!("File transfer cancelled: all active transfers for {peer_addr}"));
                    }
                    let _ = send_control_event(
                        &input_control_tx,
                        serde_json::json!({
                            "type": "file_transfer_result",
                            "success": false,
                            "transfer_id": transfer_id_for_result,
                            "error": "cancelled",
                        }),
                    );
                }
                ControlMessage::MouseMove { x, y } => {
                    let view_only = cfg_guard.view_only;
                    let monitor_index = cfg_guard.monitor_index;
                    drop(cfg_guard);
                    if !view_only {
                        match input::normalized_to_monitor_point(x, y, monitor_index) {
                            Ok((screen_x, screen_y)) => log_event(format!(
                                "Input received from {peer_addr}: mouse_move rel=({x:.3}, {y:.3}) monitor={monitor_index} screen=({screen_x}, {screen_y})"
                            )),
                            Err(err) => log_event(format!(
                                "Input received from {peer_addr}: mouse_move rel=({x:.3}, {y:.3}) monitor={monitor_index} map_failed={err}"
                            )),
                        }
                        if let Err(err) = input::move_mouse_normalized_on_monitor(x, y, monitor_index) {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        }
                    } else {
                        log_event(format!("Input ignored for {peer_addr}: mouse_move while view_only=true"));
                    }
                }
                ControlMessage::MouseScroll { delta } => {
                    let view_only = cfg_guard.view_only;
                    drop(cfg_guard);
                    if !view_only {
                        log_event(format!("Input received from {peer_addr}: mouse_scroll delta={delta}"));
                        if let Err(err) = input::mouse_scroll(delta) {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        }
                    } else {
                        log_event(format!("Input ignored for {peer_addr}: mouse_scroll while view_only=true"));
                    }
                }
                ControlMessage::LeftClick => {
                    let view_only = cfg_guard.view_only;
                    drop(cfg_guard);
                    if !view_only {
                        log_event(format!("Input received from {peer_addr}: left_click"));
                        if let Err(err) = input::left_click() {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        } else {
                            log_event(format!("Input dispatch succeeded for {peer_addr}: left_click"));
                        }
                    } else {
                        log_event(format!("Input ignored for {peer_addr}: left_click while view_only=true"));
                    }
                }
                ControlMessage::RightClick => {
                    let view_only = cfg_guard.view_only;
                    drop(cfg_guard);
                    if !view_only {
                        log_event(format!("Input received from {peer_addr}: right_click"));
                        if let Err(err) = input::right_click() {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        } else {
                            log_event(format!("Input dispatch succeeded for {peer_addr}: right_click"));
                        }
                    } else {
                        log_event(format!("Input ignored for {peer_addr}: right_click while view_only=true"));
                    }
                }
                ControlMessage::KeyDown { vk } => {
                    let view_only = cfg_guard.view_only;
                    drop(cfg_guard);
                    if !view_only {
                        if let Err(err) = input::send_key_down(vk) {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        }
                    }
                }
                ControlMessage::KeyUp { vk } => {
                    let view_only = cfg_guard.view_only;
                    drop(cfg_guard);
                    if !view_only {
                        if let Err(err) = input::send_key_up(vk) {
                            log_event(format!("Input dispatch failed for {peer_addr}: {err}"));
                        }
                    }
                }
            }
        }
    });

    let mut last_rgba = Vec::new();
    let mut last_width = 0u32;
    let mut last_height = 0u32;
    let mut last_frame_id = 0u32;
    let mut next_frame_id = 1u32;
    let mut frames_since_keyframe = 0u32;
    let mut idle_frame_multiplier: u64 = 1;
    let mut telemetry = StreamTelemetry::default();
    let mut last_telemetry_sent_at = Instant::now();

    'write_loop: loop {
        if *write_stop_rx.borrow() {
            break;
        }

        if *panic_rx.borrow() != panic_generation {
            log_event(format!("[panic] session terminated by hotkey for {peer_addr}"));
            break;
        }

        let config_snapshot = match shared_config.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                log_event("Session config mutex poisoned");
                break;
            }
        };

        if config_snapshot.audio_enabled {
            if !audio_capture_running {
                let (fresh_audio_tx, fresh_audio_rx) =
                    mpsc::unbounded_channel::<Result<audio_capture::AudioPacket, String>>();
                let fresh_audio_stop = Arc::new(AtomicBool::new(false));
                let _audio_thread =
                    audio_capture::start_loopback_capture(fresh_audio_tx, fresh_audio_stop.clone());
                audio_rx = Some(fresh_audio_rx);
                audio_stop = Some(fresh_audio_stop);
                audio_capture_running = true;
                log_event(format!("Audio capture started for {peer_addr}"));
            }
        } else if audio_capture_running {
            if let Some(stop_flag) = audio_stop.take() {
                stop_flag.store(true, Ordering::SeqCst);
            }
            audio_rx = None;
            audio_capture_running = false;
            log_event(format!("Audio capture stopped for {peer_addr}"));
        }

        let (width, height, rgba) =
            match screen_capture::capture_desktop_rgba_with_config(&config_snapshot) {
                Ok(frame) => frame,
                Err(err) => {
                    log_event(format!("Capture failed: {err}"));
                    break;
                }
            };

        let force_keyframe_now = force_keyframe.swap(false, Ordering::SeqCst);
        let maybe_plan = if !force_keyframe_now
            && config_snapshot.delta_stream_enabled
            && width == last_width
            && height == last_height
            && !last_rgba.is_empty()
        {
            delta_stream::detect_delta_plan(&last_rgba, &rgba, width, height)
        } else {
            None
        };

        let need_keyframe = force_keyframe_now
            || !config_snapshot.delta_stream_enabled
            || width != last_width
            || height != last_height
            || last_rgba.is_empty()
            || frames_since_keyframe >= 60
            || maybe_plan
                .as_ref()
                .map(|plan| {
                    delta_stream::changed_ratio(plan, width, height) > 0.45
                        || plan.patches.len() > 24
                        || plan.moves.len() > 48
                })
                .unwrap_or(false);

        let frame_changed = need_keyframe || maybe_plan.is_some();
        if frame_changed {
            idle_frame_multiplier = 1;
        } else {
            idle_frame_multiplier = (idle_frame_multiplier + 1).min(4);
        }

        if need_keyframe {
            let payload = match delta_stream::encode_keyframe_payload(
                next_frame_id,
                width,
                height,
                &rgba,
                config_snapshot.jpeg_quality,
            ) {
                Ok(payload) => payload,
                Err(err) => {
                    log_event(format!("Keyframe encode failed: {err}"));
                    break;
                }
            };
            if let Err(err) =
                protocol::write_message(&mut write_half, protocol::MSG_VIDEO_KEYFRAME, &payload).await
            {
                log_event(format!(
                    "Client {peer_addr} disconnected while sending keyframe: {err}"
                ));
                break;
            }
            telemetry.keyframes_sent += 1;
            telemetry.last_patch_count = 0;
            telemetry.last_move_count = 0;
            telemetry.last_changed_ratio = 1.0;
            last_frame_id = next_frame_id;
            next_frame_id = next_frame_id.wrapping_add(1).max(1);
            frames_since_keyframe = 0;
            last_width = width;
            last_height = height;
            last_rgba = rgba;
        } else if let Some(plan) = maybe_plan {
            let changed_ratio = delta_stream::changed_ratio(&plan, width, height);
            let move_count = plan.moves.len();
            let patch_count = plan.patches.len();
            let payload = match delta_stream::encode_delta_payload(
                next_frame_id,
                last_frame_id,
                width,
                height,
                &rgba,
                &plan,
                config_snapshot.jpeg_quality,
            ) {
                Ok(payload) => payload,
                Err(err) => {
                    log_event(format!("Delta encode failed: {err}"));
                    break;
                }
            };
            if let Err(err) =
                protocol::write_message(&mut write_half, protocol::MSG_VIDEO_DELTA, &payload).await
            {
                log_event(format!(
                    "Client {peer_addr} disconnected while sending delta frame: {err}"
                ));
                break;
            }
            telemetry.delta_frames_sent += 1;
            telemetry.last_patch_count = patch_count;
            telemetry.last_move_count = move_count;
            telemetry.last_changed_ratio = changed_ratio;
            if move_count > 0 {
                telemetry.inferred_move_frames += 1;
            }
            last_frame_id = next_frame_id;
            next_frame_id = next_frame_id.wrapping_add(1).max(1);
            frames_since_keyframe += 1;
            last_width = width;
            last_height = height;
            last_rgba = rgba;
        }

        while clipboard_event_rx.try_recv().is_ok() {
            if !config_snapshot.clipboard_mode.host_push_enabled() {
                continue;
            }
            match clipboard::get_clipboard_text() {
                Ok(Some(text)) => {
                    let text_hash = hash_clipboard_text(&text);
                    let should_send = {
                        let mut state = match clipboard_sync_state.lock() {
                            Ok(state) => state,
                            Err(_) => break,
                        };
                        let suppressed = state
                            .last_applied_hash
                            .as_ref()
                            .map(|hash| hash == &text_hash)
                            .unwrap_or(false)
                            && state
                                .last_applied_at
                                .map(|at| at.elapsed() <= StdDuration::from_secs(2))
                                .unwrap_or(false);
                        if suppressed || state.last_sent_hash.as_ref() == Some(&text_hash) {
                            false
                        } else {
                            let sync_id = generate_sync_id("host");
                            remember_sync_id(&mut state.recent_sync_ids, sync_id.clone());
                            state.last_sent_hash = Some(text_hash);
                            let _ = send_control_event(
                                &control_tx,
                                serde_json::json!({
                                    "type": "clipboard_data",
                                    "text": text,
                                    "source": "host",
                                    "sync_id": sync_id,
                                }),
                            );
                            true
                        }
                    };
                    if should_send {
                        log_event(format!("Clipboard auto-pushed to {peer_addr}"));
                    }
                }
                Ok(None) => {}
                Err(err) => log_event(format!("Clipboard listener read failed: {err}")),
            }
        }

        if let Ok(current) = pending_pair_device.lock() {
            if let Some(device_id) = current.clone() {
                if let Some(challenge) = trust_store::take_approved_pair_challenge(&device_id) {
                    let _ = send_control_event(
                        &control_tx,
                        serde_json::json!({
                            "type": "pair_challenge",
                            "device_id": challenge.device_id,
                            "host_nonce_b64": challenge.host_nonce_b64,
                            "challenge_b64": challenge.challenge_b64,
                        }),
                    );
                    log_event(format!("Pair challenge issued to {peer_addr} for {device_id}"));
                } else if matches!(
                    trust_store::pending_request_status(&device_id),
                    Some(trust_store::PendingPairRequestStatus::Rejected)
                ) {
                    let _ = send_control_event(
                        &control_tx,
                        serde_json::json!({
                            "type": "pair_result",
                            "ok": false,
                            "message": "rejected_by_host",
                            "device_id": device_id,
                        }),
                    );
                    let _ = trust_store::clear_pending_request(&device_id);
                    log_event(format!("Pair request rejected for {peer_addr}: {device_id}"));
                }
            }
        }

        let now = Instant::now();
        if now.duration_since(last_telemetry_sent_at) >= StdDuration::from_secs(2) {
            telemetry.resync_requests = resync_requests.load(Ordering::SeqCst);
            let _ = send_control_event(&control_tx, stream_stats_event(&telemetry));
            last_telemetry_sent_at = now;
        }

        while let Some(receiver) = audio_rx.as_mut() {
            let audio_message = match receiver.try_recv() {
                Ok(message) => message,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    audio_rx = None;
                    audio_capture_running = false;
                    log_event(format!("Audio capture channel closed for {peer_addr}"));
                    break;
                }
            };
            match audio_message {
                Ok(packet) => {
                    if !config_snapshot.audio_enabled {
                        continue;
                    }
                    let payload = audio_capture::encode_audio_payload(&packet);
                    if let Err(err) = protocol::write_message(
                        &mut write_half,
                        protocol::MSG_AUDIO_PACKET,
                        &payload,
                    )
                    .await
                    {
                        log_event(format!(
                            "Client {peer_addr} disconnected while sending audio packet: {err}"
                        ));
                        break 'write_loop;
                    }
                }
                Err(err) => {
                    if !config_snapshot.audio_enabled {
                        continue;
                    }
                    audio_rx = None;
                    if let Some(stop_flag) = audio_stop.take() {
                        stop_flag.store(true, Ordering::SeqCst);
                    }
                    audio_capture_running = false;
                    log_event(format!("Audio capture failed for {peer_addr}: {err}"));
                    let _ = send_control_event(
                        &control_tx,
                        serde_json::json!({
                            "type": "host_error",
                            "source": "audio_capture",
                            "message": err,
                        }),
                    );
                    break;
                }
            }
        }

        while let Ok(payload) = control_rx.try_recv() {
            if let Err(err) =
                protocol::write_message(&mut write_half, protocol::MSG_CONTROL_INPUT, &payload).await
            {
                log_event(format!(
                    "Client {peer_addr} disconnected while sending control event: {err}"
                ));
                break 'write_loop;
            }
        }

        if let Err(err) = write_half.flush().await {
            log_event(format!("Client {peer_addr} disconnected while flushing stream: {err}"));
            break;
        }

        let mut frame_interval = Duration::from_millis(
            config_snapshot
                .frame_interval_ms()
                .saturating_mul(idle_frame_multiplier),
        );
        if config_snapshot.audio_enabled && frame_interval > Duration::from_millis(20) {
            frame_interval = Duration::from_millis(20);
        }
        select! {
            _ = sleep(frame_interval) => {}
            changed = write_stop_rx.changed() => {
                if changed.is_ok() && *write_stop_rx.borrow() {
                    break;
                }
            }
            changed = panic_rx.changed() => {
                if changed.is_ok() && *panic_rx.borrow() != panic_generation {
                    log_event(format!("[panic] session terminated by hotkey for {peer_addr}"));
                    break;
                }
            }
        }
    }

    if let Some(stop_flag) = audio_stop.take() {
        stop_flag.store(true, Ordering::SeqCst);
    }
    let _ = write_half.shutdown().await;
    input_task.abort();
    let _ = input_task.await;
    log_event(format!("Client session ended for {peer_addr}"));
}

#[cfg(windows)]
fn panic_hotkey_enabled() -> bool {
    std::env::var("REMOTE_DESKTOP_PANIC_HOTKEY_ENABLED")
        .map(|v| {
            let normalized = v.trim().to_ascii_lowercase();
            !(normalized == "0" || normalized == "false" || normalized == "no")
        })
        .unwrap_or(true)
}

#[cfg(windows)]
struct FileTransferResult {
    transfer_id: String,
    success: bool,
    filename: String,
    saved_path: String,
    bytes_received: usize,
    checksum_ok: bool,
    error: Option<String>,
}

#[cfg(windows)]
fn send_control_event(tx: &mpsc::UnboundedSender<Vec<u8>>, value: serde_json::Value) -> Result<()> {
    let payload = serde_json::to_vec(&value).context("serialize control event")?;
    tx.send(payload)
        .map_err(|_| anyhow!("control channel closed"))?;
    Ok(())
}

#[cfg(windows)]
fn monitor_inventory_event() -> serde_json::Value {
    match screen_capture::list_monitors() {
        Ok(monitors) => serde_json::json!({
            "type": "monitor_inventory",
            "monitors": monitors
                .iter()
                .enumerate()
                .map(|(index, monitor)| serde_json::json!({
                    "index": index,
                    "left": monitor.left,
                    "top": monitor.top,
                    "width": monitor.width,
                    "height": monitor.height,
                    "label": format!("Monitor {} ({}x{} @ {}, {})", index, monitor.width, monitor.height, monitor.left, monitor.top),
                }))
                .collect::<Vec<_>>()
        }),
        Err(err) => serde_json::json!({
            "type": "host_error",
            "source": "monitor_inventory",
            "message": err.to_string(),
        }),
    }
}

#[cfg(windows)]
fn session_status_event(config: &SessionConfig) -> serde_json::Value {
    serde_json::json!({
        "type": "session_status",
        "monitor_index": config.monitor_index,
        "target_width": config.target_width,
        "fps": config.fps,
        "jpeg_quality": config.jpeg_quality,
        "view_only": config.view_only,
        "clipboard_mode": config.clipboard_mode.as_wire(),
        "delta_stream_enabled": config.delta_stream_enabled,
        "audio_enabled": config.audio_enabled,
    })
}

#[cfg(windows)]
fn remember_sync_id(cache: &mut VecDeque<String>, sync_id: String) {
    cache.push_back(sync_id);
    while cache.len() > 32 {
        cache.pop_front();
    }
}

#[cfg(windows)]
fn hash_clipboard_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(windows)]
fn generate_sync_id(source: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{source}-{nanos}")
}

#[cfg(windows)]
fn random_b64(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    BASE64.encode(bytes)
}

#[cfg(windows)]
fn start_file_transfer(
    transfer_id: &str,
    filename: &str,
    size: usize,
    sha256: Option<String>,
) -> Result<FileTransferState> {
    if size == 0 {
        return Err(anyhow!("file is empty"));
    }
    if size > MAX_FILE_SIZE_BYTES {
        return Err(anyhow!("file exceeds max size of {} bytes", MAX_FILE_SIZE_BYTES));
    }

    let base = receive_dir();
    fs::create_dir_all(&base).context("create output directory")?;

    let safe_name: String = filename
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    let name = if safe_name.is_empty() {
        "received.bin"
    } else {
        &safe_name
    };

    let final_path = next_available_path(&base, name);
    let temp_path = final_path.with_extension(format!(
        "{}part",
        final_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| format!("{ext}."))
            .unwrap_or_default()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp_path)
        .with_context(|| format!("create temp file {}", temp_path.display()))?;
    Ok(FileTransferState {
        transfer_id: transfer_id.to_string(),
        file: Some(file),
        filename: name.to_string(),
        temp_path,
        final_path,
        total_size: size,
        bytes_received: 0,
        expected_sha256: sha256.map(|s| s.to_lowercase()),
        hasher: Sha256::new(),
        last_progress_percent: 0,
        expected_seq: 0,
        last_acked_seq: None,
        last_acked_offset: 0,
        last_acked_len: 0,
    })
}

#[cfg(windows)]
fn write_file_chunk(state: &mut FileTransferState, seq: u64, offset: usize, chunk: &[u8]) -> Result<()> {
    use std::io::Write;
    if seq != state.expected_seq {
        return Err(anyhow!(
            "sequence mismatch: transfer_id={} expected_seq={} got_seq={}",
            state.transfer_id,
            state.expected_seq,
            seq
        ));
    }
    if offset != state.bytes_received {
        return Err(anyhow!(
            "offset mismatch: transfer_id={} expected_offset={} got_offset={}",
            state.transfer_id,
            state.bytes_received,
            offset
        ));
    }
    if state.bytes_received + chunk.len() > state.total_size {
        return Err(anyhow!(
            "chunk exceeds declared file size: transfer_id={} bytes_received={} chunk_len={} total_size={}",
            state.transfer_id,
            state.bytes_received,
            chunk.len(),
            state.total_size
        ));
    }
    let file = state
        .file
        .as_mut()
        .ok_or_else(|| anyhow!("transfer file handle missing: transfer_id={}", state.transfer_id))?;
    file.write_all(chunk).context("write file chunk")?;
    state.hasher.update(chunk);
    state.bytes_received += chunk.len();
    state.expected_seq += 1;
    state.last_acked_seq = Some(seq);
    state.last_acked_offset = offset;
    state.last_acked_len = chunk.len();
    Ok(())
}

#[cfg(windows)]
fn finalize_file_transfer(state: &mut FileTransferState) -> Result<FileTransferResult> {
    use std::io::Write;

    let mut file = state
        .file
        .take()
        .ok_or_else(|| anyhow!("transfer file handle missing: transfer_id={}", state.transfer_id))?;
    file.flush().context("flush output file")?;
    file.sync_all().context("sync output file")?;
    drop(file);
    if state.bytes_received != state.total_size {
        return Err(anyhow!(
            "incomplete transfer: transfer_id={} expected_size={} actual_size={}",
            state.transfer_id,
            state.total_size,
            state.bytes_received
        ));
    }
    let digest = format!("{:x}", state.hasher.clone().finalize());
    let checksum_ok = state
        .expected_sha256
        .as_ref()
        .map(|expected| expected == &digest)
        .unwrap_or(true);

    if !checksum_ok {
        return Err(anyhow!(
            "checksum mismatch: transfer_id={} expected_sha256={} actual_sha256={} expected_size={} actual_size={}",
            state.transfer_id,
            state.expected_sha256.clone().unwrap_or_else(|| "none".to_string()),
            digest,
            state.total_size,
            state.bytes_received
        ));
    }

    fs::rename(&state.temp_path, &state.final_path).with_context(|| {
        format!(
            "rename temp file {} to {}",
            state.temp_path.display(),
            state.final_path.display()
        )
    })?;

    let filename = state.filename.clone();
    let saved_path = state.final_path.display().to_string();

    Ok(FileTransferResult {
        transfer_id: state.transfer_id.clone(),
        success: true,
        filename,
        saved_path,
        bytes_received: state.bytes_received,
        checksum_ok: true,
        error: None,
    })
}

#[cfg(windows)]
fn cleanup_file_transfer(state: FileTransferState) {
    drop(state.file);
    let _ = fs::remove_file(&state.temp_path);
}

#[cfg(windows)]
fn current_progress_percent(state: &FileTransferState) -> Option<u8> {
    if state.total_size == 0 {
        return None;
    }
    Some(((state.bytes_received as f64 / state.total_size as f64) * 100.0).round() as u8)
}

#[cfg(windows)]
fn receive_dir() -> PathBuf {
    std::env::var("REMOTE_DESKTOP_RECEIVE_DIR")
        .or_else(|_| std::env::var("REMOTE_DESKTOP_DOWNLOAD_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("received_files"))
}

#[cfg(windows)]
fn next_available_path(base: &PathBuf, filename: &str) -> PathBuf {
    let candidate = base.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let stem = PathBuf::from(filename)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("received")
        .to_string();
    let ext = PathBuf::from(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{ext}"))
        .unwrap_or_default();

    for idx in 1..1000 {
        let next = base.join(format!("{stem}_{idx}{ext}"));
        if !next.exists() {
            return next;
        }
    }

    base.join(format!("{stem}_overflow{ext}"))
}

#[cfg(windows)]
fn stream_stats_event(telemetry: &StreamTelemetry) -> serde_json::Value {
    serde_json::json!({
        "type": "stream_stats",
        "keyframes_sent": telemetry.keyframes_sent,
        "delta_frames_sent": telemetry.delta_frames_sent,
        "resync_requests": telemetry.resync_requests,
        "inferred_move_frames": telemetry.inferred_move_frames,
        "last_patch_count": telemetry.last_patch_count,
        "last_move_count": telemetry.last_move_count,
        "last_changed_ratio": telemetry.last_changed_ratio,
    })
}
