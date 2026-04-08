#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardMode {
    Manual,
    HostToClient,
    ClientToHost,
    Bidirectional,
}

#[cfg(windows)]
impl ClipboardMode {
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "host_to_client" => Some(Self::HostToClient),
            "client_to_host" => Some(Self::ClientToHost),
            "bidirectional" => Some(Self::Bidirectional),
            _ => None,
        }
    }

    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::HostToClient => "host_to_client",
            Self::ClientToHost => "client_to_host",
            Self::Bidirectional => "bidirectional",
        }
    }

    pub fn host_push_enabled(&self) -> bool {
        matches!(self, Self::HostToClient | Self::Bidirectional)
    }

    pub fn client_push_enabled(&self) -> bool {
        matches!(self, Self::ClientToHost | Self::Bidirectional)
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub target_width: Option<u32>,
    pub fps: u32,
    pub jpeg_quality: u8,
    pub view_only: bool,
    pub monitor_index: u32,
    pub clipboard_mode: ClipboardMode,
    pub delta_stream_enabled: bool,
    pub audio_enabled: bool,
}

#[cfg(windows)]
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            target_width: Some(720),
            fps: 12,
            jpeg_quality: 50,
            view_only: false,
            monitor_index: 0,
            clipboard_mode: ClipboardMode::Manual,
            delta_stream_enabled: true,
            audio_enabled: false,
        }
    }
}

#[cfg(windows)]
impl SessionConfig {
    pub fn from_runtime_defaults() -> Self {
        let mut cfg = Self::default();

        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_DEFAULT_MONITOR_INDEX") {
            if let Ok(value) = raw.parse::<u32>() {
                cfg.monitor_index = value;
            }
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_DEFAULT_FPS") {
            if let Ok(value) = raw.parse::<u32>() {
                cfg.fps = value.clamp(1, 60);
            }
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_DEFAULT_JPEG_QUALITY") {
            if let Ok(value) = raw.parse::<u8>() {
                cfg.jpeg_quality = value.clamp(1, 100);
            }
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_DEFAULT_TARGET_WIDTH") {
            if let Ok(value) = raw.parse::<u32>() {
                cfg.target_width = if value == 0 { None } else { Some(value.clamp(320, 3840)) };
            }
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_CLIPBOARD_MODE") {
            if let Some(value) = ClipboardMode::from_wire(&raw) {
                cfg.clipboard_mode = value;
            }
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_DELTA_STREAM_ENABLED") {
            cfg.delta_stream_enabled = !matches!(raw.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no");
        }
        if let Ok(raw) = std::env::var("REMOTE_DESKTOP_AUDIO_ENABLED") {
            cfg.audio_enabled = matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }

        cfg
    }

    pub fn apply_partial(
        &mut self,
        target_width: Option<u32>,
        fps: Option<u32>,
        jpeg_quality: Option<u8>,
        view_only: Option<bool>,
        monitor_index: Option<u32>,
        clipboard_mode: Option<ClipboardMode>,
        delta_stream_enabled: Option<bool>,
        audio_enabled: Option<bool>,
    ) {
        if let Some(width) = target_width {
            self.target_width = if width == 0 { None } else { Some(width.clamp(320, 3840)) };
        }
        if let Some(fps) = fps {
            self.fps = fps.clamp(1, 60);
        }
        if let Some(quality) = jpeg_quality {
            self.jpeg_quality = quality.clamp(1, 100);
        }
        if let Some(view_only) = view_only {
            self.view_only = view_only;
        }
        if let Some(monitor_index) = monitor_index {
            self.monitor_index = monitor_index;
        }
        if let Some(clipboard_mode) = clipboard_mode {
            self.clipboard_mode = clipboard_mode;
        }
        if let Some(delta_stream_enabled) = delta_stream_enabled {
            self.delta_stream_enabled = delta_stream_enabled;
        }
        if let Some(audio_enabled) = audio_enabled {
            self.audio_enabled = audio_enabled;
        }
    }

    pub fn frame_interval_ms(&self) -> u64 {
        (1000u64 / self.fps.max(1) as u64).max(1)
    }
}
