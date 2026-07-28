use rustfft::{num_complex::Complex, FftPlanner};
use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct FFTResult {
    pub bins: Vec<f32>,
    pub freqs: Vec<f32>,
}

/// Compute FFT using rustfft for performance.
pub fn compute_fft(samples: &[(f32, f32)], size: usize) -> FFTResult {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(size);

    // Convert samples to Complex<f32>
    let mut buffer: Vec<Complex<f32>> = samples
        .iter()
        .take(size)
        .map(|(i, q)| Complex::new(*i, *q))
        .collect();

    // Pad with zeros if we don't have enough samples
    while buffer.len() < size {
        buffer.push(Complex::new(0.0, 0.0));
    }

    // Perform FFT in-place
    fft.process(&mut buffer);

    // Convert to magnitudes in dB
    let bins: Vec<f32> = buffer
        .iter()
        .map(|c| {
            let mag = (c.re * c.re + c.im * c.im).sqrt();
            20.0 * (1.0 + mag).log10() // dB scale
        })
        .collect();

    // Generate frequency bins
    let freqs: Vec<f32> = (0..size).map(|k| k as f32).collect();

    FFTResult { bins, freqs }
}

pub fn generate_test_signal(freq: f64, sample_rate: f64, count: usize) -> Vec<(f32, f32)> {
    let mut samples = Vec::with_capacity(count);
    for n in 0..count {
        let t = n as f64 / sample_rate;
        let phase = 2.0 * std::f64::consts::PI * freq * t;
        samples.push((phase.cos() as f32, phase.sin() as f32));
    }
    samples
}

pub fn apply_window(samples: &mut [(f32, f32)]) {
    let n = samples.len() as f32;
    for (i, (re, im)) in samples.iter_mut().enumerate() {
        let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / (n - 1.0)).cos();
        *re *= w;
        *im *= w;
    }
}

/// Convert IQ samples from capture module to (f32, f32) tuples for DSP.
pub fn iq_to_tuple(samples: &[crate::capture::IQSample]) -> Vec<(f32, f32)> {
    samples.iter().map(|s| (s.i, s.q)).collect()
}

/// A detected peak in the spectrum
#[derive(Debug, Clone)]
pub struct Peak {
    /// Index in the FFT bins
    pub bin_index: usize,
    /// Frequency in Hz (relative to center frequency)
    pub frequency_hz: f64,
    /// Amplitude in dB
    pub amplitude_db: f32,
    /// Signal-to-noise ratio estimate
    pub snr_db: f32,
}

/// Peak detection configuration
#[derive(Debug, Clone, Copy)]
pub struct PeakDetectConfig {
    /// Minimum peak height above surrounding noise floor (dB)
    pub min_prominence_db: f32,
    /// Minimum distance between peaks (in bins)
    pub min_distance_bins: usize,
    /// Noise floor estimation window size (bins on each side)
    pub noise_window_bins: usize,
    /// Maximum number of peaks to return
    pub max_peaks: usize,
}

impl Default for PeakDetectConfig {
    fn default() -> Self {
        Self {
            min_prominence_db: 10.0,
            min_distance_bins: 5,
            noise_window_bins: 10,
            max_peaks: 20,
        }
    }
}

