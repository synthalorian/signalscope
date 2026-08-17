# 📡 SignalScope

A software-defined radio (SDR) spectrum analyzer with a retro oscilloscope UI. RTL-SDR support, waterfall display, signal recording, demodulation, peak detection, frequency markers, and a plugin system.

## Features

- 📡 **RTL-SDR support** — plug in and scan (requires `librtlsdr`)
- 📺 **ASCII spectrum** — terminal-based waterfall with continuous updates
- 🎚️ **FFT analysis** — real-time frequency bins powered by rustfft
- 📻 **Signal demodulation** — AM envelope detector, FM discriminator, SSB (USB/LSB)
- 📍 **Frequency markers** — save and manage signal bookmarks
- 🔺 **Peak detection** — automatic threshold with Otsu's method
- 🔌 **Plugin system** — load custom DSP plugins via dynamic libraries (.so/.dll)
- 💾 **IQ recording** — save raw samples for later analysis
- 🔁 **Replay mode** — analyze recorded signals
- 🧪 **Test signal** — no hardware required for demo

## Install

### Basic build (test signal only)
```bash
cargo build --release
sudo cp target/release/signalscope /usr/local/bin/
```

### With RTL-SDR support
Install `librtlsdr` first:
- **Arch Linux**: `sudo pacman -S rtl-sdr`
- **Ubuntu/Debian**: `sudo apt-get install librtlsdr-dev`
- **macOS**: `brew install librtlsdr`

Then build with the `rtlsdr` feature:
```bash
cargo build --release --features rtlsdr
sudo cp target/release/signalscope /usr/local/bin/
```

## Usage

```bash
# List available SDR devices
signalscope devices

# Start spectrum analyzer with RTL-SDR (continuous waterfall)
signalscope spectrum --center-freq 100e6 --sample-rate 2.4e6 --fft-size 1024

# Record IQ samples to file
signalscope record -o capture.iq -d 10 --center-freq 100e6

# Replay recorded samples
signalscope replay -i capture.iq

# Generate test signal (no SDR required)
signalscope test-signal --freq 440

# Demodulate recorded signal
signalscope demod -m fm -i capture.iq -o audio.pcm --sample-rate 2.048e6

# Frequency marker management
signalscope markers add --frequency 100e6 --label "FM Radio"
signalscope markers list
signalscope markers export -f markers.csv

# Plugin management
signalscope plugins scan
signalscope plugins load -p /path/to/plugin.so
```

### Spectrum Analyzer

The spectrum command runs a continuous ASCII waterfall display in your terminal:
- Top of the display = strongest signals
- Updates in real-time as samples arrive
- Press **Ctrl+C** to stop

If no RTL-SDR device is found, it automatically falls back to a test signal for demonstration.

### Signal Demodulation

Demodulate recorded IQ samples to audio-frequency PCM:

```bash
# AM demodulation
signalscope demod -m am -i capture.iq -o am_audio.pcm

# FM demodulation (mono)
signalscope demod -m fm -i capture.iq -o fm_audio.pcm --carrier-offset 0

# FM stereo demodulation (broadcast FM with pilot recovery)
signalscope demod -m fm-stereo -i capture.iq -o fm_stereo.pcm --stereo

# SSB demodulation (USB)
signalscope demod -m ssb -i capture.iq -o ssb_audio.pcm --carrier-offset 1000

# Play the demodulated audio
ffplay -f f32le -ar 2048000 fm_audio.pcm

# Play stereo output
ffplay -f f32le -ac 2 -ar 2048000 fm_stereo.pcm
```

Supported modes:
- `am` — Envelope detection (amplitude demodulation)
- `fm` — Frequency discriminator (phase derivative)
- `fm-stereo` — FM broadcast stereo with 19kHz pilot tone recovery, L-R extraction, and matrix decoding
- `ssb` — Single sideband upper sideband (USB) via Hilbert transform FIR filter
- `ssb-lsb` — Single sideband lower sideband (LSB) via Hilbert transform FIR filter

### Frequency Markers

Mark and save frequencies of interest for later reference:

```bash
# Add a marker at 100 MHz with a label
signalscope markers add --frequency 100e6 --label "Local FM Station"

# Snap marker to nearest detected peak in IQ file
signalscope markers add --frequency 100e6 --label "Strongest Signal" --snap-file capture.iq

# List all saved markers
signalscope markers list

# Remove a marker near a frequency
signalscope markers remove --frequency 100e6

# Clear all markers
signalscope markers clear

# Import from CSV
signalscope markers import -f markers.csv

# Export to CSV
signalscope markers export -f markers.csv
```

Markers are automatically saved to `~/.config/signalscope/markers.json`.

### Peak Detection

Detect peaks in recorded IQ samples from the command line:

```bash
# List peaks in a recording
signalscope peak-list -i capture.iq --sample-rate 2.048e6 --center-freq 100e6

# Adjust sensitivity
signalscope peak-list -i capture.iq --min-prominence 15.0 --max-peaks 10
```

