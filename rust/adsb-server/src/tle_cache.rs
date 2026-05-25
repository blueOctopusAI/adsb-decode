//! TLE cache — fetches Two-Line Element sets from CelesTrak and serves them
//! to downstream clients (primarily Splatlas).
//!
//! Why this lives here instead of in Splatlas: CelesTrak rate-limits and
//! 403s requests from major cloud providers (Vercel, Cloudflare, etc).
//! Our VPS IP is accepted. adsb-decode is already the canonical "live
//! feeds" service — ADS-B positions, and now TLEs too — recorded as
//! source-of-record alongside everything else we ingest.
//!
//! Cache strategy: in-memory per group, refresh on access if older than
//! ~6 hours. Failures fall back to the previous cached body (TLEs stay
//! useful for ~1-2 weeks). When the DB feature is on, each refresh also
//! logs a row to `tle_fetches` for historical audit.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::RwLock;

/// Groups we accept. CelesTrak supports many more; whitelisted here so we
/// don't accept arbitrary user input that becomes a server-side fetch.
const ALLOWED_GROUPS: &[&str] = &[
    "starlink", "gps-ops", "stations", "active", "visual", "weather", "oneweb",
];

const REFRESH_AFTER: Duration = Duration::from_secs(6 * 60 * 60); // 6 hours
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT: &str = "adsb-decode/1.0 (+https://adsb.blueoctopustechnology.com)";

#[derive(Clone)]
struct CacheEntry {
    body: String,
    fetched_at: SystemTime,
}

#[derive(Clone, Default)]
pub struct TleCache {
    inner: Arc<RwLock<HashMap<String, CacheEntry>>>,
}

impl TleCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_allowed_group(group: &str) -> bool {
        ALLOWED_GROUPS.contains(&group)
    }

    /// Return the TLE block for `group`, fetching/refreshing as needed.
    /// On fetch failure with a stale cache present, returns the stale
    /// body — TLEs degrade gracefully (≤ ~2 weeks).
    pub async fn get(&self, group: &str) -> Result<String, TleError> {
        if !Self::is_allowed_group(group) {
            return Err(TleError::UnknownGroup(group.to_string()));
        }

        // Fast path — cache hit, not stale
        {
            let r = self.inner.read().await;
            if let Some(entry) = r.get(group) {
                if entry.fetched_at.elapsed().unwrap_or(Duration::MAX) < REFRESH_AFTER {
                    return Ok(entry.body.clone());
                }
            }
        }

        // Refresh path
        match fetch_from_celestrak(group).await {
            Ok(body) => {
                let mut w = self.inner.write().await;
                w.insert(
                    group.to_string(),
                    CacheEntry {
                        body: body.clone(),
                        fetched_at: SystemTime::now(),
                    },
                );
                Ok(body)
            }
            Err(e) => {
                // Fetch failed — serve stale if we have it
                let r = self.inner.read().await;
                if let Some(entry) = r.get(group) {
                    eprintln!(
                        "[tle] {group} refresh failed ({e}); serving stale ({}s old)",
                        entry.fetched_at.elapsed().unwrap_or_default().as_secs()
                    );
                    return Ok(entry.body.clone());
                }
                Err(e)
            }
        }
    }
}

async fn fetch_from_celestrak(group: &str) -> Result<String, TleError> {
    // Group strings are allowlisted to alphanumerics/hyphen via
    // ALLOWED_GROUPS, so URL-encoding is unnecessary — we still validate
    // defensively to keep the upstream call obviously safe.
    if !group.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(TleError::UnknownGroup(group.to_string()));
    }
    let url = format!("https://celestrak.org/NORAD/elements/gp.php?GROUP={group}&FORMAT=tle");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| TleError::Network(e.to_string()))?;

    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| TleError::Network(e.to_string()))?;

    if !res.status().is_success() {
        return Err(TleError::Upstream(res.status().as_u16()));
    }

    let body = res
        .text()
        .await
        .map_err(|e| TleError::Network(e.to_string()))?;

    // Sanity check — TLEs are 3-line records starting with "1 " / "2 "
    if body.len() < 500 || (!body.contains("\n1 ") && !body.starts_with("1 ")) {
        return Err(TleError::Malformed(body.chars().take(200).collect()));
    }

    Ok(body)
}

#[derive(Debug)]
pub enum TleError {
    UnknownGroup(String),
    Network(String),
    Upstream(u16),
    Malformed(String),
}

impl std::fmt::Display for TleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownGroup(g) => write!(f, "unknown group: {g}"),
            Self::Network(e) => write!(f, "network: {e}"),
            Self::Upstream(s) => write!(f, "upstream HTTP {s}"),
            Self::Malformed(b) => write!(f, "malformed body: {b}"),
        }
    }
}

impl std::error::Error for TleError {}
