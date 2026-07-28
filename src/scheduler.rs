use anyhow::Result;
use chrono::{Datelike, Local, NaiveTime, Timelike, Weekday};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// A scheduled recording task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSchedule {
    pub name: String,
    pub frequency_hz: u64,
    pub sample_rate_hz: u32,
    pub duration_sec: u64,
    pub output_path: String,
    pub cron_expr: String,
    pub enabled: bool,
    pub last_run: Option<chrono::DateTime<Local>>,
    pub run_count: u32,
}

impl RecordingSchedule {
    pub fn new(
        name: impl Into<String>,
        frequency_hz: u64,
        duration_sec: u64,
        output_path: impl Into<String>,
        cron_expr: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            frequency_hz,
            sample_rate_hz: 2_048_000,
            duration_sec,
            output_path: output_path.into(),
            cron_expr: cron_expr.into(),
            enabled: true,
            last_run: None,
            run_count: 0,
        }
    }

    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.sample_rate_hz = rate;
        self
    }
}

/// Parsed cron expression
#[derive(Debug, Clone)]
struct CronExpr {
    minutes: Vec<u8>,  // 0-59
    hours: Vec<u8>,    // 0-23
    days: Vec<u8>,     // 1-31
    months: Vec<u8>,   // 1-12
    weekdays: Vec<u8>, // 0-6 (0=Sunday)
}

impl CronExpr {
    /// Parse a simplified cron expression: "min hour day month dow"
    /// Supports: * (all), n (specific), n,m,p (list), n-m (range), */n (step)
    fn parse(expr: &str) -> Result<Self> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(anyhow::anyhow!(
                "Cron expression must have 5 fields: min hour day month dow"
            ));
        }

        Ok(Self {
            minutes: Self::parse_field(parts[0], 0, 59)?,
            hours: Self::parse_field(parts[1], 0, 23)?,
            days: Self::parse_field(parts[2], 1, 31)?,
            months: Self::parse_field(parts[3], 1, 12)?,
            weekdays: Self::parse_field(parts[4], 0, 6)?,
        })
    }

    fn parse_field(field: &str, min: u8, max: u8) -> Result<Vec<u8>> {
        let mut values = Vec::new();

        for part in field.split(',') {
            if part == "*" {
                for v in min..=max {
                    values.push(v);
                }
            } else if let Some(step_str) = part.strip_prefix("*/") {
                let step: u8 = step_str
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid step in cron: {}", part))?;
                if step == 0 {
                    return Err(anyhow::anyhow!("Step cannot be zero"));
                }
                let mut v = min;
                while v <= max {
                    values.push(v);
                    v += step;
                }
            } else if part.contains('-') {
                let range: Vec<&str> = part.split('-').collect();
                if range.len() != 2 {
                    return Err(anyhow::anyhow!("Invalid range: {}", part));
                }
                let start: u8 = range[0]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid range start: {}", range[0]))?;
                let end: u8 = range[1]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid range end: {}", range[1]))?;
                for v in start..=end {
                    if v >= min && v <= max {
                        values.push(v);
                    }
                }
            } else {
                let v: u8 = part
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid value: {}", part))?;
                if v >= min && v <= max {
                    values.push(v);
                } else {
                    return Err(anyhow::anyhow!(
                        "Value {} out of range [{}-{}]",
                        v,
                        min,
                        max
                    ));
                }
            }
        }

        values.sort_unstable();
        values.dedup();
        Ok(values)
    }

    /// Check if the given time matches this cron expression
    fn matches(&self, dt: &chrono::DateTime<Local>) -> bool {
        self.minutes.contains(&(dt.minute() as u8))
            && self.hours.contains(&(dt.hour() as u8))
            && self.days.contains(&(dt.day() as u8))
            && self.months.contains(&(dt.month() as u8))
            && self
                .weekdays
                .contains(&(dt.weekday().num_days_from_sunday() as u8))
    }
}

/// Recording scheduler that manages multiple scheduled recordings
pub struct RecordingScheduler {
    schedules: Vec<RecordingSchedule>,
    check_interval_sec: u64,
}

impl RecordingScheduler {
    pub fn new() -> Self {
        Self {
            schedules: Vec::new(),
            check_interval_sec: 10,
        }
    }

    pub fn add_schedule(&mut self, schedule: RecordingSchedule) {
        self.schedules.push(schedule);
    }

    pub fn remove_schedule(&mut self, name: &str) -> bool {
        let len = self.schedules.len();
        self.schedules.retain(|s| s.name != name);
        self.schedules.len() < len
    }

    pub fn list_schedules(&self) -> &[RecordingSchedule] {
        &self.schedules
    }

