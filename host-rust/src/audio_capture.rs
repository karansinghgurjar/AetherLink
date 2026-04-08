#[cfg(windows)]
use anyhow::{Context, Result};
#[cfg(windows)]
use std::collections::VecDeque;
#[cfg(windows)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::Instant;
#[cfg(windows)]
use tokio::sync::mpsc;
#[cfg(windows)]
use wasapi::{initialize_mta, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

#[cfg(windows)]
pub const AUDIO_CODEC_PCM_S16LE: u8 = 0;
#[cfg(windows)]
const SAMPLE_RATE: u32 = 48_000;
#[cfg(windows)]
const CHANNELS: u16 = 2;
#[cfg(windows)]
const CHUNK_FRAMES: usize = 960;

#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct AudioPacket {
    pub pts_ms: u64,
    pub sequence: u32,
    pub channels: u16,
    pub sample_rate: u32,
    pub codec: u8,
    pub payload: Vec<u8>,
}

#[cfg(windows)]
pub fn start_loopback_capture(
    tx: mpsc::UnboundedSender<Result<AudioPacket, String>>,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("AetherLinkAudioLoopback".to_string())
        .spawn(move || {
            if let Err(err) = capture_loop(tx.clone(), stop_flag) {
                let _ = tx.send(Err(err.to_string()));
            }
        })
        .expect("failed to spawn audio capture thread")
}

#[cfg(windows)]
pub fn encode_audio_payload(packet: &AudioPacket) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8 + 4 + 2 + 4 + 1 + 4 + packet.payload.len());
    payload.extend_from_slice(&packet.pts_ms.to_be_bytes());
    payload.extend_from_slice(&packet.sequence.to_be_bytes());
    payload.extend_from_slice(&packet.channels.to_be_bytes());
    payload.extend_from_slice(&packet.sample_rate.to_be_bytes());
    payload.push(packet.codec);
    payload.extend_from_slice(&(packet.payload.len() as u32).to_be_bytes());
    payload.extend_from_slice(&packet.payload);
    payload
}

#[cfg(windows)]
fn capture_loop(
    tx: mpsc::UnboundedSender<Result<AudioPacket, String>>,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    let _ = initialize_mta().ok();

    let enumerator = DeviceEnumerator::new().context("create audio device enumerator")?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .context("get default render device for loopback capture")?;
    let mut audio_client = device.get_iaudioclient().context("get IAudioClient")?;

    let desired_format = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE as usize, CHANNELS as usize, None);
    let blockalign = desired_format.get_blockalign() as usize;
    let (_default_period, min_period) = audio_client.get_device_period().context("get audio device period")?;

    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_period,
    };
    audio_client
        .initialize_client(&desired_format, &Direction::Capture, &mode)
        .context("initialize WASAPI loopback client")?;

    let event_handle = audio_client.set_get_eventhandle().context("set audio event handle")?;
    let capture_client = audio_client
        .get_audiocaptureclient()
        .context("get audio capture client")?;
    let buffer_frames = audio_client.get_buffer_size().unwrap_or(CHUNK_FRAMES as u32);
    let mut byte_queue: VecDeque<u8> = VecDeque::with_capacity(buffer_frames as usize * blockalign * 4);

    audio_client.start_stream().context("start loopback capture stream")?;
    let started_at = Instant::now();
    let mut sequence = 0u32;

    while !stop_flag.load(Ordering::SeqCst) {
        while byte_queue.len() >= blockalign * CHUNK_FRAMES {
            let mut float_chunk = vec![0u8; blockalign * CHUNK_FRAMES];
            for byte in &mut float_chunk {
                *byte = byte_queue.pop_front().unwrap_or(0);
            }
            let pcm16_chunk = float32le_bytes_to_pcm16le(&float_chunk);
            let packet = AudioPacket {
                pts_ms: started_at.elapsed().as_millis() as u64,
                sequence,
                channels: CHANNELS,
                sample_rate: SAMPLE_RATE,
                codec: AUDIO_CODEC_PCM_S16LE,
                payload: pcm16_chunk,
            };
            sequence = sequence.wrapping_add(1);
            if tx.send(Ok(packet)).is_err() {
                stop_flag.store(true, Ordering::SeqCst);
                break;
            }
        }

        let next_frames = capture_client.get_next_packet_size().unwrap_or(Some(0)).unwrap_or(0);
        if next_frames > 0 {
            let additional = (next_frames as usize * blockalign)
                .saturating_sub(byte_queue.capacity().saturating_sub(byte_queue.len()));
            byte_queue.reserve(additional);
            capture_client
                .read_from_device_to_deque(&mut byte_queue)
                .context("read loopback audio frames")?;
        }

        let _ = event_handle.wait_for_event(20);
    }

    audio_client.stop_stream().ok();
    Ok(())
}

#[cfg(windows)]
fn float32le_bytes_to_pcm16le(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for sample in bytes.chunks_exact(4) {
        let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
        let scaled = (value.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    out
}


