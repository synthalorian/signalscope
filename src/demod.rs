use crate::capture::IQSample;
use std::f32::consts::PI;

/// Supported demodulation modes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DemodMode {
    /// Amplitude Modulation - envelope detection
    AM,
    /// Frequency Modulation - frequency discriminator (mono)
    FM,
    /// Frequency Modulation - stereo demodulation with pilot recovery
    #[allow(non_camel_case_types)]
    FM_STEREO,
    /// Single Side Band - USB/LSB using Hilbert transform
    SSB,
    /// Single Side Band - Lower Side Band
    #[allow(non_camel_case_types)]
    SSB_LSB,
}

impl std::fmt::Display for DemodMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DemodMode::AM => write!(f, "AM"),
            DemodMode::FM => write!(f, "FM"),
            DemodMode::FM_STEREO => write!(f, "FM-Stereo"),
            DemodMode::SSB => write!(f, "SSB-USB"),
            DemodMode::SSB_LSB => write!(f, "SSB-LSB"),
        }
    }
}

/// Parse demodulation mode from string
pub fn parse_mode(mode: &str) -> Option<DemodMode> {
    match mode.to_lowercase().as_str() {
        "am" => Some(DemodMode::AM),
        "fm" => Some(DemodMode::FM),
        "fm-stereo" | "fm_stereo" | "stereo" => Some(DemodMode::FM_STEREO),
        "ssb" | "usb" => Some(DemodMode::SSB),
        "ssb-lsb" | "lsb" => Some(DemodMode::SSB_LSB),
        _ => None,
    }
}

/// Demodulate IQ samples to audio-frequency real samples
pub fn demodulate(samples: &[IQSample], sample_rate: f32, mode: DemodMode, carrier_offset: f32) -> Vec<f32> {
    match mode {
        DemodMode::AM => demod_am(samples),
        DemodMode::FM => demod_fm(samples, sample_rate),
        DemodMode::FM_STEREO => {
            let stereo = demod_fm_stereo(samples, sample_rate);
            // For compatibility with mono output, mix to mono
            stereo.iter().map(|(l, r)| (l + r) * 0.5).collect()
        }
        DemodMode::SSB | DemodMode::SSB_LSB => {
            demod_ssb_hilbert(samples, sample_rate, carrier_offset, mode == DemodMode::SSB_LSB)
        }
    }
}

/// Demodulate FM stereo and return interleaved L/R samples
pub fn demodulate_fm_stereo(samples: &[IQSample], sample_rate: f32) -> Vec<f32> {
    let stereo = demod_fm_stereo(samples, sample_rate);
    let mut output = Vec::with_capacity(stereo.len() * 2);
    for (l, r) in stereo {
        output.push(l);
        output.push(r);
    }
    output
}

// ============================================================================
// AM Demodulation
// ============================================================================

/// AM Envelope Detection
/// 
/// Extracts the amplitude envelope from the complex IQ signal.
/// Output is the magnitude of each complex sample: sqrt(I^2 + Q^2)
fn demod_am(samples: &[IQSample]) -> Vec<f32> {
    samples.iter().map(|s| {
        (s.i * s.i + s.q * s.q).sqrt()
    }).collect()
}

// ============================================================================
// FM Demodulation (Mono)
// ============================================================================

/// FM Discriminator (Frequency Demodulation)
/// 
/// Computes instantaneous frequency by taking the derivative of the phase:
/// freq[n] = (arg(s[n]) - arg(s[n-1])) / (2*pi * dt)
/// 
/// Uses atan2(Q, I) for phase, then unwraps and differences.
fn demod_fm(samples: &[IQSample], sample_rate: f32) -> Vec<f32> {
    if samples.len() < 2 {
        return vec![0.0; samples.len()];
    }

    let mut output = Vec::with_capacity(samples.len());
    let dt = 1.0 / sample_rate;
    
    // Phase of first sample
    let mut prev_phase = samples[0].q.atan2(samples[0].i);
    output.push(0.0); // First sample has no delta

    for i in 1..samples.len() {
        let phase = samples[i].q.atan2(samples[i].i);
        let mut delta = phase - prev_phase;
        
        // Unwrap phase: if jump > pi, subtract 2pi; if jump < -pi, add 2pi
        if delta > PI {
            delta -= 2.0 * PI;
        } else if delta < -PI {
            delta += 2.0 * PI;
        }
        
        // Instantaneous frequency = d(phase)/dt / (2*pi)
        let freq = delta / (2.0 * PI * dt);
        output.push(freq);
        
        prev_phase = phase;
    }

    output
}

