#[cfg(windows)]
use anyhow::{anyhow, Context, Result};
#[cfg(windows)]
use serde_json::json;
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use tokio::net::TcpStream;
#[cfg(windows)]
use tokio::sync::watch;
#[cfg(windows)]
use tokio::time::timeout;
#[cfg(windows)]
use tokio_rustls::client::TlsStream;
#[cfg(windows)]
use tokio_rustls::rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
#[cfg(windows)]
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
#[cfg(windows)]
use tokio_rustls::rustls::{self, ClientConfig, DigitallySignedStruct, SignatureScheme};
#[cfg(windows)]
use tokio_rustls::TlsConnector;

#[cfg(windows)]
use crate::{protocol, server};

#[cfg(windows)]
const RELAY_HANDSHAKE_TIMEOUT_SECS: u64 = 15;
#[cfg(windows)]
const RELAY_SESSION_WAIT_SECS: u64 = 300;
#[cfg(windows)]
const RELAY_RECONNECT_DELAY_SECS: u64 = 2;

#[cfg(windows)]
#[derive(Debug)]
struct InsecureRelayVerifier;

#[cfg(windows)]
impl ServerCertVerifier for InsecureRelayVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
        ]
    }
}

#[cfg(windows)]
pub async fn run_host_via_relay(
    relay_addr: &str,
    host_id: &str,
    relay_token: Option<String>,
    required_token: Option<String>,
) -> Result<()> {
    let mut reconnect_attempt: u64 = 0;
    loop {
        reconnect_attempt = reconnect_attempt.saturating_add(1);
        eprintln!(
            "Relay connect attempt={} start for host_id={} via {}",
            reconnect_attempt, host_id, relay_addr
        );
        let stop_rx = watch::channel(false).1;
        let panic_rx = watch::channel(0u64).1;

        let loop_result: Result<()> = async {
            let stream = connect_relay_tls(relay_addr).await?;
            eprintln!(
                "Relay connect attempt={} success for host_id={} via {}",
                reconnect_attempt, host_id, relay_addr
            );
            let peer_addr = stream
                .get_ref()
                .0
                .peer_addr()
                .or_else(|_| stream.get_ref().0.local_addr())
                .context("resolve relay peer address")?;

            let mut stream = stream;
            let registration = json!({
                "type": "relay_register_host",
                "host_id": host_id,
                "token": relay_token,
            });
            eprintln!(
                "Relay register_host sent for host_id={} attempt={}",
                host_id, reconnect_attempt
            );
            send_control(&mut stream, registration).await?;
            let mut registration_confirmed = false;

            loop {
                let read_future = protocol::read_message(&mut stream);
                let message = if registration_confirmed {
                    eprintln!(
                        "Relay heartbeat/session wait active for host_id={} timeout_secs={}",
                        host_id, RELAY_SESSION_WAIT_SECS
                    );
                    timeout(Duration::from_secs(RELAY_SESSION_WAIT_SECS), read_future)
                        .await
                        .context("relay session wait timed out")??
                } else {
                    timeout(Duration::from_secs(RELAY_HANDSHAKE_TIMEOUT_SECS), read_future)
                        .await
                        .context("relay registration timed out")??
                };
                let Some((msg_type, payload)) = message else {
                    return Err(anyhow!("relay closed connection before session was ready"));
                };
                if msg_type != protocol::MSG_CONTROL_INPUT {
                    continue;
                }
                let message: serde_json::Value =
                    serde_json::from_slice(&payload).context("parse relay message")?;
                match message.get("type").and_then(|v| v.as_str()) {
                    Some("relay_host_registered") => {
                        registration_confirmed = true;
                        eprintln!(
                            "Relay host registration complete for host_id={} via {} attempt={}",
                            host_id, relay_addr, reconnect_attempt
                        );
                    }
                    Some("relay_session_ready") => {
                        let session_id = message
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        eprintln!(
                            "Relay session ready for host_id={} session_id={} attempt={}",
                            host_id, session_id, reconnect_attempt
                        );
                        server::run_session_over_stream(
                            stream,
                            peer_addr,
                            stop_rx.clone(),
                            panic_rx.clone(),
                            required_token.clone(),
                        )
                        .await;
                        eprintln!(
                            "Relay session ended for host_id={} session_id={} reason=server_session_returned re_registering=true",
                            host_id, session_id
                        );
                        return Ok(());
                    }
                    Some("relay_error") => {
                        let detail = message
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown relay error");
                        eprintln!(
                            "Relay control error for host_id={} attempt={} detail={}",
                            host_id, reconnect_attempt, detail
                        );
                        return Err(anyhow!("relay rejected host registration: {detail}"));
                    }
                    other => {
                        eprintln!("Relay control message for host_id={}: {:?}", host_id, other);
                    }
                }
            }
        }
        .await;

        match loop_result {
            Ok(()) => {
                eprintln!(
                    "Relay reconnect scheduled for host_id={} delay_secs={} reason=session_complete",
                    host_id, RELAY_RECONNECT_DELAY_SECS
                );
                tokio::time::sleep(Duration::from_secs(RELAY_RECONNECT_DELAY_SECS)).await;
            }
            Err(err) => {
                eprintln!(
                    "Relay connection cycle failed for host_id={} via {} attempt={} disconnect_reason={err}",
                    host_id, relay_addr, reconnect_attempt
                );
                eprintln!(
                    "Relay reconnect scheduled for host_id={} delay_secs={} reason=connection_cycle_failed",
                    host_id, RELAY_RECONNECT_DELAY_SECS
                );
                tokio::time::sleep(Duration::from_secs(RELAY_RECONNECT_DELAY_SECS)).await;
            }
        }
    }
}

#[cfg(windows)]
async fn connect_relay_tls(relay_addr: &str) -> Result<TlsStream<TcpStream>> {
    let tcp_stream = TcpStream::connect(relay_addr)
        .await
        .with_context(|| format!("connect relay {relay_addr}"))?;
    let server_name = relay_addr
        .split(':')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or("localhost")
        .to_string();
    let server_name = ServerName::try_from(server_name)
        .map_err(|_| anyhow!("invalid relay server name"))?
        .to_owned();
    let client_config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureRelayVerifier))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    connector
        .connect(server_name, tcp_stream)
        .await
        .with_context(|| format!("TLS connect relay {relay_addr}"))
}

#[cfg(windows)]
async fn send_control(
    stream: &mut TlsStream<TcpStream>,
    value: serde_json::Value,
) -> Result<()> {
    let payload = serde_json::to_vec(&value).context("serialize relay control")?;
    protocol::write_message(stream, protocol::MSG_CONTROL_INPUT, &payload)
        .await
        .context("write relay control")?;
    Ok(())
}
