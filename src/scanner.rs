use crate::capture::{CaptureConfig, IQSample};
use crate::dsp::{apply_window, compute_fft, iq_to_tuple};
use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Scanner configuration
#[derive(Debug, Clone)]
pub struct ScannerConfig {
    /// Start frequency in Hz
    pub start_freq_hz: u64,
    /// End frequency in Hz
    pub end_freq_hz: u64,
    /// Step size in Hz
    pub step_hz: u64,
    /// Squelch threshold in dB (relative to noise floor)
    pub squelch_db: f32,
    /// Dwell time per frequency in milliseconds
    pub dwell_ms: u64,
    /// Sample rate for scanning
    pub sample_rate_hz: u32,
    /// Number of samples to capture per frequency
    pub sample_count: usize,
    /// Stop on first signal or scan all
    pub stop_on_signal: bool,
    /// Frequency list (overrides start/end/step if provided)
    pub frequency_list: Vec<u64>,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            start_freq_hz: 88_000_000,
            end_freq_hz: 108_000_000,
            step_hz: 100_000,
            squelch_db: 15.0,
            dwell_ms: 100,
            sample_rate_hz: 2_048_000,
            sample_count: 4096,
            stop_on_signal: true,
            frequency_list: Vec::new(),
        }
    }
}

/// Detected signal during scanning
#[derive(Debug, Clone)]
pub struct ScannedSignal {
    pub frequency_hz: u64,
    pub power_db: f32,
    pub noise_floor_db: f32,
    pub snr_db: f32,
    pub bandwidth_hz: f32,
}

/// Frequency scanner that sweeps across frequencies and detects active signals
pub struct FrequencyScanner {
    config: ScannerConfig,
}

impl FrequencyScanner {
    pub fn new(config: ScannerConfig) -> Self {
        Self { config }
    }

    /// Generate the list of frequencies to scan
    fn get_frequencies(&self) -> Vec<u64> {
        if !self.config.frequency_list.is_empty() {
            self.config.frequency_list.clone()
        } else {
            let mut freqs = Vec::new();
            let mut f = self.config.start_freq_hz;
            while f <= self.config.end_freq_hz {
                freqs.push(f);
                f += self.config.step_hz;
            }
            freqs
        }
    }