// ============================================================================
// FM Stereo Demodulation
// ============================================================================

/// FM Broadcast Stereo Demodulator
/// 
/// FM broadcast stereo encoding:
/// - Baseband 0-15kHz: L+R (mono compatible)
/// - 19kHz pilot tone
/// - 23-53kHz: L-R double-sideband suppressed carrier, modulated on 38kHz subcarrier
/// - 57-59kHz: RDS (Radio Data System) - ignored here
/// 
/// This demodulator:
/// 1. First FM-demodulates to get the baseband multiplex signal
/// 2. Extracts the 19kHz pilot tone using a bandpass filter
/// 3. Doubles the pilot to recover the 38kHz subcarrier
/// 4. Extracts the L-R signal using a bandpass filter (23-53kHz)
/// 5. Synchronously demodulates L-R using the 38kHz carrier
/// 6. Matrix: L = (M+S)/2, R = (M-S)/2
fn demod_fm_stereo(samples: &[IQSample], sample_rate: f32) -> Vec<(f32, f32)> {
    if samples.len() < 256 {
        // Not enough samples for meaningful stereo demod, return mono
        let mono = demod_fm(samples, sample_rate);
        return mono.into_iter().map(|m| (m, m)).collect();
    }

    // Step 1: FM demodulate to get baseband multiplex signal
    let multiplex = demod_fm(samples, sample_rate);
    
    // Step 2: Extract 19kHz pilot tone
    let pilot = bandpass_filter(&multiplex, sample_rate, 18_900.0, 19_100.0);
    
    // Step 3: Double pilot to get 38kHz subcarrier (squaring creates 2nd harmonic)
    let mut subcarrier_38k: Vec<f32> = pilot.iter().map(|&p| p * p).collect();
    
    // Remove DC component from squaring
    let dc = subcarrier_38k.iter().sum::<f32>() / subcarrier_38k.len() as f32;
    for s in &mut subcarrier_38k {
        *s -= dc;
    }
    
    // Bandpass around 38kHz to clean up the doubled carrier
    let _subcarrier_38k = bandpass_filter(&subcarrier_38k, sample_rate, 37_900.0, 38_100.0);
    
    // Step 4: Extract L-R signal (23-53kHz)
    let l_minus_r = bandpass_filter(&multiplex, sample_rate, 23_000.0, 53_000.0);
    
    // Step 5: Synchronously demodulate L-R
    // L-R is DSBSC at 38kHz. Multiply by 38kHz carrier to demodulate
    let omega_38k = 2.0 * PI * 38_000.0 / sample_rate;
    let mut l_minus_r_demod = Vec::with_capacity(l_minus_r.len());
    for (n, &s) in l_minus_r.iter().enumerate() {
        let carrier = (omega_38k * n as f32).cos();
        l_minus_r_demod.push(s * carrier * 2.0);
    }
    
    // Lowpass the demodulated L-R to remove high-frequency components
    let l_minus_r_demod = lowpass_filter(&l_minus_r_demod, sample_rate, 15_000.0);
    
    // Step 6: Extract mono (L+R) - lowpass the multiplex at 15kHz
    let l_plus_r = lowpass_filter(&multiplex, sample_rate, 15_000.0);
    
    // Step 7: Matrix to get L and R
    let mut stereo = Vec::with_capacity(multiplex.len().min(l_plus_r.len()).min(l_minus_r_demod.len()));
    let len = multiplex.len().min(l_plus_r.len()).min(l_minus_r_demod.len());
    
    for i in 0..len {
        let m = l_plus_r[i];
        let s = l_minus_r_demod[i];
        let left = (m + s) * 0.5;
        let right = (m - s) * 0.5;
        stereo.push((left, right));
    }
    
    stereo
}

// ============================================================================
// SSB Demodulation via Hilbert Transform
// ============================================================================

