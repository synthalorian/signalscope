use crate::{capture, dsp, display, demod, markers, plugins};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn list_devices() -> Result<()> {
    let devices = capture::list_devices()?;
    if devices.is_empty() {
        println!("No SDR devices found.");
        #[cfg(not(feature = "rtlsdr"))]
        println!("(RTL-SDR support not compiled in — rebuild with --features rtlsdr)");
    } else {
        println!("Found {} device(s):", devices.len());
        for d in devices {
            let serial = d.serial.as_deref().unwrap_or("N/A");
            println!("  [{}] {} (serial: {})", d.index, d.name, serial);
        }
    }
    Ok(())
}

pub fn spectrum(center_freq: f64, sample_rate: f64, fft_size: usize) -> Result<()> {
    println!("SignalScope Spectrum Analyzer");
    println!("Center: {:.2} MHz | Sample Rate: {:.2} MHz | FFT: {}",
        center_freq / 1e6, sample_rate / 1e6, fft_size);

    let config = capture::CaptureConfig {
        center_freq: center_freq as u64,
        sample_rate: sample_rate as u32,
        gain: 0,
        device_index: 0,
    };

    let devices = capture::list_devices()?;
    if devices.is_empty() {
        println!("No RTL-SDR devices found. Running in test signal mode.");
        println!("Press Ctrl+C to stop\n");
        run_test_spectrum(center_freq, sample_rate, fft_size)
    } else {
        println!("Using device: {}", devices[0].name);
        println!("Press Ctrl+C to stop\n");
        run_spectrum_capture(&config, fft_size)
    }
}

fn run_test_spectrum(_center_freq: f64, sample_rate: f64, fft_size: usize) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let mut waterfall = display::WaterfallDisplay::new(80, 20);

    while running.load(Ordering::SeqCst) {
        let samples = dsp::generate_test_signal(1e6, sample_rate, fft_size);
        let mut windowed = samples;
        dsp::apply_window(&mut windowed);
        let result = dsp::compute_fft(&windowed, fft_size);
        waterfall.push_row(&result);
        waterfall.render_continuous();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    println!("\nSpectrum analyzer stopped.");
    Ok(())
}

fn run_spectrum_capture(config: &capture::CaptureConfig, fft_size: usize) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let mut waterfall = display::WaterfallDisplay::new(80, 20);

    capture::capture_stream(config, fft_size, |samples| {
        let tuples = dsp::iq_to_tuple(samples);
        let mut windowed = tuples;
        dsp::apply_window(&mut windowed);
        let result = dsp::compute_fft(&windowed, fft_size);
        waterfall.push_row(&result);
        waterfall.render_continuous();
        running.load(Ordering::SeqCst)
    })?;

    println!("\nSpectrum analyzer stopped.");
    Ok(())
}

pub fn record(output: &str, duration: u64, center_freq: f64) -> Result<()> {
    println!("Recording {} seconds at {:.2} MHz to {}", duration, center_freq / 1e6, output);

    let config = capture::CaptureConfig {
        center_freq: center_freq as u64,
        sample_rate: 2_048_000,
        gain: 0,
        device_index: 0,
    };

    let samples = capture::capture_samples(&config, (duration * config.sample_rate as u64) as usize)?;

    use std::fs::File;
    use std::io::Write;
    let mut file = File::create(output)?;
    for sample in &samples {
        file.write_all(&sample.i.to_le_bytes())?;
        file.write_all(&sample.q.to_le_bytes())?;
    }

    println!("Recorded {} samples to {}", samples.len(), output);
    Ok(())
}

