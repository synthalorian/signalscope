use crate::capture::IQSample;
use crate::demod::{demod_fm, DemodMode};
use std::f32::consts::PI;

/// RDS group types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RdsGroupType {
    Type0A, // Basic tuning and switching information
    Type0B,
    Type1A, // Programme Item Number
    Type1B,
    Type2A, // RadioText (64 chars)
    Type2B, // RadioText (32 chars)
    Type3A, // Application Identification for ODA
    Type3B,
    Type4A, // Clock-time and date
    Type4B,
    Type5A, // Transparent data channels
    Type5B,
    Type6A, // In-house applications
    Type6B,
    Type7A, // Radio Paging
    Type7B,
    Type8A, // Traffic Message Channel
    Type8B,
    Type9A, // Emergency warning systems
    Type9B,
    Type10A, // Programme Type Name
    Type10B,
    Type11A, // Reserved
    Type11B,
    Type12A, // Reserved
    Type12B,
    Type13A, // Enhanced Radio Paging
    Type13B,
    Type14A, // Enhanced Other Networks information
    Type14B,
    Type15A, // Reserved
    Type15B, // Fast basic tuning and switching
    Unknown(u8, u8),
}

impl RdsGroupType {
    pub fn from_code(group_type: u8, version_b: bool) -> Self {
        match group_type {
            0 => if version_b { RdsGroupType::Type0B } else { RdsGroupType::Type0A },
            1 => if version_b { RdsGroupType::Type1B } else { RdsGroupType::Type1A },
            2 => if version_b { RdsGroupType::Type2B } else { RdsGroupType::Type2A },
            3 => if version_b { RdsGroupType::Type3B } else { RdsGroupType::Type3A },
            4 => if version_b { RdsGroupType::Type4B } else { RdsGroupType::Type4A },
            5 => if version_b { RdsGroupType::Type5B } else { RdsGroupType::Type5A },
            6 => if version_b { RdsGroupType::Type6B } else { RdsGroupType::Type6A },
            7 => if version_b { RdsGroupType::Type7B } else { RdsGroupType::Type7A },
            8 => if version_b { RdsGroupType::Type8B } else { RdsGroupType::Type8A },
            9 => if version_b { RdsGroupType::Type9B } else { RdsGroupType::Type9A },
            10 => if version_b { RdsGroupType::Type10B } else { RdsGroupType::Type10A },
            11 => if version_b { RdsGroupType::Type11B } else { RdsGroupType::Type11A },
            12 => if version_b { RdsGroupType::Type12B } else { RdsGroupType::Type12A },
            13 => if version_b { RdsGroupType::Type13B } else { RdsGroupType::Type13A },
            14 => if version_b { RdsGroupType::Type14B } else { RdsGroupType::Type14A },
            15 => if version_b { RdsGroupType::Type15B } else { RdsGroupType::Type15A },
            _ => RdsGroupType::Unknown(group_type, if version_b { 1 } else { 0 }),
        }
    }
}

/// Decoded RDS information
#[derive(Debug, Clone, Default)]
pub struct RdsInfo {
    pub pi_code: u16,
    pub programme_service: String,
    pub radio_text: String,
    pub programme_type: u8,
    pub programme_type_name: Option<String>,
    pub traffic_announcement: bool,
    pub music_speech: bool,
    pub stereo: bool,
    pub artificial_head: bool,
    pub compressed: bool,
    pub dynamic_pty: bool,
    pub clock_time: Option<String>,
    pub alt_frequencies: Vec<f32>,
    pub group_stats: std::collections::HashMap<String, u32>,
    pub error_count: u32,
    pub good_block_count: u32,
}

impl RdsInfo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pty_description(&self) -> &'static str {
        pty_to_string(self.programme_type)
    }
}

/// RDS decoder state machine
pub struct RdsDecoder {
    sample_rate: f32,
    rds_buffer: Vec<f32>,
    prev_sample: f32,
    phase_accumulator: f32,
    symbol_sync: u32,
    shift_reg: u32,
    message: [u16; 4],
    block_counter: u8,
    
    // Decoded state
    pub info: RdsInfo,
    ps_chars: [u8; 8],
    rt_chars: [u8; 64],
    rt_ab_flag: bool,
    rt_segment: u8,
}

