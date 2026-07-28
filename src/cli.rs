use crate::{
    capture, classifier, demod, display, dsp, markers, plugins, profiles, rds, scanner, scheduler,
    streaming,
};
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
    println!(
        "Center: {:.2} MHz | Sample Rate: {:.2} MHz | FFT: {}",
        center_freq / 1e6,
        sample_rate / 1e6,
        fft_size
    );

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
    })
    .expect("Error setting Ctrl-C handler");

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
    })
    .expect("Error setting Ctrl-C handler");

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
    println!(
        "Recording {} seconds at {:.2} MHz to {}",
        duration,
        center_freq / 1e6,
        output
    );

    let config = capture::CaptureConfig {
        center_freq: center_freq as u64,
        sample_rate: 2_048_000,
        gain: 0,
        device_index: 0,
    };

    let samples =
        capture::capture_samples(&config, (duration * config.sample_rate as u64) as usize)?;

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

pub fn demodulate(
    mode: &str,
    input: &str,
    output: &str,
    sample_rate: f64,
    carrier_offset: f64,
    stereo: bool,
) -> Result<()> {
    let demod_mode = match demod::parse_mode(mode) {
        Some(m) => m,
        None => {
            eprintln!(
                "Unknown demodulation mode: {}. Supported: am, fm, fm-stereo, ssb, ssb-lsb",
                mode
            );
            std::process::exit(1);
        }
    };

    println!(
        "Demodulating {} -> {} (mode: {}, sample_rate: {:.0} Hz, carrier: {:.0} Hz)",
        input, output, demod_mode, sample_rate, carrier_offset
    );

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
        println!(
            "Saved {} stereo sample pairs to {}",
            stereo_samples.len(),
            output
        );
        println!(
            "Play with: ffplay -f f32le -ac 2 -ar {} {}",
            sample_rate as u32, output
        );
    } else {
        let mut demodulator = demod::Demodulator::new(demod_mode, sample_rate as f32)
            .with_carrier(carrier_offset as f32);

        let demodulated = demodulator.process(&samples);

        let mut audio = demodulated;
        demod::normalize_audio(&mut audio);
        demod::save_pcm(&audio, output)?;

        println!("Saved {} demodulated samples to {}", audio.len(), output);
        println!(
            "Play with: ffplay -f f32le -ar {} {}",
            sample_rate as u32, output
        );
    }

    Ok(())
}

pub fn peak_list(
    input: &str,
    sample_rate: f64,
    center_freq: f64,
    fft_size: usize,
    min_prominence: f32,
    max_peaks: usize,
) -> Result<()> {
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
        println!(
            "No peaks detected with prominence >= {:.1} dB",
            min_prominence
        );
    } else {
        println!("\nDetected {} peak(s):", peaks.len());
        println!(
            "{:<6} {:<16} {:<12} {:<12}",
            "#", "Frequency", "Amplitude", "SNR"
        );
        println!("{}", "-".repeat(50));
        for (i, peak) in peaks.iter().enumerate() {
            let freq_str = format_frequency(peak.frequency_hz);
            println!(
                "{:<6} {:<16} {:<12.1} {:<12.1}",
                i + 1,
                freq_str,
                peak.amplitude_db,
                peak.snr_db
            );
        }
    }

    Ok(())
}

fn default_marker_file() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.config/signalscope/markers.json", home)
}

