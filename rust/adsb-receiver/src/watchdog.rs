//! Frame-stall watchdog.
//!
//! Why this exists: the agent is crash-only — it exits when the `rtl_adsb`
//! subprocess ends, and systemd's `Restart=always` brings it back. That covers
//! a dongle that *disappears*. It does not cover a dongle that stays attached
//! but goes silent. On 2026-10-05 the dongle dropped off USB twice, the kernel
//! power-cycled the port and re-enumerated it at full speed, and the third
//! `rtl_adsb` instance opened the degraded device and ran for 23 days producing
//! zero frames. Heartbeats kept flowing, so the hub reported the receiver
//! online the whole time. See `docs/incident-2026-10-05-silent-receiver.md`.
//!
//! The watchdog watches `Stats::frames_captured`. If the counter has not
//! advanced for `window`, the agent logs why and exits nonzero. systemd then
//! restarts it and the fresh `rtl_adsb` reopens (and re-initialises) the
//! device — the same thing the manual `systemctl restart` did on 2026-10-29.
//!
//! Default window: 10 minutes (`DEFAULT_STALL_TIMEOUT_SECS`). `rtl_adsb`
//! emits every Mode S frame it demodulates — DF11 all-call replies, DF4/DF5
//! surveillance replies, DF17 extended squitter — from every transponder
//! within the radio horizon (~230 nm for an aircraft at FL350). A ridgeline
//! receiver normally sees several frames per second; the quietest hour of the
//! night still carries transcontinental overflights, so ten consecutive
//! minutes without a single frame means the device, not the sky, is silent.
//! A quiet sky can therefore not trip it; a dead dongle trips it within
//! ten minutes instead of twenty-three days. Set the window to 0 to disable.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::stats::Stats;

/// Default stall window in seconds. See the module docs for the justification.
pub const DEFAULT_STALL_TIMEOUT_SECS: f64 = 600.0;

/// Shortest and longest intervals between counter checks. The check is one
/// atomic load, so polling often is cheap; polling rarely delays detection.
const MIN_POLL: Duration = Duration::from_secs(1);
const MAX_POLL: Duration = Duration::from_secs(30);

/// Result of one watchdog observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Frames are flowing, or the stall is still shorter than the window.
    Healthy,
    /// `frames_captured` has sat at `frames_captured` for `stalled_for`.
    Stalled {
        frames_captured: u64,
        stalled_for: Duration,
    },
}

/// Pure stall detector. Takes the clock as a parameter so tests can drive it
/// without sleeping; `run` wires it to the real clock and `Stats`.
#[derive(Debug)]
pub struct Watchdog {
    window: Duration,
    last_count: u64,
    last_advance: Instant,
}

impl Watchdog {
    /// Start the clock at `now`. The first window therefore also covers
    /// startup: an agent that opens an already-dead dongle exits after one
    /// window instead of running silently forever.
    pub fn new(window: Duration, now: Instant) -> Self {
        Self {
            window,
            last_count: 0,
            last_advance: now,
        }
    }

    /// A zero window disables the watchdog entirely.
    pub fn is_enabled(&self) -> bool {
        !self.window.is_zero()
    }

    /// How often `run` samples the counter: a tenth of the window, clamped
    /// to [1 s, 30 s].
    pub fn poll_interval(&self) -> Duration {
        (self.window / 10).clamp(MIN_POLL, MAX_POLL)
    }

    /// Feed the current `frames_captured` value and the current time.
    pub fn observe(&mut self, frames_captured: u64, now: Instant) -> Verdict {
        // The counter only ever increases in-process, but any change is
        // evidence of life — compare for inequality rather than ordering so
        // a counter wrap could never read as a stall.
        if frames_captured != self.last_count {
            self.last_count = frames_captured;
            self.last_advance = now;
            return Verdict::Healthy;
        }
        if !self.is_enabled() {
            return Verdict::Healthy;
        }
        let stalled_for = now.saturating_duration_since(self.last_advance);
        if stalled_for >= self.window {
            Verdict::Stalled {
                frames_captured,
                stalled_for,
            }
        } else {
            Verdict::Healthy
        }
    }
}

/// Sample `stats.frames_captured` on the real clock until it stalls for
/// `window`. Resolves to the stall verdict; never resolves when `window` is
/// zero, so callers can race it in a `select!` unconditionally.
pub async fn run(stats: Arc<Stats>, window: Duration) -> Verdict {
    let mut watchdog = Watchdog::new(window, Instant::now());
    if !watchdog.is_enabled() {
        eprintln!("[watchdog] disabled (stall timeout 0)");
        return std::future::pending().await;
    }
    eprintln!(
        "[watchdog] armed: exit if no frame is captured for {:.0} s",
        window.as_secs_f64()
    );
    let mut ticker = tokio::time::interval(watchdog.poll_interval());
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let count = stats.frames_captured.load(Ordering::Relaxed);
        if let verdict @ Verdict::Stalled { .. } = watchdog.observe(count, Instant::now()) {
            return verdict;
        }
    }
}