/// Detect peaks in FFT result using prominence and noise floor estimation
///
/// Uses a local noise floor estimation (median of surrounding bins) and
/// requires peaks to have sufficient prominence above that noise floor.
pub fn detect_peaks(
    result: &FFTResult,
    sample_rate: f64,
    center_freq: f64,
    config: &PeakDetectConfig,
) -> Vec<Peak> {
    let bins = &result.bins;
    let n = bins.len();
    if n < config.noise_window_bins * 2 + 1 {
        return vec![];
    }

    let mut peaks = Vec::new();

    for i in config.noise_window_bins..(n - config.noise_window_bins) {
        let current = bins[i];
        let is_local_max = (i.saturating_sub(config.min_distance_bins)..i)
            .all(|j| bins[j] <= current)
            && ((i + 1)..=(i + config.min_distance_bins).min(n - 1)).all(|j| bins[j] <= current);

        if !is_local_max {
            continue;
        }

        let left_start = i.saturating_sub(config.noise_window_bins);
        let left_end = i;
        let right_start = i + 1;
        let right_end = (i + 1 + config.noise_window_bins).min(n);

        let mut noise_samples: Vec<f32> = bins[left_start..left_end]
            .iter()
            .chain(&bins[right_start..right_end])
            .copied()
            .collect();

        noise_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let noise_floor = if noise_samples.len().is_multiple_of(2) {
            let mid = noise_samples.len() / 2;
            (noise_samples[mid - 1] + noise_samples[mid]) / 2.0
        } else {
            noise_samples[noise_samples.len() / 2]
        };

        let prominence = current - noise_floor;

        if prominence >= config.min_prominence_db {
            let freq_offset = if i < n / 2 {
                (i as f64 / n as f64) * sample_rate
            } else {
                -((n - i) as f64 / n as f64) * sample_rate
            };

            let frequency_hz = center_freq + freq_offset;

            peaks.push(Peak {
                bin_index: i,
                frequency_hz,
                amplitude_db: current,
                snr_db: prominence,
            });
        }
    }

    peaks.sort_by(|a, b| {
        b.amplitude_db
            .partial_cmp(&a.amplitude_db)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    peaks.truncate(config.max_peaks);
    peaks.sort_by(|a, b| {
        a.frequency_hz
            .partial_cmp(&b.frequency_hz)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    peaks
}

/// Automatic threshold detection using Otsu's method on the spectrum
///
/// Returns a threshold value that separates signal from noise
pub fn auto_threshold(result: &FFTResult) -> f32 {
    let bins = &result.bins;
    if bins.is_empty() {
        return 0.0;
    }

    let min_val = bins.iter().copied().fold(f32::INFINITY, f32::min);
    let max_val = bins.iter().copied().fold(0.0f32, f32::max);

    if max_val <= min_val {
        return min_val;
    }

    const HIST_BINS: usize = 256;
    let mut histogram = vec![0u32; HIST_BINS];
    let range = max_val - min_val;

    for &val in bins {
        let idx = (((val - min_val) / range) * (HIST_BINS - 1) as f32) as usize;
        let idx = idx.min(HIST_BINS - 1);
        histogram[idx] += 1;
    }

    let total = bins.len() as f64;
    let mut best_threshold = 0;
    let mut max_variance = 0.0f64;

    for t in 1..HIST_BINS {
        let w0: f64 = histogram[..t].iter().map(|&c| c as f64).sum::<f64>() / total;
        let w1: f64 = histogram[t..].iter().map(|&c| c as f64).sum::<f64>() / total;

        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }

        let mu0: f64 = histogram[..t]
            .iter()
            .enumerate()
            .map(|(i, &c)| i as f64 * c as f64)
            .sum::<f64>()
            / (w0 * total);
        let mu1: f64 = histogram[t..]
            .iter()
            .enumerate()
            .map(|(i, &c)| (t + i) as f64 * c as f64)
            .sum::<f64>()
            / (w1 * total);

        let variance = w0 * w1 * (mu0 - mu1).powi(2);

        if variance > max_variance {
            max_variance = variance;
            best_threshold = t;
        }
    }

    min_val + (best_threshold as f32 / (HIST_BINS - 1) as f32) * range
}

/// Calculate signal-to-noise ratio for a specific frequency region
pub fn calculate_snr(result: &FFTResult, signal_bin: usize, noise_bins: usize) -> f32 {
    let bins = &result.bins;
    let n = bins.len();

    if signal_bin >= n || noise_bins == 0 {
        return 0.0;
    }

    let signal_power = bins[signal_bin];
    let left_start = signal_bin.saturating_sub(noise_bins);
    let left_end = signal_bin.saturating_sub(1);
    let right_start = (signal_bin + 1).min(n);
    let right_end = (signal_bin + 1 + noise_bins).min(n);

    let mut noise_sum = 0.0f32;
    let mut noise_count = 0usize;

    if signal_bin > 0 {
        for &val in bins.iter().take(left_end + 1).skip(left_start) {
            noise_sum += val;
            noise_count += 1;
        }
    }

    for &val in bins.iter().take(right_end).skip(right_start) {
        noise_sum += val;
        noise_count += 1;
    }

    if noise_count == 0 {
        return 0.0;
    }

    let noise_floor = noise_sum / noise_count as f32;
    if noise_floor <= 0.0 {
        return 0.0;
    }

    signal_power - noise_floor
}
