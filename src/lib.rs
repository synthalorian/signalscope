//! SignalScope — SDR spectrum analyzer with retro oscilloscope UI.
//!
//! This library exposes the DSP, demodulation, classification, marker,
//! profile, plugin, scheduling, scanning, streaming, and RDS modules used
//! by the `signalscope` binary.

pub mod capture;
pub mod classifier;
pub mod cli;
pub mod demod;
pub mod display;
pub mod dsp;
pub mod markers;
pub mod plugins;
pub mod profiles;
pub mod rds;
pub mod scanner;
pub mod scheduler;
pub mod streaming;