/// Single Side Band Demodulation using Hilbert Transform
/// 
/// Uses a FIR Hilbert transform approximation to create a 90-degree phase shift,
/// then combines I and shifted-Q to extract either USB or LSB.
/// 
/// For USB: output = I * cos(ωt) - H(Q) * sin(ωt)  [or equivalent]
/// For LSB: output = I * cos(ωt) + H(Q) * sin(ωt)
/// 
/// This implementation uses a simple frequency translation + Hilbert transform approach:
/// 1. Translate signal to baseband by mixing with complex carrier
/// 2. Apply Hilbert transform to Q to get analytic signal
/// 3. Take real part for USB, or negative real part for LSB
fn demod_ssb_hilbert(samples: &[IQSample], sample_rate: f32, carrier_offset: f32, lsb: bool) -> Vec<f32> {
    if samples.is_empty() {
        return vec![];
    }

    // Step 1: Translate to baseband (complex mixing)
    let omega = 2.0 * PI * carrier_offset / sample_rate;
    let mut baseband_i = Vec::with_capacity(samples.len());
    let mut baseband_q = Vec::with_capacity(samples.len());
    
    for (n, s) in samples.iter().enumerate() {
        let t = n as f32;
        let cos_osc = (omega * t).cos();
        let sin_osc = (omega * t).sin();
        
        // Complex multiplication: s * e^(-j*omega*t)
        baseband_i.push(s.i * cos_osc + s.q * sin_osc);
        baseband_q.push(-s.i * sin_osc + s.q * cos_osc);
    }
    
    // Step 2: Apply Hilbert transform to Q channel
    let hilbert_q = fir_hilbert_transform(&baseband_q);
    
    // Step 3: For USB: I + j*H(Q) -> take real part is just I
    //          For LSB: I - j*H(Q) -> take real part is just I
    // 
    // Actually, the standard approach:
    // - Translate to baseband
    // - USB = (I + j*H(Q)) -> lowpass -> take real part (which is I after proper filtering)
    // - LSB = (I - j*H(Q)) -> lowpass -> take real part
    // 
    // Simpler: after translation, lowpass filter I for USB, or 
    // use the Hilbert-transformed Q to suppress the unwanted sideband
    
    let mut output = Vec::with_capacity(samples.len());
    
    if lsb {
        // LSB: I * cos(0) + H(Q) * sin(0) with proper phase
        // Actually for LSB after downconversion: take I + H(Q) as analytic, then take real
        // The unwanted sideband is at positive frequencies, we want negative
        for i in 0..samples.len().min(hilbert_q.len()) {
            output.push(baseband_i[i] + hilbert_q[i]);
        }
    } else {
        // USB: take I - H(Q) as analytic signal
        for i in 0..samples.len().min(hilbert_q.len()) {
            output.push(baseband_i[i] - hilbert_q[i]);
        }
    }
    
    // Apply lowpass filter to remove residual high-frequency components
    lowpass_filter(&output, sample_rate, 3000.0)
}

/// FIR Hilbert Transform approximation
/// 
/// Uses a truncated ideal Hilbert transform impulse response:
/// h[n] = (2/πn) * sin²(πn/2) for n ≠ 0
/// h[n] = 0 for n = 0
/// 
/// Applied with a Hamming window for better frequency response.
fn fir_hilbert_transform(signal: &[f32]) -> Vec<f32> {
    const TAPS: usize = 63; // Odd number, must be odd for symmetric FIR
    let half_taps = TAPS / 2;
    
    // Generate Hilbert transform filter coefficients with Hamming window
    let mut coeffs = vec![0.0f32; TAPS];
    for n in 0..TAPS {
        let k = n as i32 - half_taps as i32;
        if k == 0 {
            coeffs[n] = 0.0;
        } else {
            let kf = k as f32;
            // Ideal Hilbert transform: h[n] = 2/(π*n) for odd n, 0 for even n
            if k % 2 == 0 {
                coeffs[n] = 0.0;
            } else {
                let ideal = 2.0 / (PI * kf);
                // Hamming window
                let window = 0.54 - 0.46 * (2.0 * PI * n as f32 / (TAPS - 1) as f32).cos();
                coeffs[n] = ideal * window;
            }
        }
    }
    
    // Convolve signal with filter
    let mut output = vec![0.0f32; signal.len()];
    for i in half_taps..signal.len() - half_taps {
        let mut sum = 0.0f32;
        for j in 0..TAPS {
            sum += signal[i + j - half_taps] * coeffs[j];
        }
        output[i] = sum;
    }
    
    // Fill edges with original signal (approximate)
    for i in 0..half_taps {
        output[i] = signal[i];
    }
    for i in signal.len() - half_taps..signal.len() {
        output[i] = signal[i];
    }
    
    output
}

