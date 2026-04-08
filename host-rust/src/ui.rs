#[cfg(windows)]
use crate::{config::HostConfig, screen_capture, server, trust_store};
#[cfg(windows)]
use anyhow::Context;
#[cfg(windows)]
use eframe::egui;
#[cfg(windows)]
use std::collections::HashMap;
#[cfg(windows)]
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
#[cfg(windows)]
use std::process::{Child, Command, Stdio};
#[cfg(windows)]
use std::sync::mpsc::{self, Receiver};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Duration;
#[cfg(windows)]
use tokio::sync::watch;

#[cfg(windows)]
pub fn run_gui() -> anyhow::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "AetherLink Host",
        options,
        Box::new(|_cc| Ok(Box::new(HostApp::new()))),
    )
    .map_err(|err| anyhow::anyhow!("Failed to run host UI: {err}"))
}

#[cfg(windows)]
struct ServerTask {
    stop_tx: watch::Sender<bool>,
    done_rx: Receiver<anyhow::Result<()>>,
    handle: thread::JoinHandle<()>,
}

#[cfg(windows)]
impl ServerTask {
    fn start(addr: String, auth_token: Option<String>, tls: server::TlsConfig) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();

            let result = match runtime {
                Ok(rt) => rt.block_on(server::run_server_until_with_auth(&addr, stop_rx, auth_token, tls)),
                Err(err) => Err(anyhow::anyhow!("Failed to create Tokio runtime: {err}")),
            };

            let _ = done_tx.send(result);
        });

        Self {
            stop_tx,
            done_rx,
            handle,
        }
    }

    fn request_stop(&self) {
        let _ = self.stop_tx.send(true);
    }

    fn poll_result(&self) -> Option<anyhow::Result<()>> {
        self.done_rx.try_recv().ok()
    }

    fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

#[cfg(windows)]
struct RelayTask {
    child: Child,
}

