use anyhow::{Context, Result};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// UDP streaming configuration
#[derive(Debug, Clone)]
pub struct UdpStreamConfig {
    pub host: String,
    pub port: u16,
    pub format: StreamFormat,
    pub packet_size: usize,
}

/// Streaming format
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamFormat {
    /// Raw interleaved IQ samples (f32 little-endian)
    RawIQ,
    /// Demodulated audio (f32 little-endian)
    AudioF32,
    /// Demodulated audio (i16 little-endian)
    AudioI16,
}

impl std::fmt::Display for StreamFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamFormat::RawIQ => write!(f, "raw-iq-f32"),
            StreamFormat::AudioF32 => write!(f, "audio-f32"),
            StreamFormat::AudioI16 => write!(f, "audio-i16"),
        }
    }
}

impl UdpStreamConfig {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            format: StreamFormat::RawIQ,
            packet_size: 1472, // Max UDP payload to avoid fragmentation
        }
    }

    pub fn with_format(mut self, format: StreamFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_packet_size(mut self, size: usize) -> Self {
        self.packet_size = size.clamp(64, 65507);
        self
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// UDP streamer for IQ or audio data
pub struct UdpStreamer {
    socket: UdpSocket,
    config: UdpStreamConfig,
    sequence_number: u32,
    packets_sent: u64,
    bytes_sent: u64,
}

impl UdpStreamer {
    pub fn new(config: UdpStreamConfig) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0").context("Failed to bind UDP socket")?;

        socket
            .connect(config.address())
            .with_context(|| format!("Failed to connect UDP socket to {}", config.address()))?;

        Ok(Self {
            socket,
            config,
            sequence_number: 0,
            packets_sent: 0,
            bytes_sent: 0,
        })
    }

    /// Stream a block of IQ samples
    /// Samples are sent as interleaved f32: [I0, Q0, I1, Q1, ...]
    pub fn stream_iq(&mut self, samples: &[(f32, f32)]) -> Result<usize> {
        match self.config.format {
            StreamFormat::RawIQ => {
                let mut bytes = Vec::with_capacity(samples.len() * 8);
                for (i, q) in samples {
                    bytes.extend_from_slice(&i.to_le_bytes());
                    bytes.extend_from_slice(&q.to_le_bytes());
                }
                self.send_chunks(&bytes)
            }
            StreamFormat::AudioF32 | StreamFormat::AudioI16 => {
                // For audio formats from IQ, convert to magnitude
                let mut bytes = Vec::with_capacity(samples.len() * 4);
                for (i, q) in samples {
                    let mag = (i * i + q * q).sqrt();
                    bytes.extend_from_slice(&mag.to_le_bytes());
                }
                self.send_chunks(&bytes)
            }
        }
    }

    /// Stream audio samples (f32)
    pub fn stream_audio_f32(&mut self, samples: &[f32]) -> Result<usize> {
        match self.config.format {
            StreamFormat::AudioF32 | StreamFormat::RawIQ => {
                let mut bytes = Vec::with_capacity(samples.len() * 4);
                for &sample in samples {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                self.send_chunks(&bytes)
            }
            StreamFormat::AudioI16 => {
                let mut bytes = Vec::with_capacity(samples.len() * 2);
                for &sample in samples {
                    let s16 = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    bytes.extend_from_slice(&s16.to_le_bytes());
                }
                self.send_chunks(&bytes)
            }
        }
    }

    /// Stream stereo audio samples (interleaved L/R)
    pub fn stream_stereo_audio(&mut self, samples: &[(f32, f32)]) -> Result<usize> {
        let mut bytes = Vec::with_capacity(samples.len() * 8);

        match self.config.format {
            StreamFormat::AudioF32 | StreamFormat::RawIQ => {
                for (l, r) in samples {
                    bytes.extend_from_slice(&l.to_le_bytes());
                    bytes.extend_from_slice(&r.to_le_bytes());
                }
            }
            StreamFormat::AudioI16 => {
                for (l, r) in samples {
                    let l16 = (l.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    let r16 = (r.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    bytes.extend_from_slice(&l16.to_le_bytes());
                    bytes.extend_from_slice(&r16.to_le_bytes());
                }
            }
        }

        self.send_chunks(&bytes)
    }

    fn send_chunks(&mut self, data: &[u8]) -> Result<usize> {
        let chunk_size = self.config.packet_size;
        let mut total_sent = 0usize;

        for chunk in data.chunks(chunk_size) {
            // Add a simple header: 4 bytes sequence number
            let mut packet = Vec::with_capacity(chunk.len() + 4);
            packet.extend_from_slice(&self.sequence_number.to_le_bytes());
            packet.extend_from_slice(chunk);

            match self.socket.send(&packet) {
                Ok(sent) => {
                    total_sent += sent.saturating_sub(4);
                    self.packets_sent += 1;
                    self.bytes_sent += sent as u64;
                }
                Err(e) => {
                    eprintln!("UDP send error: {}", e);
                    break;
                }
            }

            self.sequence_number = self.sequence_number.wrapping_add(1);
        }

        Ok(total_sent)
    }

    pub fn stats(&self) -> StreamStats {
        StreamStats {
            packets_sent: self.packets_sent,
            bytes_sent: self.bytes_sent,
            sequence_number: self.sequence_number,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StreamStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub sequence_number: u32,
}

/// Stream IQ samples from a capture source continuously
pub fn stream_iq_from_source<F>(
    config: &UdpStreamConfig,
    mut source: F,
    running: Arc<AtomicBool>,
) -> Result<()>
where
    F: FnMut() -> Option<Vec<(f32, f32)>>,
{
    let mut streamer = UdpStreamer::new(config.clone())?;
    println!(
        "Streaming IQ to {} (format: {})",
        config.address(),
        config.format
    );
    println!("Press Ctrl+C to stop");

    while running.load(Ordering::SeqCst) {
        if let Some(samples) = source() {
            streamer.stream_iq(&samples)?;
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let stats = streamer.stats();
    println!(
        "\nStreamed {} packets ({} bytes)",
        stats.packets_sent, stats.bytes_sent
    );
    Ok(())
}

/// Parse stream format from string
pub fn parse_stream_format(s: &str) -> Option<StreamFormat> {
    match s.to_lowercase().as_str() {
        "raw-iq" | "iq" | "raw_iq" | "raw-iq-f32" => Some(StreamFormat::RawIQ),
        "audio" | "audio-f32" | "audio_f32" => Some(StreamFormat::AudioF32),
        "audio-i16" | "audio_i16" => Some(StreamFormat::AudioI16),
        _ => None,
    }
}