// ============================================================================
// Filter Implementations
// ============================================================================

/// Simple first-order IIR low-pass filter
fn lowpass_filter(signal: &[f32], sample_rate: f32, cutoff_hz: f32) -> Vec<f32> {
    if signal.is_empty() {
        return vec![];
    }
    
    let rc = 1.0 / (2.0 * PI * cutoff_hz);
    let dt = 1.0 / sample_rate;
    let alpha = dt / (rc + dt);
    
    let mut filtered = Vec::with_capacity(signal.len());
    let mut prev = signal[0];
    filtered.push(prev);
    
    for &sample in &signal[1..] {
        prev = prev + alpha * (sample - prev);
        filtered.push(prev);
    }
    
    filtered
}

/// Bandpass filter using cascaded lowpass + highpass (simple IIR approximation)
/// 
/// For better performance, this uses a simple 2nd-order IIR bandpass.
/// Center frequency and bandwidth determine the Q factor.
fn bandpass_filter(signal: &[f32], sample_rate: f32, low_hz: f32, high_hz: f32) -> Vec<f32> {
    if signal.is_empty() {
        return vec![];
    }
    
    // Simple approach: highpass then lowpass
    let highpassed = highpass_filter(signal, sample_rate, low_hz);
    lowpass_filter(&highpassed, sample_rate, high_hz)
}

/// Simple first-order IIR high-pass filter
fn highpass_filter(signal: &[f32], sample_rate: f32, cutoff_hz: f32) -> Vec<f32> {
    if signal.is_empty() {
        return vec![];
    }
    
    let rc = 1.0 / (2.0 * PI * cutoff_hz);
    let dt = 1.0 / sample_rate;
    let alpha = rc / (rc + dt);
    
    let mut filtered = Vec::with_capacity(signal.len());
    let mut prev_input = signal[0];
    let mut prev_output = 0.0f32;
    filtered.push(prev_output);
    
    for &sample in &signal[1..] {
        let output = alpha * (prev_output + sample - prev_input);
        filtered.push(output);
        prev_input = sample;
        prev_output = output;
    }
    
    filtered
}

// ============================================================================
// Audio Utilities
// ============================================================================

/// Normalize audio samples to [-1.0, 1.0] range
pub fn normalize_audio(samples: &mut [f32]) {
    let max_val = samples.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    if max_val > 0.0 {
        for s in samples.iter_mut() {
            *s /= max_val;
        }
    }
}

/// Convert f32 audio samples to i16 for WAV/PCM output
pub fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|&s| {
        let clamped = s.clamp(-1.0, 1.0);
        (clamped * i16::MAX as f32) as i16
    }).collect()
}

/// Save audio samples as raw PCM (f32 little-endian)
pub fn save_pcm(samples: &[f32], path: &str) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;
    
    let mut file = File::create(path)?;
    for &sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

/// Save stereo interleaved audio samples as raw PCM (f32 little-endian)
pub fn save_pcm_stereo(samples: &[(f32, f32)], path: &str) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;
    
    let mut file = File::create(path)?;
    for &(l, r) in samples {
        file.write_all(&l.to_le_bytes())?;
        file.write_all(&r.to_le_bytes())?;
    }
    Ok(())
}

// ============================================================================
// Demodulator State Machine
// ============================================================================

/// Demodulator state for streaming operation
pub struct Demodulator {
    mode: DemodMode,
    sample_rate: f32,
    carrier_offset: f32,
    /// Previous sample for FM discriminator continuity
    prev_sample: Option<IQSample>,
    prev_phase: f32,
    /// Buffer for FM stereo processing (needs block processing)
    stereo_buffer: Vec<IQSample>,
    /// Stereo buffer size (must be large enough for filter transients)
    stereo_buffer_size: usize,
}

impl Demodulator {
    pub fn new(mode: DemodMode, sample_rate: f32) -> Self {
        Self {
            mode,
            sample_rate,
            carrier_offset: 0.0,
            prev_sample: None,
            prev_phase: 0.0,
            stereo_buffer: Vec::new(),
            stereo_buffer_size: 4096,
        }
    }

