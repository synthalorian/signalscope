use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use anyhow::{Result, Context};

/// A frequency marker representing a detected or user-placed signal peak
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrequencyMarker {
    /// Frequency in Hz
    pub frequency: f64,
    /// Signal strength in dB (optional)
    pub amplitude_db: Option<f32>,
    /// User-defined label
    pub label: Option<String>,
    /// Timestamp when marker was created (seconds since epoch)
    pub timestamp: u64,
    /// Marker color/style hint for display
    pub color: Option<String>,
}

impl FrequencyMarker {
    pub fn new(frequency: f64) -> Self {
        Self {
            frequency,
            amplitude_db: None,
            label: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            color: None,
        }
    }

    pub fn with_amplitude(mut self, db: f32) -> Self {
        self.amplitude_db = Some(db);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Format frequency as human-readable string (Hz, kHz, MHz, GHz)
    pub fn format_frequency(&self) -> String {
        let f = self.frequency.abs();
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
}

/// A collection of frequency markers with save/load capability
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarkerBank {
    markers: BTreeMap<u64, FrequencyMarker>, // keyed by frequency as u64 (Hz * 1000 for sub-Hz)
    next_id: u64,
}

impl MarkerBank {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a marker, returning its ID
    pub fn add(&mut self, marker: FrequencyMarker) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.markers.insert(id, marker);
        id
    }

    /// Remove a marker by ID
    pub fn remove(&mut self, id: u64) -> Option<FrequencyMarker> {
        self.markers.remove(&id)
    }

    /// Get a marker by ID
    pub fn get(&self, id: u64) -> Option<&FrequencyMarker> {
        self.markers.get(&id)
    }

    /// Get all markers
    pub fn all(&self) -> Vec<&FrequencyMarker> {
        self.markers.values().collect()
    }

    /// Find marker closest to given frequency within tolerance (Hz)
    pub fn find_near(&self, frequency: f64, tolerance_hz: f64) -> Option<(u64, &FrequencyMarker)> {
        self.markers
            .iter()
            .map(|(&id, m)| (id, m, (m.frequency - frequency).abs()))
            .filter(|(_, _, dist)| *dist <= tolerance_hz)
            .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(id, m, _)| (id, m))
    }

    /// Clear all markers
    pub fn clear(&mut self) {
        self.markers.clear();
    }

    pub fn len(&self) -> usize {
        self.markers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Save marker bank to JSON file
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.markers)?;
        fs::write(path, json)?;
        Ok(())
    }

    /// Load marker bank from JSON file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let data = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read marker file: {}", path.as_ref().display()))?;
        let markers: BTreeMap<u64, FrequencyMarker> = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse marker JSON from: {}", path.as_ref().display()))?;
        
        let next_id = markers.keys().max().copied().unwrap_or(0) + 1;
        Ok(MarkerBank { markers, next_id })
    }

    /// Export markers as CSV
    pub fn export_csv<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)?;
        writeln!(file, "frequency_hz,amplitude_db,label,timestamp")?;
        for marker in self.markers.values() {
            writeln!(
                file,
                "{},{:.2},{},{}",
                marker.frequency,
                marker.amplitude_db.unwrap_or(0.0),
                marker.label.as_deref().unwrap_or(""),
                marker.timestamp
            )?;
        }
        Ok(())
    }

    /// Import markers from CSV
    pub fn import_csv<P: AsRef<Path>>(&mut self, path: P) -> Result<usize> {
        let data = fs::read_to_string(path)?;
        let mut count = 0;
        for (i, line) in data.lines().enumerate() {
            if i == 0 || line.trim().is_empty() {
                continue; // Skip header
            }
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 1 {
                if let Ok(freq) = parts[0].parse::<f64>() {
                    let mut marker = FrequencyMarker::new(freq);
                    if parts.len() > 1 {
                        if let Ok(amp) = parts[1].parse::<f32>() {
                            marker.amplitude_db = Some(amp);
                        }
                    }
                    if parts.len() > 2 && !parts[2].is_empty() {
                        marker.label = Some(parts[2].to_string());
                    }
                    self.add(marker);
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

/// Marker manager that wraps a MarkerBank with auto-save capability
pub struct MarkerManager {
    bank: MarkerBank,
    auto_save_path: Option<String>,
}

impl MarkerManager {
    pub fn new() -> Self {
        Self {
            bank: MarkerBank::new(),
            auto_save_path: None,
        }
    }

    pub fn with_auto_save(mut self, path: impl Into<String>) -> Self {
        self.auto_save_path = Some(path.into());
        self
    }

    pub fn add(&mut self, marker: FrequencyMarker) -> u64 {
        let id = self.bank.add(marker);
        self.try_auto_save();
        id
    }

    pub fn remove(&mut self, id: u64) -> Option<FrequencyMarker> {
        let result = self.bank.remove(id);
        if result.is_some() {
            self.try_auto_save();
        }
        result
    }

    pub fn clear(&mut self) {
        self.bank.clear();
        self.try_auto_save();
    }

    pub fn get(&self, id: u64) -> Option<&FrequencyMarker> {
        self.bank.get(id)
    }

    pub fn all(&self) -> Vec<&FrequencyMarker> {
        self.bank.all()
    }

    pub fn find_near(&self, frequency: f64, tolerance_hz: f64) -> Option<(u64, &FrequencyMarker)> {
        self.bank.find_near(frequency, tolerance_hz)
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.bank.save(path)
    }

    pub fn load<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.bank = MarkerBank::load(path)?;
        Ok(())
    }

    pub fn export_csv<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.bank.export_csv(path)
    }

    pub fn import_csv<P: AsRef<Path>>(&mut self, path: P) -> Result<usize> {
        let count = self.bank.import_csv(path)?;
        self.try_auto_save();
        Ok(count)
    }

    pub fn bank(&self) -> &MarkerBank {
        &self.bank
    }

    fn try_auto_save(&self) {
        if let Some(ref path) = self.auto_save_path {
            let _ = self.bank.save(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marker_creation() {
        let m = FrequencyMarker::new(100e6)
            .with_amplitude(-30.5)
            .with_label("FM Radio");
        
        assert_eq!(m.frequency, 100e6);
        assert_eq!(m.amplitude_db, Some(-30.5));
        assert_eq!(m.label, Some("FM Radio".to_string()));
    }

    #[test]
    fn test_marker_bank() {
        let mut bank = MarkerBank::new();
        let id = bank.add(FrequencyMarker::new(100e6));
        assert_eq!(bank.len(), 1);
        assert!(bank.get(id).is_some());
        
        bank.remove(id);
        assert!(bank.is_empty());
    }

    #[test]
    fn test_find_near() {
        let mut bank = MarkerBank::new();
        bank.add(FrequencyMarker::new(100e6));
        bank.add(FrequencyMarker::new(200e6));
        
        let found = bank.find_near(100.1e6, 1e6);
        assert!(found.is_some());
        assert!((found.unwrap().1.frequency - 100e6).abs() < 1.0);
    }

    #[test]
    fn test_format_frequency() {
        let m = FrequencyMarker::new(100e6);
        assert_eq!(m.format_frequency(), "100.000 MHz");
        
        let m = FrequencyMarker::new(2.4e9);
        assert_eq!(m.format_frequency(), "2.400 GHz");
    }
}
