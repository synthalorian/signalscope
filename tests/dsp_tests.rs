//! Tests for the DSP core: FFT, windowing, peak detection, Otsu threshold, SNR.
use signalscope::capture::IQSample;
use signalscope::dsp::*;

/// Build an FFTResult directly from raw bin magnitudes (dB).
fn fft_result_from_bins(bins: Vec<f32>) -> FFTResult {
    let freqs: Vec<f32> = (0..bins.len()).map(|k| k as f32).collect();
    FFTResult { bins, freqs }
}

#[test]
fn fft_of_pure_tone_peaks_at_expected_bin() {
    // fs = size = 1024 -> tone at 100 Hz lands exactly in bin 100.
    let samples = generate_test_signal(100.0, 1024.0, 1024);
    let result = compute_fft(&samples, 1024);

    assert_eq!(result.bins.len(), 1024);
    assert_eq!(result.freqs.len(), 1024);
    assert!((result.freqs[100] - 100.0).abs() < f32::EPSILON);

    let (peak_bin, &peak_val) = result
        .bins
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    assert_eq!(peak_bin, 100);
    // Magnitude 1024 -> 20*log10(1 + 1024) ≈ 60.2 dB
    assert!((peak_val - 60.2).abs() < 0.5, "peak dB was {peak_val}");
    // Far-away bins carry no energy.
    assert!(result.bins[500] < 1.0, "bin 500 was {}", result.bins[500]);
}

#[test]
fn fft_of_silence_is_zero_db() {
    let samples = vec![(0.0f32, 0.0f32); 256];
    let result = compute_fft(&samples, 256);
    assert!(result.bins.iter().all(|&b| b == 0.0));
}

#[test]
fn fft_pads_short_inputs_with_zeros() {
    let samples = generate_test_signal(10.0, 256.0, 128);
    let result = compute_fft(&samples, 256);
    assert_eq!(result.bins.len(), 256);
    let (peak_bin, _) = result
        .bins
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap();
    assert_eq!(peak_bin, 10);
}

#[test]
fn generate_test_signal_has_unit_magnitude() {
    let samples = generate_test_signal(440.0, 48_000.0, 512);
    assert_eq!(samples.len(), 512);
    for (i, q) in &samples {
        let mag = (i * i + q * q).sqrt();
        assert!((mag - 1.0).abs() < 1e-5, "magnitude {mag}");
    }
}

#[test]
fn hamming_window_shapes_endpoints() {
    let mut samples = vec![(1.0f32, 0.0f32); 101];
    apply_window(&mut samples);
    // Hamming: w(0) = 0.54 - 0.46 = 0.08, w(center) ≈ 1.0
    assert!((samples[0].0 - 0.08).abs() < 1e-6, "edge {}", samples[0].0);
    assert!(
        (samples[50].0 - 1.0).abs() < 1e-3,
        "center {}",
        samples[50].0
    );
    assert!(samples[100].0.abs() < 0.09, "far edge {}", samples[100].0);
}

#[test]
fn iq_to_tuple_preserves_values() {
    let samples = vec![IQSample { i: 0.25, q: -0.5 }, IQSample { i: 1.0, q: 1.0 }];
    let tuples = iq_to_tuple(&samples);
    assert_eq!(tuples, vec![(0.25, -0.5), (1.0, 1.0)]);
}

#[test]
fn detect_peaks_finds_known_synthetic_peaks() {
    let mut bins = vec![20.0f32; 512];
    bins[100] = 50.0; // strongest
    bins[300] = 45.0;
    let result = fft_result_from_bins(bins);

    let peaks = detect_peaks(&result, 1.0e6, 100.0e6, &PeakDetectConfig::default());
    assert_eq!(peaks.len(), 2);
    // Sorted by frequency ascending. With a 1 MHz sample rate around a 100 MHz
    // center, bin 300 maps BELOW the center frequency (negative offset) while
    // bin 100 maps above it — so bin 300 sorts first despite the higher index.
    assert_eq!(peaks[0].bin_index, 300);
    assert_eq!(peaks[1].bin_index, 100);
    // Frequency mapping: bin 100 of 512 at 1 MHz sample rate, 100 MHz center.
    let expected = 100.0e6 + (100.0 / 512.0) * 1.0e6; // = 100_195_312.5 Hz
    assert!((peaks[1].frequency_hz - expected).abs() < 1.0);
    // Prominence over the 20 dB noise floor.
    assert!((peaks[1].amplitude_db - 50.0).abs() < f32::EPSILON);
    assert!(
        (peaks[1].snr_db - 30.0).abs() < 0.01,
        "snr {}",
        peaks[1].snr_db
    );
    assert!((peaks[0].snr_db - 25.0).abs() < 0.01);
}