The DSP module includes automatic peak detection using:
- **Local prominence** — peaks must exceed surrounding noise floor by a configurable margin
- **Otsu's threshold** — automatic threshold calculation for signal/noise separation
- **SNR estimation** — signal-to-noise ratio for each detected peak

These are available programmatically via the `dsp::detect_peaks()` and `dsp::auto_threshold()` functions.

## DSP Implementation Notes

### FM Stereo Demodulation

The FM stereo demodulator (`demod::demod_fm_stereo`) implements the full FM broadcast stereo decoding chain:

1. **FM Discrimination** — Converts IQ to baseband multiplex signal using phase derivative
2. **Pilot Recovery** — Bandpass filter at 19kHz ±100Hz extracts the stereo pilot tone
3. **Subcarrier Generation** — Squaring the pilot creates a 38kHz component; DC is removed and the result is bandpass-filtered to isolate the clean 38kHz carrier
4. **L-R Extraction** — Bandpass 23-53kHz isolates the L-R DSBSC signal; synchronous demodulation with the 38kHz carrier recovers L-R
5. **Matrix Decoding** — L = (M+S)/2, R = (M-S)/2 where M is the 0-15kHz mono signal and S is the demodulated L-R

### SSB Demodulation (Hilbert Transform)

The SSB demodulator (`demod::demod_ssb_hilbert`) uses a 63-tap FIR Hilbert transform filter with Hamming window:

- **Impulse response**: h[n] = (2/πn) · sin²(πn/2) for odd n, windowed with Hamming
- **USB**: I − H(Q) — suppresses lower sideband
- **LSB**: I + H(Q) — suppresses upper sideband
- Post-filter: 3kHz IIR lowpass removes residual out-of-band energy

### Peak Detection

Spectrum peak detection (`dsp::detect_peaks`) uses:
- Local maxima search with minimum bin-distance separation
- Median noise-floor estimation in a sliding window around each candidate
- Prominence filtering (peak − noise_floor ≥ threshold)
- Sorting by amplitude, truncation to max_peaks, then re-sorting by frequency

### Plugin System

Load custom DSP plugins compiled as shared libraries:

```bash
# Scan default plugin directories
signalscope plugins scan

# Load a specific plugin
signalscope plugins load -p ./my_filter.so

# List built-in plugins
signalscope plugins list
```

Plugins must export a `create_dsp_plugin` symbol returning a `CDspPlugin` struct. See `src/plugins.rs` for the C FFI interface.

Built-in demodulation plugins (registerable via `DemodPluginRegistry`):
- `am` — AM envelope detection
- `fm` — FM frequency discriminator
- `fm-stereo` — FM stereo broadcast demodulator
- `ssb` — SSB-USB Hilbert transform demodulator
- `ssb-lsb` — SSB-LSB Hilbert transform demodulator

Built-in DSP plugins (available programmatically):
- `gain` — Simple gain/attenuation
- `noise_gate` — Noise gate with configurable threshold

### Record & Replay

IQ recordings are saved as raw interleaved f32 files:
- Format: `[I, Q, I, Q, ...]` where each sample is 4 bytes (little-endian f32)
- Use `replay` to load and visualize saved captures
- Use `demod` to extract audio from recorded signals

## Architecture

```
signalscope/
├── src/
│   ├── main.rs      # CLI argument parsing with clap
│   ├── cli.rs       # Command handlers (spectrum, record, replay, demod, peaks, etc.)
│   ├── capture.rs   # RTL-SDR device enumeration & IQ capture
│   ├── dsp.rs       # FFT processing, windowing, peak detection, threshold
│   ├── demod.rs     # AM/FM/FM-Stereo/SSB demodulation + DemodPlugin trait + registry
│   ├── markers.rs   # Frequency marker management (save/load/import/export/snap-to-peak)
│   ├── plugins.rs   # Dynamic DSP plugin loading via libloading + DemodPluginRegistry
│   └── display.rs   # ASCII spectrum & waterfall rendering
├── Cargo.toml
└── README.md
```

## Dependencies

- `clap` — command-line argument parsing
- `anyhow` — error handling
- `rustfft` — high-performance FFT
- `num-complex` — complex number support
- `ctrlc` — graceful shutdown handling
- `serde` / `serde_json` — marker serialization
- `libloading` — dynamic plugin loading
- `libc` — C FFI for plugin interface
- `rtlsdr` — RTL-SDR hardware bindings (optional feature)

## Roadmap

- [x] RTL-SDR integration (rtlsdr crate)
- [x] Signal demodulation (AM/FM/SSB)
- [x] Frequency marker / peak detection
- [x] Plugin system for custom DSP
- [ ] HackRF support
- [ ] Real-time wgpu renderer (retro oscilloscope UI)

## License

MIT

---

## ☕ Support the Developer

If this project saved you time, solved a problem, or just made your day a little more neon, you can fuel the next one:

[![Buy Me A Coffee](https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png)](https://buymeacoffee.com/synthalorian)