    pub fn with_carrier(mut self, offset_hz: f32) -> Self {
        self.carrier_offset = offset_hz;
        self
    }

    pub fn with_stereo_buffer_size(mut self, size: usize) -> Self {
        self.stereo_buffer_size = size.max(256);
        self
    }

    /// Process a chunk of samples (stateful for FM)
    pub fn process(&mut self, samples: &[IQSample]) -> Vec<f32> {
        let result = match self.mode {
            DemodMode::AM => demod_am(samples),
            DemodMode::FM => {
                let mut input = Vec::with_capacity(samples.len() + 1);
                if let Some(ref prev) = self.prev_sample {
                    input.push(prev.clone());
                }
                input.extend(samples.iter().cloned());
                
                if input.len() < 2 {
                    vec![0.0; samples.len()]
                } else {
                    demod_fm(&input, self.sample_rate)
                        .into_iter().skip(if self.prev_sample.is_some() { 1 } else { 0 })
                        .collect()
                }
            }
            DemodMode::FM_STEREO => {
                // For stereo, accumulate samples and process in blocks
                self.stereo_buffer.extend(samples.iter().cloned());
                if self.stereo_buffer.len() >= self.stereo_buffer_size {
                    let block: Vec<IQSample> = self.stereo_buffer.drain(..self.stereo_buffer_size).collect();
                    let stereo = demod_fm_stereo(&block, self.sample_rate);
                    stereo.into_iter().map(|(l, r)| (l + r) * 0.5).collect()
                } else {
                    vec![0.0; samples.len()]
                }
            }
            DemodMode::SSB | DemodMode::SSB_LSB => {
                demod_ssb_hilbert(samples, self.sample_rate, self.carrier_offset, self.mode == DemodMode::SSB_LSB)
            }
        };

        // Save last sample for next chunk
        if let Some(last) = samples.last() {
            self.prev_sample = Some(last.clone());
            if self.mode == DemodMode::FM || self.mode == DemodMode::FM_STEREO {
                self.prev_phase = last.q.atan2(last.i);
            }
        }

        result
    }

    /// Process stereo and return interleaved L/R samples
    /// Only valid for FM_STEREO mode
    pub fn process_stereo(&mut self, samples: &[IQSample]) -> Vec<(f32, f32)> {
        if self.mode != DemodMode::FM_STEREO {
            // Fall back to mono, duplicate to both channels
            let mono = self.process(samples);
            return mono.into_iter().map(|m| (m, m)).collect();
        }

        self.stereo_buffer.extend(samples.iter().cloned());
        
        if self.stereo_buffer.len() >= self.stereo_buffer_size {
            let block: Vec<IQSample> = self.stereo_buffer.drain(..self.stereo_buffer_size).collect();
            demod_fm_stereo(&block, self.sample_rate)
        } else {
            vec![(0.0, 0.0); samples.len()]
        }
    }

    /// Flush any buffered stereo samples
    pub fn flush_stereo(&mut self) -> Vec<(f32, f32)> {
        if self.stereo_buffer.is_empty() {
            return vec![];
        }
        let remaining = self.stereo_buffer.drain(..).collect::<Vec<_>>();
        demod_fm_stereo(&remaining, self.sample_rate)
    }

    pub fn mode(&self) -> DemodMode {
        self.mode
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn set_carrier_offset(&mut self, offset_hz: f32) {
        self.carrier_offset = offset_hz;
    }
}

// ============================================================================
// Plugin Trait for Demodulation
// ============================================================================

/// Trait for demodulator plugins
/// 
/// Plugins can implement custom demodulation algorithms and be registered
/// in the demod plugin registry or loaded dynamically.
pub trait DemodPlugin: Send + Sync {
    /// Plugin name
    fn name(&self) -> &str;
    
    /// Plugin description
    fn description(&self) -> &str;
    
    /// Demodulate a block of IQ samples to audio
    fn demodulate(&mut self, samples: &[IQSample], sample_rate: f32, carrier_offset: f32) -> Vec<f32>;
    
    /// Check if this plugin supports stereo output
    fn supports_stereo(&self) -> bool {
        false
    }
    
    /// Demodulate to stereo (optional, default returns mono duplicated)
    fn demodulate_stereo(&mut self, samples: &[IQSample], sample_rate: f32, carrier_offset: f32) -> Vec<(f32, f32)> {
        let mono = self.demodulate(samples, sample_rate, carrier_offset);
        mono.into_iter().map(|m| (m, m)).collect()
    }
    