impl RdsDecoder {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            rds_buffer: Vec::with_capacity(4096),
            prev_sample: 0.0,
            phase_accumulator: 0.0,
            symbol_sync: 0,
            shift_reg: 0,
            message: [0; 4],
            block_counter: 0,
            info: RdsInfo::new(),
            ps_chars: [0; 8],
            rt_chars: [0; 64],
            rt_ab_flag: false,
            rt_segment: 0,
        }
    }

    /// Process IQ samples and extract RDS data
    /// 
    /// The RDS signal is at 57kHz in the FM multiplex, BPSK modulated at 1187.5 baud.
    /// 
    /// Steps:
    /// 1. FM demodulate to get baseband multiplex
    /// 2. Bandpass filter around 57kHz to extract RDS subcarrier
    /// 3. Multiply by 57kHz carrier to downconvert to baseband
    /// 4. Low-pass filter and sample at symbol rate
    /// 5. Decode differentially encoded BPSK
    /// 6. Frame sync and block decoding
    pub fn process_samples(&mut self, samples: &[IQSample]) {
        if samples.len() < 256 {
            return;
        }

        // Step 1: FM demodulate
        let multiplex = demod_fm(samples, self.sample_rate);
        
        // Step 2: Extract RDS subcarrier at 57kHz ±2.4kHz
        let rds_subcarrier = self.bandpass_filter(&multiplex, 54_600.0, 59_400.0);
        
        // Step 3: Downconvert to baseband by mixing with 57kHz
        let omega_57k = 2.0 * PI * 57_000.0 / self.sample_rate;
        let mut baseband = Vec::with_capacity(rds_subcarrier.len());
        for (n, &s) in rds_subcarrier.iter().enumerate() {
            let carrier = (omega_57k * n as f32).cos();
            baseband.push(s * carrier * 2.0);
        }
        
        // Step 4: Lowpass at ~3kHz to isolate RDS baseband
        let baseband = self.lowpass_filter(&baseband, 3000.0);
        
        // Step 5: Sample at symbol rate (1187.5 Hz)
        let samples_per_symbol = (self.sample_rate / 1187.5) as usize;
        if samples_per_symbol == 0 {
            return;
        }

        // Differential decode and process symbols
        let mut prev_symbol = 0.0f32;
        for chunk in baseband.chunks(samples_per_symbol) {
            if chunk.is_empty() {
                continue;
            }
            
            // Average over symbol period
            let symbol = chunk.iter().sum::<f32>() / chunk.len() as f32;
            
            // Differential decode: bit = sign(symbol * prev_symbol)
            let diff = symbol * prev_symbol;
            let bit = if diff >= 0.0 { 1 } else { 0 };
            
            self.process_bit(bit);
            prev_symbol = symbol;
        }
    }

    fn process_bit(&mut self, bit: u8) {
        // Shift register for frame sync
        self.shift_reg = ((self.shift_reg << 1) | (bit as u32)) & 0x3FFFFFFF;
        
        // Check for block A sync word (0x0FC) or its complement
        // RDS uses offset words for synchronization
        let sync_patterns = [
            (0x0FC, 0),      // Block A
            (0x198, 1),      // Block B
            (0x168, 2),      // Block C
            (0x1B4, 3),      // Block C'
            (0x350, 2),      // Block D (same as C position)
        ];
        
        for &(pattern, block_type) in &sync_patterns {
            let mask = 0x3FF; // 10 bits
            if (self.shift_reg & mask) == pattern {
                self.block_counter = block_type;
                self.message = [0; 4];
                return;
            }
        }
        
        // Accumulate bits into message
        if self.block_counter < 4 {
            self.message[self.block_counter as usize] = (self.message[self.block_counter as usize] << 1) | (bit as u16);
            
            // Check if we have a complete group (4 blocks of 26 bits each)
            // Simplified: just check if we have enough bits for basic decoding
            if self.block_counter == 3 && self.message[0] != 0 {
                self.decode_group();
            }
        }
    }

    fn decode_group(&mut self) {
        // Extract PI code from block A (first 16 bits)
        let pi = self.message[0];
        self.info.pi_code = pi;
        
        // Extract group type from block B
        let block_b = self.message[1];
        let group_type = ((block_b >> 12) & 0x0F) as u8;
        let version_b = ((block_b >> 11) & 0x01) != 0;
        let tp = ((block_b >> 10) & 0x01) != 0;
        let pty = ((block_b >> 5) & 0x1F) as u8;
        
        self.info.programme_type = pty;
        
        let gt = RdsGroupType::from_code(group_type, version_b);
        let gt_name = format!("{:?}", gt);
        *self.info.group_stats.entry(gt_name).or_insert(0) += 1;
        
        match gt {
            RdsGroupType::Type0A | RdsGroupType::Type0B => {
                self.decode_type0(block_b);
            }
            RdsGroupType::Type2A | RdsGroupType::Type2B => {
                self.decode_type2(block_b, version_b);
            }
            RdsGroupType::Type4A => {
                self.decode_type4a();
            }
            RdsGroupType::Type14A => {
                self.decode_type14a(block_b);
            }
            _ => {}
        }
        
        self.info.good_block_count += 1;
    }

    fn decode_type0(&mut self, block_b: u16) {
        // TA, M/S, DI, C/I
        self.info.traffic_announcement = ((block_b >> 4) & 0x01) != 0;
        self.info.music_speech = ((block_b >> 3) & 0x01) == 0; // 0=music, 1=speech
        
        let di = ((block_b >> 2) & 0x01) != 0;
        let ci = (block_b & 0x03) as u8;
        
        // DI and CI bits convey decoder info
        match ci {
            0 => self.info.stereo = di,
            1 => self.info.artificial_head = di,
            2 => self.info.compressed = di,
            3 => self.info.dynamic_pty = di,
            _ => {}
        }
        
        // Programme Service name (8 characters, 2 per group)
        let segment = (block_b & 0x03) as usize;
        if segment < 4 {
            let c1 = (self.message[2] >> 8) as u8;
            let c2 = (self.message[2] & 0xFF) as u8;
            
            if segment * 2 < 8 {
                self.ps_chars[segment * 2] = c1;
                self.ps_chars[segment * 2 + 1] = c2;
                
                // Check if we have a complete PS name
                if segment == 3 {
                    let ps: String = self.ps_chars.iter()
                        .filter(|&&c| c >= 32 && c < 127)
                        .map(|&c| c as char)
                        .collect();
                    if !ps.is_empty() {
                        self.info.programme_service = ps;
                    }
                }
            }
        }
    }

    fn decode_type2(&mut self, block_b: u16, version_b: bool) {
        let segment = (block_b & 0x0F) as usize;
        let ab_flag = ((block_b >> 4) & 0x01) != 0;
        
        // Reset radio text if AB flag changed
        if ab_flag != self.rt_ab_flag {
            self.rt_chars = [0; 64];
            self.rt_ab_flag = ab_flag;
        }
        
        if version_b {
            // Type 2B: 32 characters, 2 per group
            if segment < 16 {
                let c1 = (self.message[2] >> 8) as u8;
                let c2 = (self.message[2] & 0xFF) as u8;
                self.rt_chars[segment * 2] = c1;
                self.rt_chars[segment * 2 + 1] = c2;
            }
        } else {
            // Type 2A: 64 characters, 4 per group
            if segment < 16 {
                let c1 = (self.message[2] >> 8) as u8;
                let c2 = (self.message[2] & 0xFF) as u8;
                let c3 = (self.message[3] >> 8) as u8;
                let c4 = (self.message[3] & 0xFF) as u8;
                
                self.rt_chars[segment * 4] = c1;
                self.rt_chars[segment * 4 + 1] = c2;
                self.rt_chars[segment * 4 + 2] = c3;
                self.rt_chars[segment * 4 + 3] = c4;
            }
        }
        
        // Update radio text string from accumulated characters
        let rt: String = self.rt_chars.iter()
            .take_while(|&&c| c != 0x0D && c != 0)
            .filter(|&&c| c >= 32 && c < 127)
            .map(|&c| c as char)
            .collect();
        
        if !rt.is_empty() {
            self.info.radio_text = rt;
        }
    }

    fn decode_type4a(&mut self) {
        // Clock time and date (simplified)
        // MJD (Modified Julian Date) is in blocks 2 and 3
        let mjd = ((self.message[1] as u32 & 0x03) << 15) | ((self.message[2] as u32) >> 1);
        let hours = ((self.message[2] & 0x01) as u8) << 4 | ((self.message[3] >> 12) & 0x0F) as u8;
        let minutes = ((self.message[3] >> 6) & 0x3F) as u8;
        
        if hours < 24 && minutes < 60 {
            self.info.clock_time = Some(format!("{:02}:{:02} (MJD: {})", hours, minutes, mjd));
        }
    }

    fn decode_type14a(&mut self, _block_b: u16) {
        // Enhanced Other Networks (EON)
        // Simplified: just note that EON info is present
        let _eon_pi = self.message[3];
        // Could extract TP, TA, and other network info here
    }

    // Simple bandpass filter (cascaded lowpass + highpass approximation)
    fn bandpass_filter(&self, signal: &[f32], low_hz: f32, high_hz: f32) -> Vec<f32> {
        let highpassed = self.highpass_filter(signal, low_hz);
        self.lowpass_filter(&highpassed, high_hz)
    }

    fn lowpass_filter(&self, signal: &[f32], cutoff_hz: f32) -> Vec<f32> {
        if signal.is_empty() {
            return vec![];
        }
        
        let rc = 1.0 / (2.0 * PI * cutoff_hz);
        let dt = 1.0 / self.sample_rate;
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

    fn highpass_filter(&self, signal: &[f32], cutoff_hz: f32) -> Vec<f32> {
        if signal.is_empty() {
            return vec![];
        }
        
        let rc = 1.0 / (2.0 * PI * cutoff_hz);
        let dt = 1.0 / self.sample_rate;
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

    pub fn reset(&mut self) {
        self.rds_buffer.clear();
        self.prev_sample = 0.0;
        self.phase_accumulator = 0.0;
        self.symbol_sync = 0;
        self.shift_reg = 0;
        self.message = [0; 4];
        self.block_counter = 0;
        self.info = RdsInfo::new();
        self.ps_chars = [0; 8];
        self.rt_chars = [0; 64];
        self.rt_ab_flag = false;
        self.rt_segment = 0;
    }
}

impl Default for RdsDecoder {
    fn default() -> Self {
        Self::new(2_048_000.0)
    }
}

/// Programme Type (PTY) decoder
pub fn pty_to_string(pty: u8) -> &'static str {
    match pty {
        0 => "No programme type",
        1 => "News",
        2 => "Current affairs",
        3 => "Information",
        4 => "Sport",
        5 => "Education",
        6 => "Drama",
        7 => "Culture",
        8 => "Science",
        9 => "Varied",
        10 => "Pop music",
        11 => "Rock music",
        12 => "Easy listening",
        13 => "Light classical",
        14 => "Serious classical",
        15 => "Other music",
        16 => "Weather",
        17 => "Finance",
        18 => "Children's programmes",
        19 => "Social affairs",
        20 => "Religion",
        21 => "Phone-in",
        22 => "Travel",
        23 => "Leisure",
        24 => "Jazz music",
        25 => "Country music",
        26 => "National music",
        27 => "Oldies music",
        28 => "Folk music",
        29 => "Documentary",
        30 => "Alarm test",
        31 => "Alarm",
        _ => "Unknown",
    }
}

