use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct SdrDevice {
    pub index: usize,
    pub name: String,
    pub serial: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IQSample {
    pub i: f32,
    pub q: f32,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub center_freq: u64,
    pub sample_rate: u32,
    pub gain: i32,
    pub device_index: i32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            center_freq: 100_000_000, // 100 MHz
            sample_rate: 2_048_000,   // 2.048 MHz
            gain: 0,                  // auto
            device_index: 0,
        }
    }
}

#[cfg(feature = "rtlsdr")]
pub fn list_devices() -> Result<Vec<SdrDevice>> {
    let count = rtlsdr::get_device_count();
    if count == 0 {
        return Ok(vec![]);
    }

    let mut devices = Vec::with_capacity(count as usize);
    for i in 0..count {
        let name = rtlsdr::get_device_name(i);
        let serial = match rtlsdr::get_device_usb_strings(i) {
            Ok(strings) => Some(strings.serial),
            Err(_) => None,
        };
        devices.push(SdrDevice {
            index: i as usize,
            name,
            serial,
        });
    }
    Ok(devices)
}

#[cfg(not(feature = "rtlsdr"))]
pub fn list_devices() -> Result<Vec<SdrDevice>> {
    Ok(vec![])
}

#[cfg(feature = "rtlsdr")]
pub fn capture_samples(config: &CaptureConfig, count: usize) -> Result<Vec<IQSample>> {
    let mut device = rtlsdr::open(config.device_index)
        .map_err(|e| anyhow!("Failed to open RTL-SDR device: {}", e))?;

    // Set sample rate
    device
        .set_sample_rate(config.sample_rate)
        .map_err(|e| anyhow!("Failed to set sample rate: {}", e))?;

    // Set center frequency
    device
        .set_center_freq(config.center_freq as u32)
        .map_err(|e| anyhow!("Failed to set center frequency: {}", e))?;

    // Configure gain
    if config.gain > 0 {
        device
            .set_tuner_gain_mode(true)
            .map_err(|e| anyhow!("Failed to set gain mode: {}", e))?;
        device
            .set_tuner_gain(config.gain)
            .map_err(|e| anyhow!("Failed to set gain: {}", e))?;
    } else {
        device
            .set_tuner_gain_mode(false)
            .map_err(|e| anyhow!("Failed to set auto gain: {}", e))?;
    }

    // Reset buffer before reading
    device
        .reset_buffer()
        .map_err(|e| anyhow!("Failed to reset buffer: {}", e))?;

    // Read raw IQ samples
    // RTL-SDR returns interleaved unsigned bytes: [I, Q, I, Q, ...]
    // Each sample is 2 bytes, so request count * 2 bytes
    let bytes_needed = count * 2;
    let raw = device
        .read_sync(bytes_needed)
        .map_err(|e| anyhow!("Failed to read samples: {}", e))?;

    // Convert raw bytes to normalized f32 IQ samples
    let samples: Vec<IQSample> = raw
        .chunks_exact(2)
        .map(|chunk| {
            let i = (chunk[0] as f32 - 127.5) / 127.5;
            let q = (chunk[1] as f32 - 127.5) / 127.5;
            IQSample { i, q }
        })
        .collect();

    Ok(samples)
}

#[cfg(not(feature = "rtlsdr"))]
pub fn capture_samples(_config: &CaptureConfig, _count: usize) -> Result<Vec<IQSample>> {
    Err(anyhow!(
        "RTL-SDR support not compiled in. Rebuild with --features rtlsdr"
    ))
}

/// Capture samples from RTL-SDR as a continuous stream, calling the provided
/// callback for each chunk.
#[cfg(feature = "rtlsdr")]
pub fn capture_stream<F>(config: &CaptureConfig, chunk_size: usize, mut callback: F) -> Result<()>
where
    F: FnMut(&[IQSample]) -> bool,
{
    let mut device = rtlsdr::open(config.device_index)
        .map_err(|e| anyhow!("Failed to open RTL-SDR device: {}", e))?;

    // Set sample rate
    device
        .set_sample_rate(config.sample_rate)
        .map_err(|e| anyhow!("Failed to set sample rate: {}", e))?;

    // Set center frequency
    device
        .set_center_freq(config.center_freq as u32)
        .map_err(|e| anyhow!("Failed to set center frequency: {}", e))?;

    // Configure gain
    if config.gain > 0 {
        device
            .set_tuner_gain_mode(true)
            .map_err(|e| anyhow!("Failed to set gain mode: {}", e))?;
        device
            .set_tuner_gain(config.gain)
            .map_err(|e| anyhow!("Failed to set gain: {}", e))?;
    } else {
        device
            .set_tuner_gain_mode(false)
            .map_err(|e| anyhow!("Failed to set auto gain: {}", e))?;
    }

    // Reset buffer before streaming
    device
        .reset_buffer()
        .map_err(|e| anyhow!("Failed to reset buffer: {}", e))?;

    let bytes_needed = chunk_size * 2;

    loop {
        let raw = device
            .read_sync(bytes_needed)
            .map_err(|e| anyhow!("Failed to read samples: {}", e))?;

        let samples: Vec<IQSample> = raw
            .chunks_exact(2)
            .map(|chunk| {
                let i = (chunk[0] as f32 - 127.5) / 127.5;
                let q = (chunk[1] as f32 - 127.5) / 127.5;
                IQSample { i, q }
            })
            .collect();

        if !callback(&samples) {
            break;
        }
    }

    Ok(())
}

#[cfg(not(feature = "rtlsdr"))]
pub fn capture_stream<F>(_config: &CaptureConfig, _chunk_size: usize, _callback: F) -> Result<()>
where
    F: FnMut(&[IQSample]) -> bool,
{
    Err(anyhow!(
        "RTL-SDR support not compiled in. Rebuild with --features rtlsdr"
    ))
}
