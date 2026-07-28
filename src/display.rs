use crate::dsp::FFTResult;
use crate::dsp::{detect_peaks, Peak, PeakDetectConfig};
use crate::markers::FrequencyMarker;

/// Render spectrum as static ASCII graph
pub fn render_spectrum(result: &FFTResult, width: usize, height: usize) {
    let max_val = result.bins.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let step = result.bins.len() / width.max(1);

    println!("┌{}┐", "─".repeat(width));
    for row in 0..height {
        let threshold = max_val * (1.0 - row as f32 / height as f32);
        let mut line = String::with_capacity(width);
        for col in 0..width {
            let idx = (col * step).min(result.bins.len() - 1);
            let val = result.bins[idx];
            let ch = if val > threshold {
                "█"
            } else if val > threshold * 0.7 {
                "▓"
            } else if val > threshold * 0.4 {
                "▒"
            } else {
                " "
            };
            line.push_str(ch);
        }
        println!("│{}│", line);
    }
    println!("└{}┘", "─".repeat(width));
}

/// Render spectrum with peaks highlighted
pub fn render_spectrum_with_peaks(
    result: &FFTResult,
    width: usize,
    height: usize,
    peaks: &[Peak],
    _sample_rate: f64,
    _center_freq: f64,
) {
    let max_val = result.bins.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let step = result.bins.len() / width.max(1);

    println!("┌{}┐", "─".repeat(width));
    for row in 0..height {
        let threshold = max_val * (1.0 - row as f32 / height as f32);
        let mut line = String::with_capacity(width);
        for col in 0..width {
            let idx = (col * step).min(result.bins.len() - 1);
            let val = result.bins[idx];

            let is_peak = peaks.iter().any(|p| {
                let peak_col = (p.bin_index / step).min(width - 1);
                peak_col == col
            });

            let ch = if is_peak && row == height - 1 {
                "^"
            } else if val > threshold {
                "█"
            } else if val > threshold * 0.7 {
                "▓"
            } else if val > threshold * 0.4 {
                "▒"
            } else {
                " "
            };
            line.push_str(ch);
        }
        println!("│{}│", line);
    }
    println!("└{}┘", "─".repeat(width));

    if !peaks.is_empty() {
        println!("Detected peaks:");
        for peak in peaks.iter().take(5) {
            println!(
                "  {} — {:.1} dB (SNR: {:.1} dB)",
                format_frequency(peak.frequency_hz),
                peak.amplitude_db,
                peak.snr_db
            );
        }
    }
}

/// Render a single waterfall row as a string of ASCII characters
pub fn render_waterfall_row(result: &FFTResult, width: usize) -> String {
    let max_val = result.bins.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let step = result.bins.len() / width.max(1);
    let mut line = String::with_capacity(width);
    for col in 0..width {
        let idx = (col * step).min(result.bins.len() - 1);
        let val = result.bins[idx] / max_val;
        let ch = if val > 0.8 {
            "█"
        } else if val > 0.6 {
            "▓"
        } else if val > 0.4 {
            "▒"
        } else if val > 0.2 {
            "░"
        } else {
            " "
        };
        line.push_str(ch);
    }
    line
}

/// Render a waterfall row with markers overlaid
pub fn render_waterfall_row_with_markers(
    result: &FFTResult,
    width: usize,
    markers: &[&FrequencyMarker],
    center_freq: f64,
    sample_rate: f64,
    fft_size: usize,
) -> String {
    let max_val = result.bins.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let step = result.bins.len() / width.max(1);
    let mut line = String::with_capacity(width);

    for col in 0..width {
        let idx = (col * step).min(result.bins.len() - 1);
        let val = result.bins[idx] / max_val;

        // Check if a marker is at this column
        let marker_here = markers.iter().any(|m| {
            let freq_offset = m.frequency - center_freq;
            let bin_idx = if freq_offset >= 0.0 {
                ((freq_offset / sample_rate) * fft_size as f64) as usize
            } else {
                fft_size - ((-freq_offset / sample_rate) * fft_size as f64) as usize
            };
            let marker_col = (bin_idx / step).min(width - 1);
            marker_col == col
        });

        let ch = if marker_here {
            "▼"
        } else if val > 0.8 {
            "█"
        } else if val > 0.6 {
            "▓"
        } else if val > 0.4 {
            "▒"
        } else if val > 0.2 {
            "░"
        } else {
            " "
        };
        line.push_str(ch);
    }
    line
}

/// A terminal waterfall display that updates continuously
pub struct WaterfallDisplay {
    width: usize,
    height: usize,
    rows: Vec<String>,
    markers: Vec<FrequencyMarker>,
    center_freq: f64,
    sample_rate: f64,
    fft_size: usize,
}

impl WaterfallDisplay {
    pub fn new(width: usize, height: usize) -> Self {
        WaterfallDisplay {
            width,
            height,
            rows: Vec::with_capacity(height),
            markers: Vec::new(),
            center_freq: 100e6,
            sample_rate: 2.4e6,
            fft_size: 1024,
        }
    }

    pub fn with_markers(mut self, markers: Vec<FrequencyMarker>) -> Self {
        self.markers = markers;
        self
    }

    pub fn set_freq_params(&mut self, center_freq: f64, sample_rate: f64, fft_size: usize) {
        self.center_freq = center_freq;
        self.sample_rate = sample_rate;
        self.fft_size = fft_size;
    }

    /// Push a new row and redraw the entire waterfall
    pub fn push_row(&mut self, result: &FFTResult) {
        let row = if self.markers.is_empty() {
            render_waterfall_row(result, self.width)
        } else {
            let marker_refs: Vec<&FrequencyMarker> = self.markers.iter().collect();
            render_waterfall_row_with_markers(
                result,
                self.width,
                &marker_refs,
                self.center_freq,
                self.sample_rate,
                self.fft_size,
            )
        };
        self.rows.push(row);
        if self.rows.len() > self.height {
            self.rows.remove(0);
        }
    }

    /// Render the waterfall as a static block (for non-interactive use)
    pub fn render(&self) {
        if self.rows.is_empty() {
            return;
        }
        println!("┌{}┐", "─".repeat(self.width));
        for row in &self.rows {
            println!("│{}│", row);
        }
        println!("└{}┘", "─".repeat(self.width));
    }

    /// Clear screen and render waterfall at current cursor position
    pub fn render_continuous(&self) {
        print!("\x1B[2J\x1B[H");
        if self.rows.is_empty() {
            println!("Waiting for samples...");
            return;
        }
        println!("┌{}┐", "─".repeat(self.width));
        for row in &self.rows {
            println!("│{}│", row);
        }
        println!("└{}┘", "─".repeat(self.width));
        println!("SignalScope Spectrum Analyzer — Press Ctrl+C to stop");

        if !self.markers.is_empty() {
            print!("Markers: ");
            for (i, m) in self.markers.iter().take(3).enumerate() {
                if i > 0 {
                    print!(", ");
                }
                print!("{}", m.format_frequency());
            }
            if self.markers.len() > 3 {
                print!(", +{} more", self.markers.len() - 3);
            }
            println!();
        }
    }
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

/// Render spectrum with automatic peak detection and display
pub fn render_spectrum_auto_peaks(
    result: &FFTResult,
    width: usize,
    height: usize,
    sample_rate: f64,
    center_freq: f64,
) {
    let config = PeakDetectConfig::default();
    let peaks = detect_peaks(result, sample_rate, center_freq, &config);
    render_spectrum_with_peaks(result, width, height, &peaks, sample_rate, center_freq);
}