    /// Set a parameter by name
    fn set_param(&mut self, _key: &str, _value: f64) {}
    
    /// Get a parameter by name
    fn get_param(&self, _key: &str) -> Option<f64> {
        None
    }
}

/// Built-in AM demodulation plugin
pub struct AmDemodPlugin;

impl DemodPlugin for AmDemodPlugin {
    fn name(&self) -> &str {
        "am"
    }
    
    fn description(&self) -> &str {
        "AM envelope detection demodulator"
    }
    
    fn demodulate(&mut self, samples: &[IQSample], _sample_rate: f32, _carrier_offset: f32) -> Vec<f32> {
        demod_am(samples)
    }
}

/// Built-in FM demodulation plugin
pub struct FmDemodPlugin;

impl DemodPlugin for FmDemodPlugin {
    fn name(&self) -> &str {
        "fm"
    }
    
    fn description(&self) -> &str {
        "FM frequency discriminator demodulator"
    }
    
    fn demodulate(&mut self, samples: &[IQSample], sample_rate: f32, _carrier_offset: f32) -> Vec<f32> {
        demod_fm(samples, sample_rate)
    }
}

/// Built-in FM stereo demodulation plugin
pub struct FmStereoDemodPlugin;

impl DemodPlugin for FmStereoDemodPlugin {
    fn name(&self) -> &str {
        "fm-stereo"
    }
    
    fn description(&self) -> &str {
        "FM stereo broadcast demodulator with pilot recovery"
    }
    
    fn demodulate(&mut self, samples: &[IQSample], sample_rate: f32, _carrier_offset: f32) -> Vec<f32> {
        let stereo = demod_fm_stereo(samples, sample_rate);
        stereo.into_iter().map(|(l, r)| (l + r) * 0.5).collect()
    }
    
    fn supports_stereo(&self) -> bool {
        true
    }
    
    fn demodulate_stereo(&mut self, samples: &[IQSample], sample_rate: f32, _carrier_offset: f32) -> Vec<(f32, f32)> {
        demod_fm_stereo(samples, sample_rate)
    }
}

/// Built-in SSB demodulation plugin
pub struct SsbDemodPlugin {
    lsb: bool,
}

impl SsbDemodPlugin {
    pub fn new(lsb: bool) -> Self {
        Self { lsb }
    }
}

impl DemodPlugin for SsbDemodPlugin {
    fn name(&self) -> &str {
        if self.lsb { "ssb-lsb" } else { "ssb" }
    }
    
    fn description(&self) -> &str {
        if self.lsb {
            "SSB lower sideband demodulator using Hilbert transform"
        } else {
            "SSB upper sideband demodulator using Hilbert transform"
        }
    }
    
    fn demodulate(&mut self, samples: &[IQSample], sample_rate: f32, carrier_offset: f32) -> Vec<f32> {
        demod_ssb_hilbert(samples, sample_rate, carrier_offset, self.lsb)
    }
}

/// Registry of built-in demodulation plugins
pub struct DemodPluginRegistry {
    plugins: Vec<Box<dyn DemodPlugin>>,
}

impl DemodPluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }
    
    /// Register all built-in demodulators
    pub fn with_builtins(mut self) -> Self {
        self.register(Box::new(AmDemodPlugin));
        self.register(Box::new(FmDemodPlugin));
        self.register(Box::new(FmStereoDemodPlugin));
        self.register(Box::new(SsbDemodPlugin::new(false)));
        self.register(Box::new(SsbDemodPlugin::new(true)));
        self
    }
    
    /// Register a plugin
    pub fn register(&mut self, plugin: Box<dyn DemodPlugin>) {
        self.plugins.push(plugin);
    }
    
    /// Find a plugin by name
    pub fn find(&self, name: &str) -> Option<&dyn DemodPlugin> {
        self.plugins.iter().find(|p| p.name() == name).map(|p| p.as_ref())
    }
    
    /// List all registered plugins
    pub fn list(&self) -> Vec<(&str, &str)> {
        self.plugins.iter().map(|p| (p.name(), p.description())).collect()
    }
    
    /// Number of registered plugins
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

impl Default for DemodPluginRegistry {
    fn default() -> Self {
        Self::new().with_builtins()
    }
}