/// The operator-facing line logged when the watchdog trips. Direct and
/// actionable: what stalled, for how long, and what happens next.
pub fn stall_message(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Stalled {
            frames_captured,
            stalled_for,
        } => format!(
            "no frame captured for {:.0} s (frames_captured stuck at {}) — rtl_adsb is \
             running but silent, usually a dongle that re-enumerated in a bad state; \
             exiting so systemd restarts the agent and rtl_adsb reopens the device",
            stalled_for.as_secs_f64(),
            frames_captured
        ),
        Verdict::Healthy => "frames are flowing".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(600);

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn healthy_while_counter_advances() {
        let t0 = Instant::now();
        let mut wd = Watchdog::new(WINDOW, t0);
        // Several frames per second for an hour — never stalls.
        for i in 1..=3600u64 {
            assert_eq!(wd.observe(i * 3, t0 + secs(i)), Verdict::Healthy);
        }
    }

    #[test]
    fn stalls_after_window_without_advance() {
        let t0 = Instant::now();
        let mut wd = Watchdog::new(WINDOW, t0);
        assert_eq!(wd.observe(42, t0 + secs(1)), Verdict::Healthy);
        // Just short of the window: still healthy.
        assert_eq!(wd.observe(42, t0 + secs(1) + secs(599)), Verdict::Healthy);
        // At the window: stalled, reporting the stuck count and the idle time.
        assert_eq!(
            wd.observe(42, t0 + secs(1) + WINDOW),
            Verdict::Stalled {
                frames_captured: 42,
                stalled_for: WINDOW,
            }
        );
    }

    #[test]
    fn startup_against_dead_dongle_stalls_at_zero() {
        // The 2026-10-05 case: third instance starts, rtl_adsb opens the
        // degraded device, counter never leaves zero.
        let t0 = Instant::now();
        let mut wd = Watchdog::new(WINDOW, t0);
        assert_eq!(wd.observe(0, t0 + secs(300)), Verdict::Healthy);
        assert!(matches!(
            wd.observe(0, t0 + WINDOW),
            Verdict::Stalled {
                frames_captured: 0,
                ..
            }
        ));
    }

    #[test]
    fn advance_resets_the_clock() {
        let t0 = Instant::now();
        let mut wd = Watchdog::new(WINDOW, t0);
        assert_eq!(wd.observe(10, t0 + secs(590)), Verdict::Healthy);
        // 590 s idle after the advance — not yet.
        assert_eq!(wd.observe(10, t0 + secs(590) + secs(590)), Verdict::Healthy);
        // One more frame resets the window.
        assert_eq!(wd.observe(11, t0 + secs(590) + secs(595)), Verdict::Healthy);
        assert_eq!(
            wd.observe(11, t0 + secs(590) + secs(595) + secs(599)),
            Verdict::Healthy
        );
        assert!(matches!(
            wd.observe(11, t0 + secs(590) + secs(595) + WINDOW),
            Verdict::Stalled { .. }
        ));
    }

    #[test]
    fn zero_window_disables() {
        let t0 = Instant::now();
        let mut wd = Watchdog::new(Duration::ZERO, t0);
        assert!(!wd.is_enabled());
        assert_eq!(wd.observe(0, t0 + secs(86_400 * 30)), Verdict::Healthy);
    }

    #[test]
    fn poll_interval_is_clamped() {
        let t0 = Instant::now();
        assert_eq!(Watchdog::new(WINDOW, t0).poll_interval(), secs(30));
        assert_eq!(Watchdog::new(secs(100), t0).poll_interval(), secs(10));
        assert_eq!(Watchdog::new(secs(5), t0).poll_interval(), secs(1));
        assert_eq!(Watchdog::new(secs(3600), t0).poll_interval(), secs(30));
    }

    #[test]
    fn stall_message_names_cause_and_action() {
        let msg = stall_message(&Verdict::Stalled {
            frames_captured: 0,
            stalled_for: WINDOW,
        });
        assert!(msg.contains("600 s"), "{msg}");
        assert!(msg.contains("stuck at 0"), "{msg}");
        assert!(msg.contains("systemd restarts"), "{msg}");
    }

    #[tokio::test]
    async fn run_resolves_when_counter_stalls() {
        let stats = Arc::new(Stats::new());
        // Window of 2 s polls every 1 s; the counter never moves.
        let verdict = tokio::time::timeout(secs(10), run(stats, secs(2)))
            .await
            .expect("watchdog should trip within the timeout");
        assert!(matches!(
            verdict,
            Verdict::Stalled {
                frames_captured: 0,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn run_never_resolves_when_disabled() {
        let stats = Arc::new(Stats::new());
        let result =
            tokio::time::timeout(Duration::from_millis(200), run(stats, Duration::ZERO)).await;
        assert!(result.is_err(), "disabled watchdog must stay pending");
    }
}
