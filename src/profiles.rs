use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// A saved receiver configuration profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverProfile {
    pub name: String,
    pub center_freq_hz: u64,
    pub sample_rate_hz: u32,
    pub gain_db: i32,
    pub demod_mode: String,
    pub description: Option<String>,
    pub created_at: String,
    pub tags: Vec<String>,
}

impl ReceiverProfile {
    pub fn new(name: impl Into<String>, center_freq_hz: u64, sample_rate_hz: u32) -> Self {
        Self {
            name: name.into(),
            center_freq_hz,
            sample_rate_hz,
            gain_db: 0,
            demod_mode: "fm".to_string(),
            description: None,
            created_at: chrono::Local::now().to_rfc3339(),
            tags: Vec::new(),
        }
    }

    pub fn with_gain(mut self, gain_db: i32) -> Self {
        self.gain_db = gain_db;
        self
    }

    pub fn with_demod(mut self, mode: impl Into<String>) -> Self {
        self.demod_mode = mode.into();
        self
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }
}

/// Profile manager that saves/loads profiles from a JSON file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileManager {
    profiles: HashMap<String, ReceiverProfile>,
}

impl ProfileManager {
    pub fn new() -> Self {
        Self {
            profiles: HashMap::new(),
        }
    }

    pub fn save_profile(&mut self, profile: ReceiverProfile) {
        self.profiles.insert(profile.name.clone(), profile);
    }

    pub fn get(&self, name: &str) -> Option<&ReceiverProfile> {
        self.profiles.get(name)
    }

    pub fn remove(&mut self, name: &str) -> Option<ReceiverProfile> {
        self.profiles.remove(name)
    }

    pub fn list(&self) -> Vec<&ReceiverProfile> {
        let mut profiles: Vec<&ReceiverProfile> = self.profiles.values().collect();
        profiles.sort_by(|a, b| a.name.cmp(&b.name));
        profiles
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.profiles)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn load_from_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let data = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read profile file: {}", path.as_ref().display()))?;
        let profiles: HashMap<String, ReceiverProfile> =
            serde_json::from_str(&data).with_context(|| {
                format!(
                    "Failed to parse profile JSON from: {}",
                    path.as_ref().display()
                )
            })?;
        self.profiles = profiles;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }
}

impl Default for ProfileManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Default profile storage path
pub fn default_profile_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.config/signalscope/profiles.json", home)
}

/// Ensure profile directory exists
pub fn ensure_profile_dir() -> Result<()> {
    let path = default_profile_path();
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_creation() {
        let p = ReceiverProfile::new("FM Radio", 100_000_000, 2_048_000)
            .with_gain(30)
            .with_demod("fm-stereo")
            .with_description("Local FM broadcast");

        assert_eq!(p.name, "FM Radio");
        assert_eq!(p.center_freq_hz, 100_000_000);
        assert_eq!(p.gain_db, 30);
        assert_eq!(p.demod_mode, "fm-stereo");
    }

    #[test]
    fn test_profile_manager() {
        let mut mgr = ProfileManager::new();
        let p = ReceiverProfile::new("Test", 100e6 as u64, 2_048_000);
        mgr.save_profile(p);
        assert_eq!(mgr.len(), 1);
        assert!(mgr.get("Test").is_some());
    }
}
