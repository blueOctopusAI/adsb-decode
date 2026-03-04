use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub struct Stats {
    pub frames_captured: AtomicU64,
    pub frames_sent: AtomicU64,
    start_time: Instant,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            frames_captured: AtomicU64::new(0),
            frames_sent: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    pub fn uptime_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub fn summary(&self) -> String {
        let captured = self.frames_captured.load(Ordering::Relaxed);
        let sent = self.frames_sent.load(Ordering::Relaxed);
        let uptime = self.uptime_secs();
        format!(
            "uptime: {:.0}s | captured: {} | sent: {} | rate: {:.1}/s",
            uptime,
            captured,
            sent,
            if uptime > 0.0 {
                captured as f64 / uptime
            } else {
                0.0
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_new_stats() {
        let stats = Stats::new();
        assert_eq!(stats.frames_captured.load(Ordering::Relaxed), 0);
        assert_eq!(stats.frames_sent.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_increment_counters() {
        let stats = Stats::new();
        stats.frames_captured.fetch_add(10, Ordering::Relaxed);
        stats.frames_sent.fetch_add(5, Ordering::Relaxed);
        assert_eq!(stats.frames_captured.load(Ordering::Relaxed), 10);
        assert_eq!(stats.frames_sent.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn test_uptime() {
        let stats = Stats::new();
        thread::sleep(Duration::from_millis(50));
        assert!(stats.uptime_secs() >= 0.04);
    }

    #[test]
    fn test_summary_format() {
        let stats = Stats::new();
        stats.frames_captured.fetch_add(100, Ordering::Relaxed);
        stats.frames_sent.fetch_add(90, Ordering::Relaxed);
        let summary = stats.summary();
        assert!(summary.contains("captured: 100"));
        assert!(summary.contains("sent: 90"));
        assert!(summary.contains("uptime:"));
        assert!(summary.contains("rate:"));
    }
}
