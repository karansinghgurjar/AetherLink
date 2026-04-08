#[cfg(not(windows))]
fn main() {
    eprintln!("This binary is Windows-only.");
}

#[cfg(windows)]
mod audio_capture;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod config;
#[cfg(windows)]
mod delta_stream;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod panic_hotkey;
#[cfg(windows)]
mod protocol;
#[cfg(windows)]
mod relay_client;
#[cfg(windows)]
mod screen_capture;
#[cfg(windows)]
mod server;
#[cfg(windows)]
mod session_config;
#[cfg(windows)]
mod trust_store;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    let loaded_config = config::HostConfig::load_or_create()?;
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--cli") {
        let relay_addr = args
            .iter()
            .position(|arg| arg == "--relay-addr")
            .and_then(|idx| args.get(idx + 1))
            .cloned();
        let relay_host_id = args
            .iter()
            .position(|arg| arg == "--relay-host-id")
            .and_then(|idx| args.get(idx + 1))
            .cloned();
        let addr = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| loaded_config.server_addr());
        let token = args
            .iter()
            .position(|arg| arg == "--token")
            .and_then(|idx| args.get(idx + 1))
            .cloned()
            .or_else(|| loaded_config.normalized_auth_token());
        let cert_path = args
            .iter()
            .position(|arg| arg == "--cert")
            .and_then(|idx| args.get(idx + 1))
            .cloned()
            .unwrap_or_else(|| loaded_config.cert_path.clone());
        let key_path = args
            .iter()
            .position(|arg| arg == "--key")
            .and_then(|idx| args.get(idx + 1))
            .cloned()
            .unwrap_or_else(|| loaded_config.key_path.clone());
        loaded_config.apply_runtime_env();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        if let (Some(relay_addr), Some(host_id)) = (relay_addr, relay_host_id) {
            let relay_token = args
                .iter()
                .position(|arg| arg == "--relay-token")
                .and_then(|idx| args.get(idx + 1))
                .cloned();
            runtime.block_on(relay_client::run_host_via_relay(
                &relay_addr,
                &host_id,
                relay_token,
                token,
            ))
        } else {
            let tls = server::TlsConfig { cert_path, key_path };
            runtime.block_on(server::run_server_with_auth_and_tls(&addr, token, tls))
        }
    } else {
        ui::run_gui()
    }
}