/// Print RDS information in a readable format
pub fn print_rds_info(info: &RdsInfo) {
    println!("\n=== RDS Information ===");
    println!("PI Code:     0x{:04X}", info.pi_code);
    println!("Programme:   {}", if info.programme_service.is_empty() { "(not received yet)" } else { &info.programme_service });
    println!("PTY:         {} ({})", info.programme_type, info.pty_description());
    println!("Radio Text:  {}", if info.radio_text.is_empty() { "(not received yet)" } else { &info.radio_text });
    
    if let Some(ref ct) = info.clock_time {
        println!("Clock Time:  {}", ct);
    }
    
    println!("\nFeatures:");
    println!("  Traffic Announcement: {}", if info.traffic_announcement { "Yes" } else { "No" });
    println!("  Music/Speech:         {}", if info.music_speech { "Music" } else { "Speech" });
    println!("  Stereo:               {}", if info.stereo { "Yes" } else { "No" });
    println!("  Compressed:           {}", if info.compressed { "Yes" } else { "No" });
    
    if !info.group_stats.is_empty() {
        println!("\nGroup Statistics:");
        let mut stats: Vec<_> = info.group_stats.iter().collect();
        stats.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in stats.iter().take(10) {
            println!("  {}: {}", name, count);
        }
    }
    
    println!("\nGood blocks: {}, Errors: {}", info.good_block_count, info.error_count);
    println!("======================\n");
}