    /// Scan frequencies, calling the capture function for each
    pub fn scan<F>(&self, mut capture_fn: F, running: Arc<AtomicBool>) -> Result<Vec<ScannedSignal>>
    where
        F: FnMut(&CaptureConfig, usize) -> Result<Vec<IQSample>>,
    {
        let frequencies = self.get_frequencies();
        println!(
            "Frequency scanner: scanning {} frequencies",
            frequencies.len()
        );
        println!(
            "Range: {:.2} MHz - {:.2} MHz | Step: {} kHz | Squelch: {:.1} dB",
            self.config.start_freq_hz as f64 / 1e6,
            self.config.end_freq_hz as f64 / 1e6,
            self.config.step_hz / 1000,
            self.config.squelch_db
        );
        println!("Press Ctrl+C to stop\n");

        let mut detected_signals = Vec::new();
        let mut noise_floor_estimate = 0.0f32;
        let mut noise_samples = 0usize;

        for (i, &freq_hz) in frequencies.iter().enumerate() {
            if !running.load(Ordering::SeqCst) {
                println!("Scanner stopped by user.");
                break;
            }

            let capture_config = CaptureConfig {
                center_freq: freq_hz,
                sample_rate: self.config.sample_rate_hz,
                gain: 0,
                device_index: 0,
            };

            print!(
                "\r[{}/{}] {:.3} MHz...",
                i + 1,
                frequencies.len(),
                freq_hz as f64 / 1e6
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());

            match capture_fn(&capture_config, self.config.sample_count) {
                Ok(samples) => {
                    if samples.is_empty() {
                        continue;
                    }

                    // Compute FFT and measure power
                    let power = self.measure_channel_power(&samples);

                    // Update noise floor estimate (use lowest 20% of readings)
                    if noise_samples < 10 {
                        noise_floor_estimate = power;
                        noise_samples += 1;
                    } else {
                        noise_floor_estimate = noise_floor_estimate * 0.95 + power * 0.05;
                    }

                    let snr = power - noise_floor_estimate;

                    if snr >= self.config.squelch_db {
                        // Estimate bandwidth
                        let bandwidth =
                            self.estimate_bandwidth(&samples, self.config.sample_rate_hz as f32);

                        let signal = ScannedSignal {
                            frequency_hz: freq_hz,
                            power_db: power,
                            noise_floor_db: noise_floor_estimate,
                            snr_db: snr,
                            bandwidth_hz: bandwidth,
                        };

                        println!("\n  ★ Signal detected at {:.3} MHz | Power: {:.1} dB | SNR: {:.1} dB | BW: {:.0} kHz",
                            freq_hz as f64 / 1e6, power, snr, bandwidth / 1000.0);

                        detected_signals.push(signal);

                        if self.config.stop_on_signal {
                            println!("\nStopping on first signal (use --scan-all to continue).");
                            break;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("\n  Error at {:.3} MHz: {}", freq_hz as f64 / 1e6, e);
                }
            }

            thread::sleep(Duration::from_millis(self.config.dwell_ms));
        }

        println!();
        if detected_signals.is_empty() {
            println!(
                "No signals detected above {:.1} dB squelch threshold.",
                self.config.squelch_db
            );
        } else {
            println!("Detected {} signal(s):", detected_signals.len());
            println!(
                "{:<6} {:<16} {:<12} {:<12} {:<12}",
                "#", "Frequency", "Power", "SNR", "Bandwidth"
            );
            println!("{}", "-".repeat(60));
            for (i, sig) in detected_signals.iter().enumerate() {
                println!(
                    "{:<6} {:<16} {:<12.1} {:<12.1} {:<12.0}",
                    i + 1,
                    format_frequency(sig.frequency_hz),
                    sig.power_db,
                    sig.snr_db,
                    sig.bandwidth_hz
                );
            }
        }

        Ok(detected_signals)
    }

    /// Measure total channel power from IQ samples
    fn measure_channel_power(&self, samples: &[IQSample]) -> f32 {
        if samples.is_empty() {
            return -999.0;
        }

        let power_sum: f32 = samples.iter().map(|s| s.i * s.i + s.q * s.q).sum();

        let avg_power = power_sum / samples.len() as f32;
        if avg_power <= 0.0 {
            return -999.0;
        }

        // Convert to dB
        10.0 * avg_power.log10()
    }

    /// Estimate signal bandwidth from FFT
    fn estimate_bandwidth(&self, samples: &[IQSample], sample_rate: f32) -> f32 {
        let tuples = iq_to_tuple(samples);
        let fft_size = tuples.len().next_power_of_two().min(2048);
        let mut windowed: Vec<(f32, f32)> = tuples.iter().take(fft_size).copied().collect();

        while windowed.len() < fft_size {
            windowed.push((0.0, 0.0));
        }

        apply_window(&mut windowed);
        let result = compute_fft(&windowed, fft_size);

        let max_val = result.bins.iter().copied().fold(0.0f32, f32::max);
        if max_val <= 0.0 {
            return 0.0;
        }

        let threshold = max_val - 6.0; // -6dB bandwidth
        let bin_hz = sample_rate / fft_size as f32;

        let mut in_band = false;
        let mut start_bin = 0usize;
        let mut end_bin = 0usize;
        let mut max_width = 0usize;

        for (i, &val) in result.bins.iter().enumerate() {
            if val >= threshold {
                if !in_band {
                    in_band = true;
                    start_bin = i;
                }
                end_bin = i;
            } else {
                if in_band {
                    let width = end_bin.saturating_sub(start_bin);
                    if width > max_width {
                        max_width = width;
                    }
                    in_band = false;
                }
            }
        }

        if in_band {
            let width = end_bin.saturating_sub(start_bin);
            if width > max_width {
                max_width = width;
            }
        }

        max_width as f32 * bin_hz
    }
}

fn format_frequency(freq_hz: u64) -> String {
    let f = freq_hz as f64;
    if f >= 1e9 {
        format!("{:.3} GHz", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.3} MHz", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.3} kHz", f / 1e3)
    } else {
        format!("{:.0} Hz", f)
    }
}

/// Parse a frequency list from a comma-separated string
pub fn parse_frequency_list(s: &str) -> Result<Vec<u64>> {
    let mut freqs = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Try parsing as number (handles scientific notation like 100e6)
        let freq = if part.contains('e') || part.contains('E') {
            part.parse::<f64>()
                .map_err(|_| anyhow!("Invalid frequency: {}", part))? as u64
        } else {
            part.parse::<u64>()
                .map_err(|_| anyhow!("Invalid frequency: {}", part))?
        };

        freqs.push(freq);
    }

    if freqs.is_empty() {
        return Err(anyhow!("No valid frequencies in list"));
    }

    Ok(freqs)
}
