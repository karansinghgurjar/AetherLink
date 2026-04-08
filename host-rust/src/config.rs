#[cfg(windows)]
use anyhow::{Context, Result};
#[cfg(windows)]
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
fn host_app_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    let workspace_dir = exe_dir
        .as_ref()
        .and_then(|dir| dir.parent())
        .and_then(|dir| dir.parent())
        .map(|dir| dir.to_path_buf());
    workspace_dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[cfg(windows)]
fn default_cert_path() -> String {
    host_app_dir()
        .parent()
        .unwrap_or(&host_app_dir())
        .join("certs")
        .join("server.crt")
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn default_key_path() -> String {
    host_app_dir()
        .parent()
        .unwrap_or(&host_app_dir())
        .join("certs")
        .join("server.key")
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn normalize_runtime_path(path: &str, fallback: impl Fn() -> String) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return fallback();
    }
    let candidate = PathBuf::from(trimmed);
    if candidate.is_absolute() {
        return candidate.to_string_lossy().into_owned();
    }

    let app_dir = host_app_dir();
    let app_relative = app_dir.join(trimmed);
    if app_relative.exists() {
        return app_relative.to_string_lossy().into_owned();
    }

    let workspace_relative = app_dir.parent().unwrap_or(&app_dir).join(trimmed);
    if workspace_relative.exists() {
        return workspace_relative.to_string_lossy().into_owned();
    }

    fallback()
}

#[cfg(windows)]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    pub port: u16,
    pub auth_token: Option<String>,
    pub cert_path: String,
    pub key_path: String,
    pub relay_enabled: bool,
    pub relay_addr: String,
    pub relay_host_id: String,
    pub relay_token: Option<String>,
    pub default_monitor_index: u32,
    pub default_fps: u32,
    pub default_jpeg_quality: u8,
    pub default_target_width: Option<u32>,
    pub download_dir: Option<String>,
    pub panic_hotkey_enabled: bool,
}

#[cfg(windows)]
impl Default for HostConfig {
    fn default() -> Self {
        Self {
            port: 6000,
            auth_token: None,
            cert_path: default_cert_path(),
            key_path: default_key_path(),
            relay_enabled: true,
            relay_addr: "127.0.0.1:7000".to_string(),
            relay_host_id: "default-host".to_string(),
            relay_token: None,
            default_monitor_index: 0,
            default_fps: 12,
            default_jpeg_quality: 50,
            default_target_width: Some(720),
            download_dir: None,
            panic_hotkey_enabled: true,
        }
    }
}

#[cfg(windows)]
impl HostConfig {
    pub fn path() -> PathBuf {
        host_app_dir().join("config.json")
    }

    pub fn load_or_create() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            let cfg = Self::default();
            cfg.save()?;
            return Ok(cfg);
        }

        let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut cfg = serde_json::from_str::<Self>(&raw).with_context(|| format!("parse {}", path.display()))?;
        cfg.cert_path = normalize_runtime_path(&cfg.cert_path, default_cert_path);
        cfg.key_path = normalize_runtime_path(&cfg.key_path, default_key_path);
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        let raw = serde_json::to_string_pretty(self).context("serialize config")?;
        fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn server_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }

    pub fn normalized_auth_token(&self) -> Option<String> {
        self.auth_token.as_ref().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }

    pub fn resolved_download_dir(&self) -> PathBuf {
        let configured = self
            .download_dir
            .as_ref()
            .map(|dir| dir.trim())
            .filter(|dir| !dir.is_empty());

        match configured {
            Some(dir) => {
                let candidate = PathBuf::from(dir);
                if candidate.is_absolute() {
                    candidate
                } else {
                    host_app_dir().join(candidate)
                }
            }
            None => host_app_dir().join(Path::new("received_files")),
        }
    }

    pub fn apply_runtime_env(&self) {
        std::env::set_var(
            "REMOTE_DESKTOP_DEFAULT_MONITOR_INDEX",
            self.default_monitor_index.to_string(),
        );
        std::env::set_var("REMOTE_DESKTOP_DEFAULT_FPS", self.default_fps.to_string());
        std::env::set_var(
            "REMOTE_DESKTOP_DEFAULT_JPEG_QUALITY",
            self.default_jpeg_quality.to_string(),
        );
        if let Some(width) = self.default_target_width {
            std::env::set_var("REMOTE_DESKTOP_DEFAULT_TARGET_WIDTH", width.to_string());
        } else {
            std::env::remove_var("REMOTE_DESKTOP_DEFAULT_TARGET_WIDTH");
        }
        if let Some(dir) = &self.download_dir {
            if !dir.trim().is_empty() {
                std::env::set_var("REMOTE_DESKTOP_RECEIVE_DIR", dir.trim());
            }
        } else {
            std::env::remove_var("REMOTE_DESKTOP_RECEIVE_DIR");
        }
        std::env::set_var(
            "REMOTE_DESKTOP_PANIC_HOTKEY_ENABLED",
            if self.panic_hotkey_enabled { "true" } else { "false" },
        );
    }
}
