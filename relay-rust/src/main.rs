use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::{rustls, server::TlsStream, TlsAcceptor};

const MSG_CONTROL_INPUT: u8 = 0x02;
const HANDSHAKE_TIMEOUT_SECS: u64 = 15;

#[derive(Clone)]
struct RelayConfig {
    addr: String,
    cert_path: String,
    key_path: String,
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RelayControlMessage {
    RelayRegisterHost { host_id: String, token: Option<String> },
    RelayConnectClient {
        host_id: String,
        token: Option<String>,
        device_id: Option<String>,
    },
}

struct HostRegistration {
    relay_token: Option<String>,
    matched_tx: oneshot::Sender<MatchedClient>,
}

struct MatchedClient {
    stream: TlsStream<TcpStream>,
    peer_addr: std::net::SocketAddr,
    device_id: Option<String>,
}

type Registry = Arc<Mutex<HashMap<String, HostRegistration>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let config = RelayConfig {
        addr: value_after(&args, "--addr").unwrap_or_else(|| "0.0.0.0:7000".to_string()),
        cert_path: value_after(&args, "--cert").unwrap_or_else(|| "../certs/server.crt".to_string()),
        key_path: value_after(&args, "--key").unwrap_or_else(|| "../certs/server.key".to_string()),
        access_token: value_after(&args, "--access-token"),
    };

    let tls_acceptor = load_tls_acceptor(&config)?;
    let listener = TcpListener::bind(&config.addr)
        .await
        .with_context(|| format!("bind {}", config.addr))?;

    eprintln!("Relay listening on {}", config.addr);
    eprintln!("Relay TLS enabled using cert={} key={}", config.cert_path, config.key_path);
    eprintln!(
        "Relay access token loaded: {}",
        config
            .access_token
            .as_ref()
            .map(|token| !token.is_empty())
            .unwrap_or(false)
    );

    let registry: Registry = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let (tcp_stream, peer_addr) = listener.accept().await.context("accept relay connection")?;
        let acceptor = tls_acceptor.clone();
        let registry = registry.clone();
        let access_token = config.access_token.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(acceptor, registry, access_token, tcp_stream, peer_addr).await {
                eprintln!("Relay connection error for {peer_addr}: {err}");
            }
        });
    }
}