#[cfg(windows)]
impl RelayTask {
    fn start(
        relay_addr: String,
        relay_host_id: String,
        relay_token: Option<String>,
        auth_token: Option<String>,
    ) -> anyhow::Result<Self> {
        let exe_path = std::env::current_exe().context("resolve current exe for relay worker")?;
        let mut command = Command::new(exe_path);
        command
            .arg("--cli")
            .arg("0.0.0.0:6000")
            .arg("--relay-addr")
            .arg(relay_addr)
            .arg("--relay-host-id")
            .arg(relay_host_id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(token) = auth_token.filter(|value| !value.trim().is_empty()) {
            command.arg("--token").arg(token);
        }
        if let Some(token) = relay_token.filter(|value| !value.trim().is_empty()) {
            command.arg("--relay-token").arg(token);
        }
        let child = command.spawn().context("spawn relay worker")?;
        Ok(Self { child })
    }

    fn request_stop(&mut self) {
        let _ = self.child.kill();
    }

    fn is_finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

#[cfg(windows)]
struct RelayServerTask {
    child: Child,
}

#[cfg(windows)]
impl RelayServerTask {
    fn start(relay_addr: &str, relay_token: Option<String>, cert_path: &str, key_path: &str) -> anyhow::Result<Self> {
        let exe_path = resolve_relay_server_exe()?;
        let bind_addr = relay_bind_addr(relay_addr);
        let mut command = Command::new(exe_path);
        command
            .arg("--addr")
            .arg(bind_addr)
            .arg("--cert")
            .arg(cert_path)
            .arg("--key")
            .arg(key_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(token) = relay_token.filter(|value| !value.trim().is_empty()) {
            command.arg("--access-token").arg(token);
        }
        let child = command.spawn().context("spawn local relay server")?;
        Ok(Self { child })
    }

    fn request_stop(&mut self) {
        let _ = self.child.kill();
    }

    fn is_finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

#[cfg(windows)]
struct HostApp {
    config: HostConfig,
    local_ip: String,
    port: String,
    token: String,
    cert_path: String,
    key_path: String,
    relay_enabled: bool,
    relay_addr: String,
    relay_host_id: String,
    relay_token: String,
    download_dir: String,
    default_monitor_index: String,
    default_fps: String,
    default_jpeg_quality: String,
    default_target_width: String,
    panic_hotkey_enabled: bool,
    status: String,
    monitor_summary: String,
    config_path: String,
    server_task: Option<ServerTask>,
    relay_server_task: Option<RelayServerTask>,
    relay_task: Option<RelayTask>,
    stopping: bool,
    rename_buffers: HashMap<String, String>,
}

#[cfg(windows)]
impl HostApp {
    fn new() -> Self {
        let config = HostConfig::load_or_create().unwrap_or_default();
        Self {
            config_path: HostConfig::path().display().to_string(),
            config: config.clone(),
            local_ip: detect_local_ip(),
            port: config.port.to_string(),
            token: config.auth_token.clone().unwrap_or_default(),
            cert_path: config.cert_path.clone(),
            key_path: config.key_path.clone(),
            relay_enabled: config.relay_enabled,
            relay_addr: config.relay_addr.clone(),
            relay_host_id: config.relay_host_id.clone(),
            relay_token: config.relay_token.clone().unwrap_or_default(),
            download_dir: config.download_dir.clone().unwrap_or_default(),
            default_monitor_index: config.default_monitor_index.to_string(),
            default_fps: config.default_fps.to_string(),
            default_jpeg_quality: config.default_jpeg_quality.to_string(),
            default_target_width: config
                .default_target_width
                .map(|value| value.to_string())
                .unwrap_or_default(),
            panic_hotkey_enabled: config.panic_hotkey_enabled,
            status: "Stopped".to_string(),
            monitor_summary: detect_monitor_summary(),
            server_task: None,
            relay_server_task: None,
            relay_task: None,
            stopping: false,
            rename_buffers: HashMap::new(),
        }
    }

    fn sync_config_from_ui(&mut self) {
        self.config.port = self.port.trim().parse::<u16>().unwrap_or(6000);
        self.config.auth_token = {
            let trimmed = self.token.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };
        self.config.cert_path = self.cert_path.trim().to_string();
        self.config.key_path = self.key_path.trim().to_string();
        self.config.relay_enabled = self.relay_enabled;
        self.config.relay_addr = self.relay_addr.trim().to_string();
        self.config.relay_host_id = self.relay_host_id.trim().to_string();
        self.config.relay_token = {
            let trimmed = self.relay_token.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };
        self.config.download_dir = {
            let trimmed = self.download_dir.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };
        self.config.default_monitor_index = self.default_monitor_index.trim().parse::<u32>().unwrap_or(0);
        self.config.default_fps = self.default_fps.trim().parse::<u32>().unwrap_or(20).clamp(1, 60);
        self.config.default_jpeg_quality = self
            .default_jpeg_quality
            .trim()
            .parse::<u8>()
            .unwrap_or(70)
            .clamp(1, 100);
        self.config.default_target_width = {
            let trimmed = self.default_target_width.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<u32>().ok()
            }
        };
        self.config.panic_hotkey_enabled = self.panic_hotkey_enabled;
    }

    fn start_server(&mut self) {
        let port = self.port.trim().parse::<u16>();
        let port = match port {
            Ok(port) if port > 0 => port,
            _ => {
                self.status = "Invalid port".to_string();
                return;
            }
        };

        let addr = format!("0.0.0.0:{port}");
        self.sync_config_from_ui();
        let token = self.config.normalized_auth_token();
        let tls = server::TlsConfig {
            cert_path: self.cert_path.trim().to_string(),
            key_path: self.key_path.trim().to_string(),
        };
        self.config.apply_runtime_env();
        if let Err(err) = self.config.save() {
            self.status = format!("Config save failed: {err}");
            return;
        }
        self.status = format!("Starting on {addr}...");
        self.server_task = Some(ServerTask::start(addr.clone(), token.clone(), tls));
        if self.relay_enabled {
            if should_start_local_relay_server(self.relay_addr.trim()) {
                if local_relay_reachable(self.relay_addr.trim()) {
                    self.relay_server_task = None;
                } else {
                    match RelayServerTask::start(
                        self.relay_addr.trim(),
                        self.config.relay_token.clone(),
                        self.cert_path.trim(),
                        self.key_path.trim(),
                    ) {
                        Ok(task) => {
                            self.relay_server_task = Some(task);
                        }
                        Err(err) => {
                            self.server_task = None;
                            self.status = format!("Relay server failed: {err}");
                            return;
                        }
                    }
                }
            } else {
                self.relay_server_task = None;
            }
            match RelayTask::start(
                self.relay_addr.trim().to_string(),
                self.relay_host_id.trim().to_string(),
                self.config.relay_token.clone(),
                token,
            ) {
                Ok(task) => {
                    self.relay_task = Some(task);
                    self.status = format!("Starting on {addr} with relay...");
                }
                Err(err) => {
                    self.server_task = None;
                    if let Some(mut relay_server_task) = self.relay_server_task.take() {
                        relay_server_task.request_stop();
                    }
                    self.status = format!("Relay worker failed: {err}");
                    return;
                }
            }
        } else {
            self.relay_server_task = None;
            self.relay_task = None;
        }
        self.stopping = false;
    }

    fn save_config_only(&mut self) {
        self.sync_config_from_ui();
        match self.config.save() {
            Ok(_) => self.status = format!("Saved config to {}", self.config_path),
            Err(err) => self.status = format!("Config save failed: {err}"),
        }
    }

    fn request_stop_server(&mut self) {
        if let Some(task) = &self.server_task {
            task.request_stop();
        }
        if let Some(task) = &mut self.relay_server_task {
            task.request_stop();
        }
        if let Some(task) = &mut self.relay_task {
            task.request_stop();
        }
        self.status = "Stopping server...".to_string();
        self.stopping = true;
    }

    fn poll_server_state(&mut self) {
        let (result_opt, finished) = match self.server_task.as_ref() {
            Some(task) => (task.poll_result(), task.is_finished()),
            None => return,
        };

        if let Some(result) = result_opt {
            self.status = match result {
                Ok(_) => "Stopped".to_string(),
                Err(err) => format!("Server error: {err}"),
            };
        }

        if finished {
            if let Some(task) = self.server_task.take() {
                let _ = task.handle.join();
            }
            if let Some(mut relay_server_task) = self.relay_server_task.take() {
                relay_server_task.request_stop();
            }
            if let Some(mut relay_task) = self.relay_task.take() {
                relay_task.request_stop();
            }
            self.stopping = false;
            if self.status.starts_with("Starting") || self.status == "Stopping server..." {
                self.status = "Stopped".to_string();
            }
        } else if self.server_task.is_some() && !self.stopping && self.status.starts_with("Starting") {
            self.status = if self.relay_task.is_some() {
                "Running (relay enabled)".to_string()
            } else {
                "Running".to_string()
            };
        }

        if let Some(relay_server_task) = &mut self.relay_server_task {
            if relay_server_task.is_finished() && self.server_task.is_some() && !self.stopping {
                self.relay_server_task = None;
                if local_relay_reachable(self.relay_addr.trim()) {
                    if self.status.starts_with("Starting") || self.status.starts_with("Relay server stopped unexpectedly") {
                        self.status = if self.relay_task.is_some() {
                            "Running (relay enabled)".to_string()
                        } else {
                            "Running".to_string()
                        };
                    }
                } else {
                    self.status = "Relay server stopped unexpectedly".to_string();
                    if let Some(mut relay_task) = self.relay_task.take() {
                        relay_task.request_stop();
                    }
                }
            }
        }

        if let Some(relay_task) = &mut self.relay_task {
            if relay_task.is_finished() && self.server_task.is_some() && !self.stopping {
                self.status = "Relay worker stopped unexpectedly".to_string();
                self.relay_task = None;
            }
        }
    }
}

#[cfg(windows)]
impl Drop for HostApp {
    fn drop(&mut self) {
        if let Some(task) = &self.server_task {
            task.request_stop();
        }
        if let Some(task) = &mut self.relay_server_task {
            task.request_stop();
        }
        if let Some(task) = &mut self.relay_task {
            task.request_stop();
        }
        if let Some(task) = self.server_task.take() {
            let _ = task.handle.join();
        }
    }
}

#[cfg(windows)]
fn should_start_local_relay_server(relay_addr: &str) -> bool {
    let host = relay_addr
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(relay_addr)
        .trim()
        .to_ascii_lowercase();
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "0.0.0.0")
}

#[cfg(windows)]
fn relay_bind_addr(relay_addr: &str) -> String {
    if let Some((host, port)) = relay_addr.rsplit_once(':') {
        let normalized_host = host.trim().to_ascii_lowercase();
        if matches!(normalized_host.as_str(), "127.0.0.1" | "localhost") {
            return format!("0.0.0.0:{port}");
        }
    }
    relay_addr.trim().to_string()
}

#[cfg(windows)]
fn local_relay_reachable(relay_addr: &str) -> bool {
    let mut addrs = match relay_addr.to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(_) => return false,
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

#[cfg(windows)]
fn resolve_relay_server_exe() -> anyhow::Result<std::path::PathBuf> {
    let current_exe = std::env::current_exe().context("resolve current exe for relay server")?;
    for ancestor in current_exe.ancestors() {
        let candidate = ancestor.join("relay-rust").join("target").join("debug").join("relay-rust.exe");
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow::anyhow!(
        "Could not locate relay-rust.exe relative to {}",
        current_exe.display()
    ))
}

#[cfg(windows)]
impl eframe::App for HostApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_server_state();
        self.monitor_summary = detect_monitor_summary();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
            ui.heading("AetherLink Host");
            ui.separator();
            ui.label(format!("Local IP: {}", self.local_ip));
            ui.label(format!("Config: {}", self.config_path));

            ui.horizontal(|ui| {
                ui.label("Port:");
                ui.add_enabled(self.server_task.is_none(), egui::TextEdit::singleline(&mut self.port));
            });
            ui.horizontal(|ui| {
                ui.label("Token:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.token).password(true),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Cert:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.cert_path),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Key:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.key_path),
                );
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(self.server_task.is_none(), |ui| {
                    ui.checkbox(&mut self.relay_enabled, "Enable relay worker");
                });
            });
            ui.horizontal(|ui| {
                ui.label("Relay:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.relay_addr),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Relay Host ID:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.relay_host_id),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Relay Token:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.relay_token).password(true),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Download Dir:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.download_dir),
                );
            });

            ui.separator();
            ui.label("Default Session Config");
            ui.horizontal(|ui| {
                ui.label("Monitor:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.default_monitor_index),
                );
                ui.label("FPS:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.default_fps),
                );
                ui.label("JPEG:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.default_jpeg_quality),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Target Width:");
                ui.add_enabled(
                    self.server_task.is_none(),
                    egui::TextEdit::singleline(&mut self.default_target_width)
                        .hint_text("blank = native"),
                );
            });
            ui.add_enabled_ui(self.server_task.is_none(), |ui| {
                ui.checkbox(&mut self.panic_hotkey_enabled, "Enable panic hotkey");
            });

            ui.horizontal(|ui| {
                let start_enabled = self.server_task.is_none();
                if ui.add_enabled(start_enabled, egui::Button::new("Start Server")).clicked() {
                    self.start_server();
                }

                let stop_enabled = self.server_task.is_some() && !self.stopping;
                if ui.add_enabled(stop_enabled, egui::Button::new("Stop Server")).clicked() {
                    self.request_stop_server();
                }

                if ui
                    .add_enabled(self.server_task.is_none(), egui::Button::new("Save Config"))
                    .clicked()
                {
                    self.save_config_only();
                }
            });

            ui.separator();
            ui.label(format!("Status: {}", self.status));
            ui.label(format!("Monitors: {}", self.monitor_summary));
            ui.label(if self.token.trim().is_empty() {
                "Auth: Disabled"
            } else {
                "Auth: Enabled"
            });
            ui.label("Transport: TLS 1.3 (rustls)");
            ui.label(format!(
                "Relay: {} ({})",
                if self.relay_enabled { "Enabled" } else { "Disabled" },
                self.relay_addr
            ));
            ui.label("Client Host (ADB reverse): 127.0.0.1");
            ui.label("Client Host (Android emulator): 10.0.2.2");
            ui.label(if self.panic_hotkey_enabled {
                "Panic Hotkey: Ctrl+Alt+Shift+P"
            } else {
                "Panic Hotkey: Disabled"
            });
            ui.label(format!(
                "Session Defaults: monitor {}, {} FPS, JPEG {}, width {}",
                self.config.default_monitor_index,
                self.config.default_fps,
                self.config.default_jpeg_quality,
                self.config
                    .default_target_width
                    .map(|w| w.to_string())
                    .unwrap_or_else(|| "native".to_string())
            ));

            ui.separator();
            ui.label("Pending Pair Requests");
            let pending_requests = trust_store::list_pending_requests();
            if pending_requests.is_empty() {
                ui.label("No pending pair requests.");
            } else {
                for request in pending_requests {
                    ui.group(|ui| {
                        ui.label(format!("{} [{}]", request.device_name, request.device_id));
                        ui.label(format!("Requested: {}", request.requested_at));
                        ui.label(format!("Transport: {}", request.transport));
                        ui.horizontal(|ui| {
                            if ui.button("Approve").clicked() {
                                match trust_store::approve_pair_request(&request.device_id) {
                                    Ok(_) => self.status = format!("Approved pair request for {}", request.device_name),
                                    Err(err) => self.status = format!("Approve failed: {err}"),
                                }
                            }
                            if ui.button("Reject").clicked() {
                                match trust_store::reject_pair_request(&request.device_id) {
                                    Ok(_) => self.status = format!("Rejected pair request for {}", request.device_name),
                                    Err(err) => self.status = format!("Reject failed: {err}"),
                                }
                            }
                        });
                    });
                }
            }

            ui.separator();
            ui.label("Trusted Devices");
            let trusted_devices = trust_store::list_trusted_devices();
            if trusted_devices.is_empty() {
                ui.label("No trusted devices.");
            } else {
                for device in trusted_devices {
                    let rename_value = self
                        .rename_buffers
                        .entry(device.device_id.clone())
                        .or_insert_with(|| device.device_name.clone());
                    ui.group(|ui| {
                        ui.label(format!("{} [{}]", device.device_name, device.device_id));
                        ui.label(format!("Fingerprint: {}", device.public_key_fingerprint));
                        ui.label(format!("Added: {} | Last Seen: {}", device.added_at, device.last_seen));
                        ui.label(format!("Revoked: {}", if device.revoked { "yes" } else { "no" }));
                        ui.horizontal(|ui| {
                            ui.label("Label:");
                            ui.text_edit_singleline(rename_value);
                            if ui.button("Rename").clicked() {
                                match trust_store::rename_device(&device.device_id, rename_value) {
                                    Ok(_) => self.status = format!("Renamed device {}", device.device_id),
                                    Err(err) => self.status = format!("Rename failed: {err}"),
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let action = if device.revoked { "Unrevoke" } else { "Revoke" };
                            if ui.button(action).clicked() {
                                match trust_store::set_revoked(&device.device_id, !device.revoked) {
                                    Ok(_) => self.status = format!("{} device {}", action, device.device_id),
                                    Err(err) => self.status = format!("{} failed: {err}", action),
                                }
                            }
                        });
                    });
                }
            }

            ui.separator();
            ui.label("Recent Server Logs");
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                let logs = server::recent_logs(40);
                if logs.is_empty() {
                    ui.label("No logs yet.");
                } else {
                    for entry in logs {
                        ui.monospace(entry);
                    }
                }
            });
                });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn detect_local_ip() -> String {
    match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    return addr.ip().to_string();
                }
            }
            "Unknown".to_string()
        }
        Err(_) => "Unknown".to_string(),
    }
}

#[cfg(windows)]
fn detect_monitor_summary() -> String {
    match screen_capture::list_monitors() {
        Ok(monitors) => monitors
            .iter()
            .enumerate()
            .map(|(idx, m)| format!("{idx}: {}x{} @ ({}, {})", m.width, m.height, m.left, m.top))
            .collect::<Vec<_>>()
            .join(" | "),
        Err(_) => "Unavailable".to_string(),
    }
}