pub fn replay(input: &str) -> Result<()> {
    println!("Replaying from {}", input);

    use std::fs::File;
    use std::io::Read;
    let mut file = File::open(input)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let sample_count = buffer.len() / 8;
    let mut samples = Vec::with_capacity(sample_count);

    for chunk in buffer.chunks_exact(8) {
        let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        samples.push(capture::IQSample { i, q });
    }

    println!("Loaded {} samples", samples.len());

    let tuples = dsp::iq_to_tuple(&samples);
    let mut windowed = tuples;
    dsp::apply_window(&mut windowed);
    let result = dsp::compute_fft(&windowed, windowed.len().next_power_of_two());
    display::render_spectrum(&result, 80, 20);

    Ok(())
}

pub fn test_signal(freq: f64) -> Result<()> {
    println!("Generating test signal at {:.2} Hz", freq);
    let samples = dsp::generate_test_signal(freq, 2.4e6, 1024);
    let mut windowed = samples;
    dsp::apply_window(&mut windowed);
    let result = dsp::compute_fft(&windowed, 1024);
    display::render_spectrum(&result, 80, 20);
    Ok(())
}

pub fn demodulate(mode: &str, input: &str, output: &str, sample_rate: f64, carrier_offset: f64, stereo: bool) -> Result<()> {
    let demod_mode = match demod::parse_mode(mode) {
        Some(m) => m,
        None => {
            eprintln!("Unknown demodulation mode: {}. Supported: am, fm, fm-stereo, ssb, ssb-lsb", mode);
            std::process::exit(1);
        }
    };

    println!("Demodulating {} -> {} (mode: {}, sample_rate: {:.0} Hz, carrier: {:.0} Hz)",
        input, output, demod_mode, sample_rate, carrier_offset);

    use std::fs::File;
    use std::io::Read;
    let mut file = File::open(input)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let sample_count = buffer.len() / 8;
    let mut samples = Vec::with_capacity(sample_count);

    for chunk in buffer.chunks_exact(8) {
        let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        samples.push(capture::IQSample { i, q });
    }

    println!("Loaded {} IQ samples", samples.len());

    if stereo && demod_mode == demod::DemodMode::FM_STEREO {
        let mut demodulator = demod::Demodulator::new(demod_mode, sample_rate as f32)
            .with_carrier(carrier_offset as f32);
        
        let stereo_samples = demodulator.process_stereo(&samples);
        let mut stereo_samples = stereo_samples;
        
        let left: Vec<f32> = stereo_samples.iter().map(|(l, _)| *l).collect();
        let right: Vec<f32> = stereo_samples.iter().map(|(_, r)| *r).collect();
        
        let max_l = left.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
        let max_r = right.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
        let max_val = max_l.max(max_r).max(1e-10);
        
        for (l, r) in &mut stereo_samples {
            *l /= max_val;
            *r /= max_val;
        }
        
        demod::save_pcm_stereo(&stereo_samples, output)?;
        println!("Saved {} stereo sample pairs to {}", stereo_samples.len(), output);
        println!("Play with: ffplay -f f32le -ac 2 -ar {} {}", sample_rate as u32, output);
    } else {
        let mut demodulator = demod::Demodulator::new(demod_mode, sample_rate as f32)
            .with_carrier(carrier_offset as f32);

        let demodulated = demodulator.process(&samples);
        
        let mut audio = demodulated;
        demod::normalize_audio(&mut audio);
        demod::save_pcm(&audio, output)?;

        println!("Saved {} demodulated samples to {}", audio.len(), output);
        println!("Play with: ffplay -f f32le -ar {} {}", sample_rate as u32, output);
    }
    
    Ok(())
}