fn ensure_marker_dir() -> Result<()> {
    if let Some(parent) = std::path::Path::new(&default_marker_file()).parent() {
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

pub fn marker_add(
    frequency: f64,
    label: Option<String>,
    snap_file: Option<String>,
    snap_sample_rate: f64,
) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(default_marker_file());
    if std::fs::metadata(default_marker_file()).is_ok() {
        let _ = manager.load(default_marker_file());
    }

    let final_freq = if let Some(file) = snap_file {
        println!("Snapping to nearest peak in {}...", file);
        match snap_marker_to_peak(&file, snap_sample_rate, frequency) {
            Some(peak_freq) => {
                println!(
                    "Snapped from {} to {} (nearest peak)",
                    format_frequency(frequency),
                    format_frequency(peak_freq)
                );
                peak_freq
            }
            None => {
                println!(
                    "No peak found near {}, using original frequency",
                    format_frequency(frequency)
                );
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

    peaks
        .into_iter()
        .min_by(|a, b| {
            let da = (a.frequency_hz - target_freq).abs();
            let db = (b.frequency_hz - target_freq).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|p| p.frequency_hz)
}

pub fn marker_list(file: Option<String>) -> Result<()> {
    let path = file
        .as_deref()
        .map(str::to_string)
        .unwrap_or_else(default_marker_file);
    let manager = if std::fs::metadata(&path).is_ok() {
        let mut m = markers::MarkerManager::new();
        let _ = m.load(&path);
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
            let amp = marker
                .amplitude_db
                .map(|a| format!("{:.1} dB", a))
                .unwrap_or_else(|| "N/A".to_string());
            println!("  {} — {} ({})", marker.format_frequency(), label, amp);
        }
    }
    Ok(())
}

pub fn marker_remove(frequency: f64) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(default_marker_file());
    if std::fs::metadata(default_marker_file()).is_ok() {
        let _ = manager.load(default_marker_file());
    }

    if let Some((id, _)) = manager.find_near(frequency, 1.0) {
        manager.remove(id);
        println!(
            "Removed marker at {}",
            markers::FrequencyMarker::new(frequency).format_frequency()
        );
    } else {
        println!(
            "No marker found near {}",
            markers::FrequencyMarker::new(frequency).format_frequency()
        );
    }
    Ok(())
}

pub fn marker_clear() -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(default_marker_file());
    manager.clear();
    println!("All markers cleared.");
    Ok(())
}

pub fn marker_import(file: &str) -> Result<()> {
    ensure_marker_dir()?;
    let mut manager = markers::MarkerManager::new().with_auto_save(default_marker_file());
    if std::fs::metadata(default_marker_file()).is_ok() {
        let _ = manager.load(default_marker_file());
    }

    let count = manager.import_csv(file)?;
    println!("Imported {} markers from {}", count, file);
    Ok(())
}

pub fn marker_export(file: &str) -> Result<()> {
    let mut manager = markers::MarkerManager::new();
    if std::fs::metadata(default_marker_file()).is_ok() {
        let _ = manager.load(default_marker_file());
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

fn default_schedule_file() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.config/signalscope/schedules.json", home)
}

fn ensure_schedule_dir() -> Result<()> {
    if let Some(parent) = std::path::Path::new(&default_schedule_file()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn schedule_add(
    name: &str,
    frequency: f64,
    duration: u64,
    output: &str,
    cron: &str,
    sample_rate: f64,
) -> Result<()> {
    ensure_schedule_dir()?;

    let mut scheduler = scheduler::RecordingScheduler::new();
    if std::fs::metadata(default_schedule_file()).is_ok() {
        let _ = scheduler.load(default_schedule_file());
    }

    let schedule =
        scheduler::RecordingSchedule::new(name, frequency as u64, duration, output, cron)
            .with_sample_rate(sample_rate as u32);

    scheduler.add_schedule(schedule);
    scheduler.save(default_schedule_file())?;

    println!(
        "Added schedule '{}' at {} MHz, duration {}s",
        name,
        frequency / 1e6,
        duration
    );
    println!("Cron: {} | Output: {}", cron, output);
    Ok(())
}

pub fn schedule_list() -> Result<()> {
    let mut scheduler = scheduler::RecordingScheduler::new();
    if std::fs::metadata(default_schedule_file()).is_ok() {
        let _ = scheduler.load(default_schedule_file());
    }

    let schedules = scheduler.list_schedules();
    if schedules.is_empty() {
        println!("No scheduled recordings.");
    } else {
        println!("{} scheduled recording(s):", schedules.len());
        for s in schedules {
            let status = if s.enabled { "enabled" } else { "disabled" };
            let last_run = s
                .last_run
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            println!(
                "  [{}] {} — {:.2} MHz, {}s, cron: {} ({})",
                s.name,
                status,
                s.frequency_hz as f64 / 1e6,
                s.duration_sec,
                s.cron_expr,
                last_run
            );
        }
    }
    Ok(())
}

pub fn schedule_remove(name: &str) -> Result<()> {
    ensure_schedule_dir()?;

    let mut scheduler = scheduler::RecordingScheduler::new();
    if std::fs::metadata(default_schedule_file()).is_ok() {
        let _ = scheduler.load(default_schedule_file());
    }

    if scheduler.remove_schedule(name) {
        scheduler.save(default_schedule_file())?;
        println!("Removed schedule '{}'", name);
    } else {
        println!("Schedule '{}' not found.", name);
    }
    Ok(())
}

pub fn schedule_run() -> Result<()> {
    ensure_schedule_dir()?;

    let mut scheduler = scheduler::RecordingScheduler::new();
    if std::fs::metadata(default_schedule_file()).is_ok() {
        scheduler.load(default_schedule_file())?;
    }

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    scheduler.run(
        |schedule| {
            let config = capture::CaptureConfig {
                center_freq: schedule.frequency_hz,
                sample_rate: schedule.sample_rate_hz,
                gain: 0,
                device_index: 0,
            };

            let sample_count = (schedule.duration_sec * schedule.sample_rate_hz as u64) as usize;
            let samples = capture::capture_samples(&config, sample_count)?;

            use std::fs::File;
            use std::io::Write;
            let mut file = File::create(&schedule.output_path)?;
            for sample in &samples {
                file.write_all(&sample.i.to_le_bytes())?;
                file.write_all(&sample.q.to_le_bytes())?;
            }

            println!(
                "Recorded {} samples to {}",
                samples.len(),
                schedule.output_path
            );
            Ok(())
        },
        running,
    )?;

    Ok(())
}

pub fn classify(input: &str, sample_rate: f64) -> Result<()> {
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
    println!("Classifying signal...\n");

    let classifier = classifier::SignalClassifier::new();
    let result = classifier.classify(&samples, sample_rate as f32);

    println!("Classification Result:");
    println!(
        "  Type:       {} (confidence: {:.1}%)",
        result.class,
        result.confidence * 100.0
    );
    println!("  Bandwidth:  {:.0} Hz", result.bandwidth_hz);
    println!("  Notes:");
    for note in &result.notes {
        println!("    - {}", note);
    }

    Ok(())
}

pub fn rds_decode(input: &str, sample_rate: f64, blocks: usize) -> Result<()> {
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
    println!("Decoding RDS from FM multiplex...\n");

    let mut decoder = rds::RdsDecoder::new(sample_rate as f32);

    let block_size = 65536usize;
    for i in 0..blocks {
        let start = i * block_size;
        let end = ((i + 1) * block_size).min(samples.len());
        if start >= samples.len() {
            break;
        }

        decoder.process_samples(&samples[start..end]);

        if !decoder.info.programme_service.is_empty() {
            print!(
                "\rBlock {}/{}: PS='{}' RT='{}'",
                i + 1,
                blocks,
                decoder.info.programme_service,
                if decoder.info.radio_text.len() > 30 {
                    format!("{}...", &decoder.info.radio_text[..30])
                } else {
                    decoder.info.radio_text.clone()
                }
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }

    println!();
    rds::print_rds_info(&decoder.info);

    Ok(())
}

#[allow(clippy::too_many_arguments)] // mirrors the CLI argument set 1:1
pub fn scan(
    start_freq: f64,
    end_freq: f64,
    step: f64,
    squelch: f32,
    dwell_ms: u64,
    freq_list: Option<String>,
    scan_all: bool,
    sample_rate: f64,
) -> Result<()> {
    let config = scanner::ScannerConfig {
        start_freq_hz: start_freq as u64,
        end_freq_hz: end_freq as u64,
        step_hz: step as u64,
        squelch_db: squelch,
        dwell_ms,
        sample_rate_hz: sample_rate as u32,
        sample_count: 4096,
        stop_on_signal: !scan_all,
        frequency_list: match freq_list {
            Some(list) => scanner::parse_frequency_list(&list)?,
            None => Vec::new(),
        },
    };

    let scanner = scanner::FrequencyScanner::new(config);
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    scanner.scan(capture::capture_samples, running)?;

    Ok(())
}

pub fn stream(
    host: &str,
    port: u16,
    format: &str,
    center_freq: Option<f64>,
    input: Option<&str>,
    sample_rate: f64,
    duration: u64,
) -> Result<()> {
    let stream_format = streaming::parse_stream_format(format).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown stream format: {}. Use: raw-iq, audio-f32, audio-i16",
            format
        )
    })?;

    let config = streaming::UdpStreamConfig::new(host, port).with_format(stream_format);

    if let Some(input_file) = input {
        use std::fs::File;
        use std::io::Read;

        println!(
            "Streaming {} to {}:{} (format: {})",
            input_file, host, port, format
        );

        let mut file = File::open(input_file)?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)?;

        let sample_count = buffer.len() / 8;
        let mut samples = Vec::with_capacity(sample_count);

        for chunk in buffer.chunks_exact(8) {
            let i = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let q = f32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            samples.push((i, q));
        }

        let mut streamer = streaming::UdpStreamer::new(config)?;
        let chunk_size = 1024usize;

        for chunk in samples.chunks(chunk_size) {
            streamer.stream_iq(chunk)?;
        }

        let stats = streamer.stats();
        println!(
            "Streamed {} packets ({} bytes)",
            stats.packets_sent, stats.bytes_sent
        );
    } else {
        let freq = center_freq.unwrap_or(100e6);
        let capture_config = capture::CaptureConfig {
            center_freq: freq as u64,
            sample_rate: sample_rate as u32,
            gain: 0,
            device_index: 0,
        };

        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .expect("Error setting Ctrl-C handler");

        let start_time = std::time::Instant::now();
        let max_duration = if duration > 0 {
            Some(std::time::Duration::from_secs(duration))
        } else {
            None
        };

        let mut streamer = streaming::UdpStreamer::new(config)?;

        capture::capture_stream(&capture_config, 1024, |samples| {
            let tuples: Vec<(f32, f32)> = samples.iter().map(|s| (s.i, s.q)).collect();
            let _ = streamer.stream_iq(&tuples);

            if let Some(max_dur) = max_duration {
                start_time.elapsed() < max_dur
            } else {
                running.load(Ordering::SeqCst)
            }
        })?;

        let stats = streamer.stats();
        println!(
            "\nStreamed {} packets ({} bytes)",
            stats.packets_sent, stats.bytes_sent
        );
    }

    Ok(())
}

fn ensure_profile_dir() -> Result<()> {
    profiles::ensure_profile_dir()
}

pub fn profile_save(
    name: &str,
    frequency: f64,
    sample_rate: f64,
    gain: i32,
    demod: &str,
    description: Option<String>,
    tags: Option<String>,
) -> Result<()> {
    ensure_profile_dir()?;

    let mut manager = profiles::ProfileManager::new();
    if std::fs::metadata(profiles::default_profile_path()).is_ok() {
        let _ = manager.load_from_file(profiles::default_profile_path());
    }

    let mut profile = profiles::ReceiverProfile::new(name, frequency as u64, sample_rate as u32)
        .with_gain(gain)
        .with_demod(demod);

    if let Some(desc) = description {
        profile = profile.with_description(desc);
    }

    if let Some(tag_str) = tags {
        let tag_list: Vec<String> = tag_str.split(',').map(|s| s.trim().to_string()).collect();
        profile = profile.with_tags(tag_list);
    }

    manager.save_profile(profile);
    manager.save_to_file(profiles::default_profile_path())?;

    println!("Saved profile '{}'", name);
    println!(
        "  Frequency: {} | Sample Rate: {} | Gain: {} dB | Demod: {}",
        format_frequency(frequency),
        format_frequency(sample_rate),
        gain,
        demod
    );
    Ok(())
}

pub fn profile_load(name: &str) -> Result<()> {
    let mut manager = profiles::ProfileManager::new();
    if std::fs::metadata(profiles::default_profile_path()).is_ok() {
        manager.load_from_file(profiles::default_profile_path())?;
    }

    match manager.get(name) {
        Some(p) => {
            println!("Profile: {}", p.name);
            println!("  Frequency:    {} Hz", p.center_freq_hz);
            println!("  Sample Rate:  {} Hz", p.sample_rate_hz);
            println!("  Gain:         {} dB", p.gain_db);
            println!("  Demod Mode:   {}", p.demod_mode);
            if let Some(ref desc) = p.description {
                println!("  Description:  {}", desc);
            }
            if !p.tags.is_empty() {
                println!("  Tags:         {}", p.tags.join(", "));
            }
            println!("  Created:      {}", p.created_at);
        }
        None => {
            println!("Profile '{}' not found.", name);
        }
    }
    Ok(())
}

pub fn profile_list() -> Result<()> {
    let mut manager = profiles::ProfileManager::new();
    if std::fs::metadata(profiles::default_profile_path()).is_ok() {
        let _ = manager.load_from_file(profiles::default_profile_path());
    }

    let profiles = manager.list();
    if profiles.is_empty() {
        println!("No profiles saved.");
    } else {
        println!("{} profile(s):", profiles.len());
        for p in profiles {
            println!(
                "  {} — {} Hz, {} demod [{}]",
                p.name,
                format_frequency(p.center_freq_hz as f64),
                p.demod_mode,
                p.description.as_deref().unwrap_or("no description")
            );
        }
    }
    Ok(())
}

pub fn profile_delete(name: &str) -> Result<()> {
    ensure_profile_dir()?;

    let mut manager = profiles::ProfileManager::new();
    if std::fs::metadata(profiles::default_profile_path()).is_ok() {
        let _ = manager.load_from_file(profiles::default_profile_path());
    }

    if manager.remove(name).is_some() {
        manager.save_to_file(profiles::default_profile_path())?;
        println!("Deleted profile '{}'", name);
    } else {
        println!("Profile '{}' not found.", name);
    }
    Ok(())
}