    /// Run the scheduler loop, executing recordings when scheduled
    pub fn run<F>(&mut self, mut execute: F, running: Arc<AtomicBool>) -> Result<()>
    where
        F: FnMut(&RecordingSchedule) -> Result<()>,
    {
        println!(
            "Recording scheduler started with {} schedule(s)",
            self.schedules.len()
        );
        println!(
            "Checking every {} seconds. Press Ctrl+C to stop.",
            self.check_interval_sec
        );

        let mut last_checked_minute: Option<u8> = None;

        while running.load(Ordering::SeqCst) {
            let now = Local::now();
            let current_minute = now.minute() as u8;

            // Only check once per minute
            if last_checked_minute != Some(current_minute) {
                last_checked_minute = Some(current_minute);

                for schedule in &mut self.schedules {
                    if !schedule.enabled {
                        continue;
                    }

                    match CronExpr::parse(&schedule.cron_expr) {
                        Ok(cron) => {
                            if cron.matches(&now) {
                                // Check if we already ran this minute
                                let should_run = match schedule.last_run {
                                    Some(last) => {
                                        let last_minute = last.minute() as u8;
                                        let last_hour = last.hour() as u8;
                                        let last_day = last.day() as u8;
                                        last_minute != current_minute
                                            || last_hour != now.hour() as u8
                                            || last_day != now.day() as u8
                                    }
                                    None => true,
                                };

                                if should_run {
                                    println!(
                                        "[{}] Executing scheduled recording: {} at {} MHz",
                                        now.format("%Y-%m-%d %H:%M:%S"),
                                        schedule.name,
                                        schedule.frequency_hz as f64 / 1e6
                                    );

                                    match execute(schedule) {
                                        Ok(()) => {
                                            schedule.last_run = Some(Local::now());
                                            schedule.run_count += 1;
                                            println!(
                                                "[{}] Recording {} completed",
                                                Local::now().format("%Y-%m-%d %H:%M:%S"),
                                                schedule.name
                                            );
                                        }
                                        Err(e) => {
                                            eprintln!(
                                                "[{}] Recording {} failed: {}",
                                                Local::now().format("%Y-%m-%d %H:%M:%S"),
                                                schedule.name,
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "Invalid cron expression for schedule '{}': {}",
                                schedule.name, e
                            );
                        }
                    }
                }
            }

            thread::sleep(Duration::from_secs(self.check_interval_sec));
        }

        println!("Scheduler stopped.");
        Ok(())
    }

    /// Run scheduler in daemon mode (background)
    pub fn run_daemon<F>(&mut self, execute: F, running: Arc<AtomicBool>) -> Result<()>
    where
        F: FnMut(&RecordingSchedule) -> Result<()> + Send + 'static,
    {
        self.run(execute, running)
    }

    /// Save schedules to JSON file
    pub fn save<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.schedules)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load schedules from JSON file
    pub fn load<P: AsRef<std::path::Path>>(&mut self, path: P) -> Result<()> {
        let data = std::fs::read_to_string(&path)?;
        self.schedules = serde_json::from_str(&data)?;
        Ok(())
    }
}

impl Default for RecordingScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Quick helper to create a daily schedule at a specific time
pub fn daily_at(
    name: &str,
    frequency_hz: u64,
    duration_sec: u64,
    output_path: &str,
    time: &str,
) -> Result<RecordingSchedule> {
    let t = NaiveTime::parse_from_str(time, "%H:%M")
        .map_err(|_| anyhow::anyhow!("Time must be in HH:MM format"))?;

    let cron = format!("{} {} * * *", t.minute(), t.hour());

    Ok(RecordingSchedule::new(
        name,
        frequency_hz,
        duration_sec,
        output_path,
        cron,
    ))
}

/// Quick helper to create an hourly schedule
pub fn hourly(
    name: &str,
    frequency_hz: u64,
    duration_sec: u64,
    output_path: &str,
    minute: u8,
) -> RecordingSchedule {
    let cron = format!("{} * * * *", minute.clamp(0, 59));
    RecordingSchedule::new(name, frequency_hz, duration_sec, output_path, cron)
}

/// Quick helper to create a weekly schedule
pub fn weekly_at(
    name: &str,
    frequency_hz: u64,
    duration_sec: u64,
    output_path: &str,
    weekday: Weekday,
    time: &str,
) -> Result<RecordingSchedule> {
    let t = NaiveTime::parse_from_str(time, "%H:%M")
        .map_err(|_| anyhow::anyhow!("Time must be in HH:MM format"))?;

    let wd = weekday.num_days_from_sunday() as u8;
    let cron = format!("{} {} * * {}", t.minute(), t.hour(), wd);

    Ok(RecordingSchedule::new(
        name,
        frequency_hz,
        duration_sec,
        output_path,
        cron,
    ))
}
