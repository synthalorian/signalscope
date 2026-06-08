use crate::capture::IQSample;
use crate::dsp::{FFTResult, compute_fft, apply_window};
use std::f32::consts::PI;

/// Classification result for a signal
#[derive(Debug, Clone, PartialEq)]
pub enum SignalClass {
    /// Amplitude Modulation - carrier with symmetric sidebands
    AM,
    /// Frequency Modulation - wide flat-top spectrum
    FM,
    /// Single Side Band - asymmetric spectrum
    SSB,
    /// Digital signal - rectangular/spectral flatness pattern
    Digital,
    /// Unknown / noise
    Unknown,
    /// Multiple overlapping signals
    Multi,
}

impl std::fmt::Display for SignalClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignalClass::AM => write!(f, "AM"),
            SignalClass::FM => write!(f, "FM"),
            SignalClass::SSB => write!(f, "SSB"),
            SignalClass::Digital => write!(f, "Digital"),
            SignalClass::Unknown => write!(f, "Unknown"),
            SignalClass::Multi => write!(f, "Multi"),
        }
    }
}

/// Confidence score for classification
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub class: SignalClass,
    pub confidence: f32,
    pub bandwidth_hz: f32,
    pub carrier_offset_hz: f32,
    pub notes: Vec<String>,
}

/// Simple signal classifier based on spectral shape analysis
pub struct SignalClassifier;

impl SignalClassifier {
    pub fn new() -> Self {
        Self
    }

    /// Classify a signal from its IQ samples
    /// 
    /// Uses spectral analysis to determine the modulation type:
    /// - AM: Strong carrier peak with symmetric sidebands
    /// - FM: Wide, relatively flat spectrum with Bessel-like shape
    /// - SSB: Asymmetric spectrum, only one sideband present
    /// - Digital: Sharp edges, spectral flatness, or multiple tones
    pub fn classify(&self, samples: &[IQSample], sample_rate: f32) -> ClassificationResult {
        if samples.len() < 256 {
            return ClassificationResult {
                class: SignalClass::Unknown,
                confidence: 0.0,
                bandwidth_hz: 0.0,
                carrier_offset_hz: 0.0,
                notes: vec!["Too few samples for classification".to_string()],
            };
        }

        // Convert to tuples and compute FFT
        let tuples: Vec<(f32, f32)> = samples.iter().map(|s| (s.i, s.q)).collect();
        let fft_size = tuples.len().next_power_of_two().min(4096);
        let mut windowed: Vec<(f32, f32)> = tuples.iter().take(fft_size).copied().collect();
        
        // Pad if needed
        while windowed.len() < fft_size {
            windowed.push((0.0, 0.0));
        }
        
        apply_window(&mut windowed);
        let fft_result = compute_fft(&windowed, fft_size);
        let bins = &fft_result.bins;

        // Analyze spectrum characteristics
        let bandwidth = self.estimate_bandwidth(bins, sample_rate);
        let symmetry = self.measure_symmetry(bins);
        let flatness = self.spectral_flatness(bins);
        let carrier_score = self.detect_carrier_peak(bins);
        let peakiness = self.measure_peakiness(bins);

        let mut notes = Vec::new();
        notes.push(format!("Bandwidth: {:.0} Hz", bandwidth));
        notes.push(format!("Symmetry: {:.3}", symmetry));
        notes.push(format!("Flatness: {:.3}", flatness));
        notes.push(format!("Carrier score: {:.3}", carrier_score));

        // Decision logic
        let (class, confidence) = if bandwidth > 150_000.0 {
            // Wide signal - likely FM broadcast
            if flatness > 0.3 && peakiness < 0.5 {
                (SignalClass::FM, 0.85)
            } else {
                (SignalClass::FM, 0.60)
            }
        } else if bandwidth > 10_000.0 && bandwidth <= 150_000.0 {
            // Medium bandwidth
            if carrier_score > 0.6 && symmetry > 0.7 {
                // Strong carrier with symmetric sidebands = AM
                (SignalClass::AM, carrier_score)
            } else if symmetry < 0.4 {
                // Asymmetric = SSB
                (SignalClass::SSB, 1.0 - symmetry)
            } else if flatness > 0.4 {
                (SignalClass::Digital, flatness)
            } else {
                (SignalClass::FM, 0.50)
            }
        } else if bandwidth > 1_000.0 && bandwidth <= 10_000.0 {
            // Narrow bandwidth
            if carrier_score > 0.5 && symmetry > 0.6 {
                (SignalClass::AM, carrier_score * 0.8)
            } else if symmetry < 0.4 {
                (SignalClass::SSB, (1.0 - symmetry) * 0.8)
            } else if flatness > 0.3 {
                (SignalClass::Digital, flatness * 0.8)
            } else {
                (SignalClass::Unknown, 0.3)
            }
        } else {
            (SignalClass::Unknown, 0.2)
        };

        ClassificationResult {
            class,
            confidence: confidence.min(0.99),
            bandwidth_hz: bandwidth,
            carrier_offset_hz: 0.0,
            notes,
        }
    }