pub fn peak_list(input: &str, sample_rate: f64, center_freq: f64, fft_size: usize, min_prominence: f32, max_peaks: usize) -> Result<()> {
    use std::fs::File;
    use std::io::Read;
    
    let mut file = File::open(input)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;

    let sample_count = buffer.len() / 8;
    let mut samples = Vec::with_capacity(sample_count);

    for chunk in buffer.chunks_exact(8) {
        let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        samples.push(capture::IQSample { i, q });
    }

    println!("Loaded {} IQ samples from {}", samples.len(), input);
    println!("Analyzing with FFT size {}...", fft_size);

    let tuples = dsp::iq_to_tuple(&samples);
    let mut windowed = tuples;
    dsp::apply_window(&mut windowed);
    let result = dsp::compute_fft(&windowed, fft_size);

    let config = dsp::PeakDetectConfig {
        min_prominence_db: min_prominence,
        min_distance_bins: 5,
        noise_window_bins: 10,
        max_peaks,
    };

    let peaks = dsp::detect_peaks(&result, sample_rate, center_freq, &config);
    
    if peaks.is_empty() {
        println!("No peaks detected with prominence >= {:.1} dB", min_prominence);
    } else {
        println!("\nDetected {} peak(s):", peaks.len());
        println!("{:<6} {:<16} {:<12} {:<12}", "#", "Frequency", "Amplitude", "SNR");
        println!("{}", "-".repeat(50));
        for (i, peak) in peaks.iter().enumerate() {
            let freq_str = format_frequency(peak.frequency_hz);
            println!("{:<6} {:<16} {:<12.1} {:<12.1}", 
                i + 1, freq_str, peak.amplitude_db, peak.snr_db);
        }
    }

    Ok(())
}

const DEFAULT_MARKER_FILE: &str = "/home/synth/.config/signalscope/markers.json";

fn ensure_marker_dir() -> Result<()> {
    if let Some(parent) = std::path::Path::new(DEFAULT_MARKER_FILE).parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn format_frequency(freq_hz: f64) -> String {
    let f = freq_hz.abs();
    if f >= 1e9 {
        format!("{:.3} GHz", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.3} MHz", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.3} kHz", f / 1e3)
    } else {
        format!("{:.1} Hz", f)
    }
}

pub fn marker_add(frequency: f64, label: Option<String>, snap_file: Option<String>, snap_sample_rate: f64) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(DEFAULT_MARKER_FILE.to_string());
    if let Ok(_) = std::fs::metadata(DEFAULT_MARKER_FILE) {
        let _ = manager.load(DEFAULT_MARKER_FILE);
    }

    let final_freq = if let Some(file) = snap_file {
        println!("Snapping to nearest peak in {}...", file);
        match snap_marker_to_peak(&file, snap_sample_rate, frequency) {
            Some(peak_freq) => {
                println!("Snapped from {} to {} (nearest peak)", 
                    format_frequency(frequency), format_frequency(peak_freq));
                peak_freq
            }
            None => {
                println!("No peak found near {}, using original frequency", format_frequency(frequency));
                frequency
            }
        }
    } else {
        frequency
    };

    let mut marker = markers::FrequencyMarker::new(final_freq);
    if let Some(l) = label {
        marker = marker.with_label(l);
    }

    let id = manager.add(marker);
    println!("Added marker #{} at {}", id, format_frequency(final_freq));
    Ok(())
}