#[test]
fn detect_peaks_respects_min_prominence() {
    let mut bins = vec![20.0f32; 512];
    bins[100] = 50.0; // prominence 30
    bins[300] = 45.0; // prominence 25
    let result = fft_result_from_bins(bins);

    let config = PeakDetectConfig {
        min_prominence_db: 28.0,
        ..Default::default()
    };
    let peaks = detect_peaks(&result, 1.0e6, 100.0e6, &config);
    assert_eq!(peaks.len(), 1);
    assert_eq!(peaks[0].bin_index, 100);
}

#[test]
fn detect_peaks_respects_max_peaks_keeps_strongest() {
    let mut bins = vec![20.0f32; 512];
    bins[100] = 42.0;
    bins[300] = 50.0; // strongest, higher bin index
    let result = fft_result_from_bins(bins);

    let config = PeakDetectConfig {
        max_peaks: 1,
        ..Default::default()
    };
    let peaks = detect_peaks(&result, 1.0e6, 100.0e6, &config);
    assert_eq!(peaks.len(), 1);
    assert_eq!(peaks[0].bin_index, 300);
}

#[test]
fn detect_peaks_negative_frequencies_map_below_center() {
    let mut bins = vec![20.0f32; 512];
    bins[400] = 50.0; // > n/2 -> negative offset
    let result = fft_result_from_bins(bins);

    let peaks = detect_peaks(&result, 1.0e6, 100.0e6, &PeakDetectConfig::default());
    assert_eq!(peaks.len(), 1);
    let expected = 100.0e6 - ((512.0 - 400.0) / 512.0) * 1.0e6;
    assert!((peaks[0].frequency_hz - expected).abs() < 1.0);
}

#[test]
fn detect_peaks_on_real_tone_spectrum() {
    let sample_rate = 1024.0;
    let samples = generate_test_signal(100.0, sample_rate, 1024);
    let mut windowed = samples;
    apply_window(&mut windowed);
    let result = compute_fft(&windowed, 1024);

    let peaks = detect_peaks(&result, sample_rate, 0.0, &PeakDetectConfig::default());
    assert_eq!(peaks.len(), 1, "expected exactly one tone peak: {peaks:?}");
    assert!((peaks[0].frequency_hz - 100.0).abs() < 2.0);
    assert!(peaks[0].snr_db > 20.0);
}

#[test]
fn detect_peaks_returns_empty_for_tiny_input() {
    let result = fft_result_from_bins(vec![1.0; 8]);
    let peaks = detect_peaks(&result, 1.0e6, 0.0, &PeakDetectConfig::default());
    assert!(peaks.is_empty());
}

#[test]
fn otsu_threshold_separates_bimodal_spectrum() {
    // 900 noise bins at 20 dB, 100 signal bins at 50 dB.
    let mut bins = vec![20.0f32; 900];
    bins.extend(vec![50.0f32; 100]);
    let result = fft_result_from_bins(bins);

    let threshold = auto_threshold(&result);
    assert!(
        threshold > 20.0 && threshold < 50.0,
        "Otsu threshold {threshold} should split the two clusters"
    );
}

#[test]
fn otsu_threshold_edge_cases() {
    let empty = fft_result_from_bins(vec![]);
    assert_eq!(auto_threshold(&empty), 0.0);

    let flat = fft_result_from_bins(vec![7.5; 64]);
    assert_eq!(auto_threshold(&flat), 7.5);
}

#[test]
fn calculate_snr_measures_peak_above_floor() {
    let mut bins = vec![10.0f32; 101];
    bins[50] = 40.0;
    let result = fft_result_from_bins(bins);

    let snr = calculate_snr(&result, 50, 10);
    assert!((snr - 30.0).abs() < 0.01, "snr {snr}");
}

#[test]
fn calculate_snr_handles_invalid_inputs() {
    let result = fft_result_from_bins(vec![10.0; 16]);
    assert_eq!(calculate_snr(&result, 100, 10), 0.0); // bin out of range
    assert_eq!(calculate_snr(&result, 5, 0), 0.0); // no noise bins
}

#[test]
fn calculate_snr_at_band_edges_does_not_panic() {
    let mut bins = vec![5.0f32; 64];
    bins[0] = 30.0;
    bins[63] = 25.0;
    let result = fft_result_from_bins(bins);
    let snr_first = calculate_snr(&result, 0, 5);
    let snr_last = calculate_snr(&result, 63, 5);
    assert!((snr_first - 25.0).abs() < 0.01, "snr_first {snr_first}");
    assert!((snr_last - 20.0).abs() < 0.01, "snr_last {snr_last}");
}