    /// Estimate -3dB bandwidth from spectrum
    fn estimate_bandwidth(&self, bins: &[f32], sample_rate: f32) -> f32 {
        let max_val = bins.iter().copied().fold(0.0f32, f32::max);
        if max_val <= 0.0 {
            return 0.0;
        }

        let threshold = max_val - 3.0; // -3dB point
        let bin_hz = sample_rate / bins.len() as f32;

        // Find contiguous region above threshold
        let mut in_band = false;
        let mut start_bin = 0usize;
        let mut end_bin = 0usize;
        let mut max_width = 0usize;

        for (i, &val) in bins.iter().enumerate() {
            if val >= threshold {
                if !in_band {
                    in_band = true;
                    start_bin = i;
                }
                end_bin = i;
            } else {
                if in_band {
                    let width = end_bin - start_bin;
                    if width > max_width {
                        max_width = width;
                    }
                    in_band = false;
                }
            }
        }

        if in_band {
            let width = end_bin - start_bin;
            if width > max_width {
                max_width = width;
            }
        }

        max_width as f32 * bin_hz
    }

    /// Measure spectral symmetry around center bin
    /// Returns 1.0 for perfectly symmetric, 0.0 for completely asymmetric
    fn measure_symmetry(&self, bins: &[f32]) -> f32 {
        let n = bins.len();
        if n < 4 {
            return 0.5;
        }

        let center = n / 2;
        let check_range = (n / 4).min(512);

        let mut diff_sum = 0.0f32;
        let mut sum = 0.0f32;

        for i in 1..check_range {
            let left = center.saturating_sub(i);
            let right = (center + i).min(n - 1);
            let diff = (bins[left] - bins[right]).abs();
            diff_sum += diff;
            sum += bins[left] + bins[right];
        }

        if sum <= 0.0 {
            return 0.5;
        }

        // Convert to similarity: 1.0 = identical, 0.0 = completely different
        let similarity = 1.0 - (diff_sum / sum).min(1.0);
        similarity
    }

    /// Spectral flatness measure (geometric mean / arithmetic mean)
    /// Higher = more flat = likely digital/noise
    fn spectral_flatness(&self, bins: &[f32]) -> f32 {
        let n = bins.len();
        if n == 0 {
            return 0.0;
        }

        // Use linear magnitudes (bins are already in dB, convert back)
        let linear: Vec<f32> = bins.iter().map(|&db| {
            let db_clamped = db.max(0.0);
            10.0f32.powf(db_clamped / 20.0) - 1.0 // Undo the 1.0 + mag scaling
        }).map(|v| v.max(1e-10)).collect();

        let sum: f32 = linear.iter().sum();
        if sum <= 0.0 {
            return 0.0;
        }

        let arithmetic_mean = sum / n as f32;

        // Geometric mean via log-sum
        let log_sum: f32 = linear.iter().map(|&v| v.ln()).sum();
        let geometric_mean = (log_sum / n as f32).exp();

        (geometric_mean / arithmetic_mean).min(1.0).max(0.0)
    }

    /// Detect if there's a strong carrier peak in the center
    fn detect_carrier_peak(&self, bins: &[f32]) -> f32 {
        let n = bins.len();
        if n < 8 {
            return 0.0;
        }

        let center = n / 2;
        let carrier_bins = 3usize;
        let carrier_power: f32 = bins[center.saturating_sub(carrier_bins)..=(center + carrier_bins).min(n - 1)]
            .iter().sum();

        let sideband_bins = 10usize;
        let left_start = center.saturating_sub(carrier_bins + sideband_bins);
        let left_end = center.saturating_sub(carrier_bins);
        let right_start = (center + carrier_bins).min(n - 1);
        let right_end = (center + carrier_bins + sideband_bins).min(n - 1);

        let sideband_power: f32 = if left_end > left_start && right_end > right_start {
            bins[left_start..left_end].iter().sum::<f32>() + bins[right_start..right_end].iter().sum::<f32>()
        } else {
            carrier_power
        };

        if sideband_power <= 0.0 {
            return 0.5;
        }

        let ratio = carrier_power / sideband_power;
        ratio.min(2.0) / 2.0 // Normalize to 0-1
    }

    /// Measure how "peaky" the spectrum is
    fn measure_peakiness(&self, bins: &[f32]) -> f32 {
        let n = bins.len();
        if n == 0 {
            return 0.0;
        }

        let mean = bins.iter().sum::<f32>() / n as f32;
        if mean <= 0.0 {
            return 0.0;
        }

        let max = bins.iter().copied().fold(0.0f32, f32::max);
        (max / mean - 1.0).min(5.0) / 5.0
    }
}

impl Default for SignalClassifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Batch classify multiple frequency regions
pub fn classify_regions(samples: &[IQSample], sample_rate: f32, regions: &[(f32, f32)]) -> Vec<(f32, ClassificationResult)> {
    let classifier = SignalClassifier::new();
    let mut results = Vec::new();

    for &(start_hz, end_hz) in regions {
        // Extract region samples (simple approach: use full samples but note the region)
        // For a proper implementation, we'd shift the region to baseband first
        let result = classifier.classify(samples, sample_rate);
        results.push((start_hz, result));
    }

    results
}
