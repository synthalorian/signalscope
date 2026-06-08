use clap::{Parser, Subcommand};

mod capture;
mod display;
mod dsp;
mod cli;
mod demod;
mod markers;
mod plugins;

#[derive(Parser)]
#[command(name = "signalscope")]
#[command(about = "SDR spectrum analyzer with retro oscilloscope UI")]
struct Args {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available SDR devices
    Devices,
    /// Start spectrum analyzer (TUI mode)
    Spectrum {
        #[arg(short, long, default_value = "100e6")]
        center_freq: f64,
        #[arg(short, long, default_value = "2.4e6")]
        sample_rate: f64,
        #[arg(short, long, default_value = "1024")]
        fft_size: usize,
    },
    /// Record IQ samples to file
    Record {
        #[arg(short, long)]
        output: String,
        #[arg(short, long, default_value = "10")]
        duration: u64,
        #[arg(short, long, default_value = "100e6")]
        center_freq: f64,
    },
    /// Replay recorded samples
    Replay {
        #[arg(short, long)]
        input: String,
    },
    /// Generate test signal (no SDR required)
    TestSignal {
        #[arg(short, long, default_value = "440.0")]
        freq: f64,
    },
    /// Demodulate signal (AM, FM, FM-Stereo, SSB)
    Demod {
        /// Demodulation mode: am, fm, fm-stereo, ssb, ssb-lsb
        #[arg(short, long, default_value = "fm")]
        mode: String,
        /// Input file (IQ samples)
        #[arg(short, long)]
        input: String,
        /// Output file (demodulated audio)
        #[arg(short, long, default_value = "output.pcm")]
        output: String,
        /// Sample rate in Hz
        #[arg(short, long, default_value = "2.048e6")]
        sample_rate: f64,
        /// Carrier frequency offset in Hz
        #[arg(short, long, default_value = "0.0")]
        carrier_offset: f64,
        /// Output stereo interleaved L/R (for fm-stereo)
        #[arg(long)]
        stereo: bool,
    },
    /// Detect peaks in recorded IQ samples
    PeakList {
        /// Input file (IQ samples)
        #[arg(short, long)]
        input: String,
        /// Sample rate in Hz
        #[arg(short, long, default_value = "2.048e6")]
        sample_rate: f64,
        /// Center frequency in Hz
        #[arg(short, long, default_value = "100e6")]
        center_freq: f64,
        /// FFT size
        #[arg(short, long, default_value = "2048")]
        fft_size: usize,
        /// Minimum peak prominence in dB
        #[arg(long, default_value = "10.0")]
        min_prominence: f32,
        /// Maximum number of peaks
        #[arg(long, default_value = "20")]
        max_peaks: usize,
    },
    /// Frequency marker management
    Markers {
        #[command(subcommand)]
        cmd: MarkerCommands,
    },
    /// Plugin management
    Plugins {
        #[command(subcommand)]
        cmd: PluginCommands,
    },
}

#[derive(Subcommand)]
enum MarkerCommands {
    /// Add a marker at a frequency
    Add {
        #[arg(short, long)]
        frequency: f64,
        #[arg(short, long)]
        label: Option<String>,
        /// Snap marker to nearest detected peak in IQ file
        #[arg(short, long)]
        snap_file: Option<String>,
        /// Sample rate for snap calculation
        #[arg(long, default_value = "2.048e6")]
        snap_sample_rate: f64,
    },
    /// List all markers
    List {
        #[arg(short, long)]
        file: Option<String>,
    },
        Remove {
            #[arg(short, long)]
            frequency: f64,
        },
    /// Clear all markers
    Clear,
    /// Import markers from CSV
    Import {
        #[arg(short, long)]
        file: String,
    },
    /// Export markers to CSV
    Export {
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
enum PluginCommands {
    /// List loaded plugins
    List,
    /// Load a plugin from file
    Load {
        #[arg(short, long)]
        path: String,
    },
    /// Scan plugin directories
    Scan,
    /// Set plugin parameter
    SetParam {
        #[arg(short, long)]
        index: usize,
        #[arg(short, long)]
        key: String,
        #[arg(short, long)]
        value: f64,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.cmd {
        Commands::Devices => cli::list_devices(),
        Commands::Spectrum { center_freq, sample_rate, fft_size } => {
            cli::spectrum(center_freq, sample_rate, fft_size)
        }
        Commands::Record { output, duration, center_freq } => {
            cli::record(&output, duration, center_freq)
        }
        Commands::Replay { input } => cli::replay(&input),
        Commands::TestSignal { freq } => cli::test_signal(freq),
        Commands::Demod { mode, input, output, sample_rate, carrier_offset, stereo } => {
            cli::demodulate(&mode, &input, &output, sample_rate, carrier_offset, stereo)
        }
        Commands::PeakList { input, sample_rate, center_freq, fft_size, min_prominence, max_peaks } => {
            cli::peak_list(&input, sample_rate, center_freq, fft_size, min_prominence, max_peaks)
        }
        Commands::Markers { cmd } => match cmd {
            MarkerCommands::Add { frequency, label, snap_file, snap_sample_rate } => {
                cli::marker_add(frequency, label, snap_file, snap_sample_rate)
            }
            MarkerCommands::List { file } => cli::marker_list(file),
            MarkerCommands::Remove { frequency } => cli::marker_remove(frequency),
            MarkerCommands::Clear => cli::marker_clear(),
            MarkerCommands::Import { file } => cli::marker_import(&file),
            MarkerCommands::Export { file } => cli::marker_export(&file),
        },
        Commands::Plugins { cmd } => match cmd {
            PluginCommands::List => cli::plugin_list(),
            PluginCommands::Load { path } => cli::plugin_load(&path),
            PluginCommands::Scan => cli::plugin_scan(),
            PluginCommands::SetParam { index, key, value } => cli::plugin_set_param(index, &key, value),
        },
    }
}