async fn handle_connection(
    tls_acceptor: TlsAcceptor,
    registry: Registry,
    access_token: Option<String>,
    tcp_stream: TcpStream,
    peer_addr: std::net::SocketAddr,
) -> Result<()> {
    let mut stream = timeout(Duration::from_secs(HANDSHAKE_TIMEOUT_SECS), tls_acceptor.accept(tcp_stream))
        .await
        .context("relay TLS handshake timed out")??
        ;

    let Some((msg_type, payload)) = timeout(
        Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        read_message(&mut stream),
    )
    .await
    .context("relay initial message timed out")?? else {
        return Err(anyhow!("connection closed before relay handshake"));
    };

    if msg_type != MSG_CONTROL_INPUT {
        return Err(anyhow!("expected control message as first relay frame"));
    }

    let control: RelayControlMessage =
        serde_json::from_slice(&payload).context("parse relay control")?;

    match control {
        RelayControlMessage::RelayRegisterHost { host_id, token } => {
            if access_token != token {
                send_control(
                    &mut stream,
                    json!({"type":"relay_error","message":"relay access token rejected"}),
                )
                .await?;
                return Ok(());
            }

            let (matched_tx, matched_rx) = oneshot::channel();
            {
                let mut guard = registry.lock().await;
                if guard.contains_key(&host_id) {
                    send_control(
                        &mut stream,
                        json!({"type":"relay_error","message":"duplicate host registration","host_id":host_id}),
                    )
                    .await?;
                    return Ok(());
                }
                guard.insert(
                    host_id.clone(),
                    HostRegistration {
                        relay_token: token,
                        matched_tx,
                    },
                );
            }

            eprintln!("Relay host registered: host_id={host_id} peer={peer_addr}");
            send_control(
                &mut stream,
                json!({"type":"relay_host_registered","host_id":host_id}),
            )
            .await?;

            let matched = match matched_rx.await {
                Ok(matched) => matched,
                Err(_) => {
                    let mut guard = registry.lock().await;
                    guard.remove(&host_id);
                    eprintln!("Relay host removed: host_id={host_id} reason=registration_cancelled_before_match");
                    return Err(anyhow!("host registration cancelled before match"));
                }
            };

            let session_id = format!("{}-{}", host_id, Instant::now().elapsed().as_nanos());
            let mut client_stream = matched.stream;

            send_control(
                &mut stream,
                json!({
                    "type":"relay_session_ready",
                    "session_id":session_id,
                    "host_id":host_id,
                    "peer_role":"host",
                    "client_peer":matched.peer_addr.to_string(),
                    "device_id":matched.device_id,
                }),
            )
            .await?;
            send_control(
                &mut client_stream,
                json!({
                    "type":"relay_session_ready",
                    "session_id":session_id,
                    "host_id":host_id,
                    "peer_role":"client",
                }),
            )
            .await?;
            eprintln!("Relay session ready: host_id={host_id} session_id={session_id}");

            let (host_read, host_write) = tokio::io::split(stream);
            let (client_read, client_write) = tokio::io::split(client_stream);
            let host_to_client = tokio::spawn(forward_session_frames(
                host_id.clone(),
                session_id.clone(),
                "host->client",
                host_read,
                client_write,
            ));
            let client_to_host = tokio::spawn(forward_session_frames(
                host_id.clone(),
                session_id.clone(),
                "client->host",
                client_read,
                host_write,
            ));
            let _ = tokio::join!(host_to_client, client_to_host);
            eprintln!("Relay session closed: host_id={host_id} session_id={session_id}");
            Ok(())
        }
        RelayControlMessage::RelayConnectClient {
            host_id,
            token,
            device_id,
        } => {
            let registration = {
                let mut guard = registry.lock().await;
                let removed = guard.remove(&host_id);
                if removed.is_some() {
                    eprintln!("Relay host removed: host_id={host_id} reason=matched_to_client");
                }
                removed
            };

            let Some(registration) = registration else {
                eprintln!("Relay connect rejected: host_id={host_id} reason=host_offline");
                send_control(
                    &mut stream,
                    json!({"type":"relay_error","message":"host offline","host_id":host_id}),
                )
                .await?;
                return Ok(());
            };

            if registration.relay_token != token {
                send_control(
                    &mut stream,
                    json!({"type":"relay_error","message":"relay access token rejected","host_id":host_id}),
                )
                .await?;
                return Ok(());
            }

            eprintln!(
                "Relay client connect request: host_id={} peer={} device_id={}",
                host_id,
                peer_addr,
                device_id.clone().unwrap_or_else(|| "unknown".to_string())
            );

            registration
                .matched_tx
                .send(MatchedClient {
                    stream,
                    peer_addr,
                    device_id,
                })
                .map_err(|_| anyhow!("host registration no longer available"))?;
            Ok(())
        }
    }
}

async fn forward_session_frames<R, W>(
    host_id: String,
    session_id: String,
    direction: &'static str,
    mut reader: R,
    mut writer: W,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    while let Some((msg_type, payload)) = read_message(&mut reader).await? {
        if msg_type == MSG_CONTROL_INPUT {
            eprintln!(
                "Relay forward channel=control direction={direction} host_id={host_id} session_id={session_id} len={}",
                payload.len()
            );
        }
        write_message(&mut writer, msg_type, &payload).await?;
    }
    Ok(())
}

fn load_tls_acceptor(config: &RelayConfig) -> Result<TlsAcceptor> {
    let cert_file = File::open(&config.cert_path).with_context(|| format!("open {}", config.cert_path))?;
    let mut cert_reader = BufReader::new(cert_file);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("read relay certificate chain")?;
    if cert_chain.is_empty() {
        return Err(anyhow!("relay certificate chain is empty"));
    }

    let key_file = File::open(&config.key_path).with_context(|| format!("open {}", config.key_path))?;
    let mut key_reader = BufReader::new(key_file);
    let private_key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .context("read relay private key")?
        .ok_or_else(|| anyhow!("no relay private key found"))?;

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)
        .context("build relay TLS config")?;
    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

async fn send_control<S>(stream: &mut S, value: serde_json::Value) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(&value).context("serialize relay response")?;
    write_message(stream, MSG_CONTROL_INPUT, &payload)
        .await
        .context("write relay response")?;
    Ok(())
}

async fn write_message<W>(writer: &mut W, msg_type: u8, payload: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_u8(msg_type).await?;
    writer.write_u32(payload.len() as u32).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_message<R>(reader: &mut R) -> io::Result<Option<(u8, Vec<u8>)>>
where
    R: AsyncRead + Unpin,
{
    let mut msg_type = [0u8; 1];
    match reader.read_exact(&mut msg_type).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }

    let payload_len = reader.read_u32().await? as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload).await?;
    Ok(Some((msg_type[0], payload)))
}

fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|idx| args.get(idx + 1))
        .cloned()
}