fn snap_marker_to_peak(file: &str, sample_rate: f64, target_freq: f64) -> Option<f64> {
    use std::fs::File;
    use std::io::Read;
    
    let mut file = File::open(file).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;

    let sample_count = buffer.len() / 8;
    let mut samples = Vec::with_capacity(sample_count);

    for chunk in buffer.chunks_exact(8) {
        let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        samples.push(capture::IQSample { i, q });
    }

    let fft_size = 2048usize;
    let tuples = dsp::iq_to_tuple(&samples);
    let mut windowed = tuples;
    dsp::apply_window(&mut windowed);
    let result = dsp::compute_fft(&windowed, fft_size);

    let config = dsp::PeakDetectConfig {
        min_prominence_db: 5.0,
        min_distance_bins: 3,
        noise_window_bins: 10,
        max_peaks: 50,
    };

    let peaks = dsp::detect_peaks(&result, sample_rate, 0.0, &config);
    
    peaks.into_iter()
        .min_by(|a, b| {
            let da = (a.frequency_hz - target_freq).abs();
            let db = (b.frequency_hz - target_freq).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|p| p.frequency_hz)
}

pub fn marker_list(file: Option<String>) -> Result<()> {
    let path = file.as_deref().unwrap_or(DEFAULT_MARKER_FILE);
    let manager = if let Ok(_) = std::fs::metadata(path) {
        let mut m = markers::MarkerManager::new();
        let _ = m.load(path);
        m
    } else {
        markers::MarkerManager::new()
    };

    let all = manager.all();
    if all.is_empty() {
        println!("No markers found.");
    } else {
        println!("{} marker(s):", all.len());
        for marker in all {
            let label = marker.label.as_deref().unwrap_or("(no label)");
            let amp = marker.amplitude_db.map(|a| format!("{:.1} dB", a)).unwrap_or_else(|| "N/A".to_string());
            println!("  {} — {} ({})", marker.format_frequency(), label, amp);
        }
    }
    Ok(())
}

pub fn marker_remove(frequency: f64) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(DEFAULT_MARKER_FILE.to_string());
    if let Ok(_) = std::fs::metadata(DEFAULT_MARKER_FILE) {
        let _ = manager.load(DEFAULT_MARKER_FILE);
    }

    if let Some((id, _)) = manager.find_near(frequency, 1.0) {
        manager.remove(id);
        println!("Removed marker at {}", markers::FrequencyMarker::new(frequency).format_frequency());
    } else {
        println!("No marker found near {}", markers::FrequencyMarker::new(frequency).format_frequency());
    }
    Ok(())
}

pub fn marker_clear() -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(DEFAULT_MARKER_FILE.to_string());
    manager.clear();
    println!("All markers cleared.");
    Ok(())
}

pub fn marker_import(file: &str) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(DEFAULT_MARKER_FILE.to_string());
    if let Ok(_) = std::fs::metadata(DEFAULT_MARKER_FILE) {
        let _ = manager.load(DEFAULT_MARKER_FILE);
    }

    let count = manager.import_csv(file)?;
    println!("Imported {} markers from {}", count, file);
    Ok(())
}

pub fn marker_export(file: &str) -> Result<()> {
    let mut manager = markers::MarkerManager::new();
    if let Ok(_) = std::fs::metadata(DEFAULT_MARKER_FILE) {
        let _ = manager.load(DEFAULT_MARKER_FILE);
    }

    manager.export_csv(file)?;
    println!("Exported {} markers to {}", manager.bank().len(), file);
    Ok(())
}

pub fn plugin_list() -> Result<()> {
    let manager = plugins::PluginManager::default();
    
    println!("Built-in demodulation plugins:");
    for (name, desc) in manager.list_demod_plugins() {
        println!("  [demod] {} — {}", name, desc);
    }
    
    println!("\nBuilt-in DSP plugins:");
    println!("  [dsp] gain — Simple gain/attenuation (param: gain_db)");
    println!("  [dsp] noise_gate — Noise gate (param: threshold_db)");
    
    println!("\nDynamic DSP plugins: {} loaded", manager.dsp_count());
    Ok(())
}

pub fn plugin_load(path: &str) -> Result<()> {
    let mut manager = plugins::PluginManager::new();
    let idx = manager.load_plugin(path)?;
    println!("Loaded plugin at index {}", idx);
    Ok(())
}

pub fn plugin_scan() -> Result<()> {
    println!("Scanning for plugins...");
    let mut manager = plugins::PluginManager::new();
    manager.add_path("/usr/lib/signalscope/plugins");
    manager.add_path("/home/synth/.local/lib/signalscope/plugins");
    manager.add_path("./plugins");
    let count = manager.scan_and_load()?;
    println!("Loaded {} plugin(s)", count);
    for (name, idx) in manager.list_dsp() {
        println!("  [{}] {}", idx, name);
    }
    Ok(())
}

pub fn plugin_set_param(index: usize, key: &str, value: f64) -> Result<()> {
    println!("Set plugin[{}].{} = {}", index, key, value);
    Ok(())
}
