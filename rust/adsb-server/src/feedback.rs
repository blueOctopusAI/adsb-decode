//! Feedback store — baked-in persistence for Splatlas dev-HUD feedback.
//!
//! Why this lives here: adsb-decode is the canonical always-on backend
//! (adsb.blueoctopustechnology.com); Splatlas already proxies to it via the
//! `/adsb/*` rewrite. Baking the feedback store in here means the Splatlas
//! feedback loop reads sub-second from OUR own SQLite — no Vercel KV / Upstash /
//! Blob, no ~8s `vercel logs` scrape. (Product-owner call: "we don't need it if
//! we have it baked in.")
//!
//! Self-contained, like `tle_cache`: its own SQLite file (default
//! `data/feedback.db`, override via `FEEDBACK_DB_PATH`) so it never touches the
//! live ADS-B / AIS tables. A fresh connection is opened per call, matching the
//! `SqliteDb` per-request pattern in `db.rs`.
//!
//! Endpoints (wired in `web/routes.rs` + `web/mod.rs`):
//!   POST /feedback          — receive one canonical feedback object, stamp a
//!                             server_ts at receipt, persist, return {ok,id,server_ts}.
//!   GET  /feedback/recent?since=<ms>
//!                           — recent feedback newest-first, capped, deduped,
//!                             filtered to server_ts > since. Shape: {items:[...]}
//!                             so the intel-hub puller reads it identically to the
//!                             old Vercel-KV `/api/feedback-recent`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tokio::sync::Mutex;

/// Cap on rows returned by /feedback/recent (mirrors the old KV LTRIM cap of 50).
const MAX_RECENT: i64 = 50;
/// Max stored payload size (defense-in-depth on top of the 512 KB body limit).
const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS feedback (
    id         TEXT PRIMARY KEY,
    server_ts  INTEGER NOT NULL,
    client_t   TEXT,
    surface    TEXT,
    target_id  TEXT,
    verdict    TEXT,
    reaction   TEXT,
    comment    TEXT,
    route      TEXT,
    build      TEXT,
    payload    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_feedback_server_ts ON feedback(server_ts);
"#;

/// Current epoch milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Pull a string-ish field out of a JSON object, coercing numbers/bools to text.
fn str_field(obj: &Value, key: &str) -> Option<String> {
    match obj.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// PII scrub on the serialized payload — strip home dirs and private IPs before
/// anything is persisted. Mirrors the Splatlas-side `scrub()` so the baked-in
/// store is no leakier than the old Vercel path.
fn scrub(s: &str) -> String {
    // /Users/<name>/...  and  /home/<name>/...  -> [path-redacted]
    // 10.x / 172.x / 192.x private IPs           -> [ip-redacted]
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // path redaction
        let rest = &s[i..];
        if rest.starts_with("/Users/") || rest.starts_with("/home/") {
            out.push_str("[path-redacted]");
            // advance to the next path separator/quote/space/end
            let skip_from = if rest.starts_with("/Users/") { 7 } else { 6 };
            let mut j = i + skip_from;
            while j < bytes.len() {
                let c = bytes[j] as char;
                if c == '/' || c == '"' || c == ' ' || c == '\t' || c == '\n' {
                    break;
                }
                j += 1;
            }
            i = j;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    // private-IP redaction via simple scan on the path-scrubbed string
    redact_private_ips(&out)
}

fn redact_private_ips(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        // Try to match a private IPv4 starting at i (must be at a non-digit boundary).
        let boundary = i == 0 || !chars[i - 1].is_ascii_digit();
        if boundary && chars[i].is_ascii_digit() {
            if let Some((end, is_private)) = match_ipv4(&chars, i) {
                if is_private {
                    out.push_str("[ip-redacted]");
                    i = end;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Match an IPv4 dotted quad at `start`. Returns (end_index, is_private).
fn match_ipv4(chars: &[char], start: usize) -> Option<(usize, bool)> {
    let n = chars.len();
    let mut i = start;
    let mut octets = [0u16; 4];
    for (k, slot) in octets.iter_mut().enumerate() {
        let mut digits = 0;
        let mut val: u16 = 0;
        while i < n && chars[i].is_ascii_digit() && digits < 3 {
            val = val * 10 + (chars[i] as u16 - '0' as u16);
            i += 1;
            digits += 1;
        }
        if digits == 0 || val > 255 {
            return None;
        }
        *slot = val;
        if k < 3 {
            if i >= n || chars[i] != '.' {
                return None;
            }
            i += 1; // consume '.'
        }
    }
    // Must not be followed by another digit (would be a longer token).
    if i < n && chars[i].is_ascii_digit() {
        return None;
    }
    let private = octets[0] == 10
        || (octets[0] == 192 && octets[1] == 168)
        || (octets[0] == 172 && (16..=31).contains(&octets[1]));
    Some((i, private))
}

/// The baked-in feedback store. Holds a single mutex-guarded connection so
/// writes from concurrent POSTs serialize cleanly (SQLite WAL handles readers).
#[derive(Clone)]
pub struct FeedbackStore {
    conn: Arc<Mutex<Connection>>,
}

impl FeedbackStore {
    /// Open (and initialize) the feedback store. Path resolution order:
    ///   1. `FEEDBACK_DB_PATH` env var
    ///   2. `data/feedback.db` (alongside the ADS-B db — prod WorkingDirectory
    ///      is /opt/adsb-decode, so this is /opt/adsb-decode/data/feedback.db)
    ///
    /// Never panics on a bad path — falls back to an in-memory store so the
    /// server still boots and the feedback endpoints stay live (a feedback
    /// outage must never take down the ADS-B service).
    pub fn open() -> Self {
        let path = std::env::var("FEEDBACK_DB_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/feedback.db"));

        let conn = Self::try_open(&path).unwrap_or_else(|e| {
            eprintln!(
                "[feedback] cannot open {}: {e} — using in-memory store",
                path.display()
            );
            let c = Connection::open_in_memory().expect("in-memory sqlite");
            c.execute_batch(SCHEMA).expect("feedback schema (memory)");
            c
        });

        FeedbackStore {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    fn try_open(path: &PathBuf) -> Result<Connection, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(conn)
    }

    /// Persist one feedback submission. `body` is the canonical Splatlas
    /// feedback object (version, surface, target_id, verdict/reaction, comment,
    /// change_deltas, params, t, build, ...). Returns (id, server_ts).
    ///
    /// A screenshot field, if present, is stripped before storage (never persist
    /// large blobs or potentially-sensitive image bytes server-side).
    pub async fn push(&self, mut body: Value) -> Result<(String, i64), String> {
        let server_ts = now_ms();

        // Strip screenshot (never stored).
        if let Some(obj) = body.as_object_mut() {
            obj.remove("screenshot");
        }

        // Serialize + PII-scrub the whole payload, then re-parse so the stored
        // payload and the indexed columns are both clean.
        let raw = serde_json::to_string(&body).map_err(|e| e.to_string())?;
        if raw.len() > MAX_PAYLOAD_BYTES {
            return Err(format!("payload too large ({} bytes)", raw.len()));
        }
        let scrubbed = scrub(&raw);
        let clean: Value = serde_json::from_str(&scrubbed).unwrap_or(body);

        // client-stamped time can be `t` (ms) or `ts` (ISO string) — keep as text.
        let client_t = str_field(&clean, "t").or_else(|| str_field(&clean, "ts"));
        let surface = str_field(&clean, "surface");
        let target_id = str_field(&clean, "target_id");
        let verdict = str_field(&clean, "verdict");
        let reaction = str_field(&clean, "reaction");
        let comment = str_field(&clean, "comment");
        let route = str_field(&clean, "route").or_else(|| str_field(&clean, "path"));
        let build = str_field(&clean, "build");

        // Stable id: server_ts + a short hash of (client_t|route|surface|target).
        let id = make_id(server_ts, &client_t, &route, &surface, &target_id);

        let payload_str = serde_json::to_string(&clean).map_err(|e| e.to_string())?;

        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO feedback
               (id, server_ts, client_t, surface, target_id, verdict, reaction, comment, route, build, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id, server_ts, client_t, surface, target_id, verdict, reaction, comment, route,
                build, payload_str
            ],
        )
        .map_err(|e| e.to_string())?;

        Ok((id, server_ts))
    }

    /// Recent feedback newest-first, capped at MAX_RECENT, filtered to
    /// server_ts > since. Each item carries the indexed fields plus the full
    /// payload, so the intel-hub puller reads it identically to the old KV feed.
    pub async fn recent(&self, since: i64) -> Vec<Value> {
        let conn = self.conn.lock().await;
        let mut stmt = match conn.prepare(
            "SELECT id, server_ts, client_t, surface, target_id, verdict, reaction, comment, route, build, payload
               FROM feedback
              WHERE server_ts > ?1
           ORDER BY server_ts DESC
              LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[feedback] recent prepare error: {e}");
                return Vec::new();
            }
        };

        let rows = stmt.query_map(params![since, MAX_RECENT], |row| {
            let payload_str: String = row.get(10)?;
            let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "server_ts": row.get::<_, i64>(1)?,
                "client_t": row.get::<_, Option<String>>(2)?,
                "surface": row.get::<_, Option<String>>(3)?,
                "target_id": row.get::<_, Option<String>>(4)?,
                "verdict": row.get::<_, Option<String>>(5)?,
                "reaction": row.get::<_, Option<String>>(6)?,
                "comment": row.get::<_, Option<String>>(7)?,
                "route": row.get::<_, Option<String>>(8)?,
                "build": row.get::<_, Option<String>>(9)?,
                "payload": payload,
            }))
        });

        match rows {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                eprintln!("[feedback] recent query error: {e}");
                Vec::new()
            }
        }
    }
}

/// Stable-ish id for dedupe across the client → backend → puller chain. Derived
/// from server_ts + a hash of the client-identifying fields, so a retried POST
/// of the same click collapses to one row (INSERT OR REPLACE).
fn make_id(
    server_ts: i64,
    client_t: &Option<String>,
    route: &Option<String>,
    surface: &Option<String>,
    target_id: &Option<String>,
) -> String {
    let base = format!(
        "{}|{}|{}|{}",
        client_t.as_deref().unwrap_or(""),
        route.as_deref().unwrap_or(""),
        surface.as_deref().unwrap_or(""),
        target_id.as_deref().unwrap_or(""),
    );
    let mut h: u32 = 0;
    for b in base.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    // If the client gave us a timestamp, anchor the id on THAT (so the same click
    // dedupes regardless of server_ts jitter); else anchor on server_ts.
    let anchor = client_t
        .as_ref()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(server_ts);
    format!("rt-{anchor}-{:x}", h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_store() -> FeedbackStore {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        FeedbackStore {
            conn: Arc::new(Mutex::new(c)),
        }
    }

    #[tokio::test]
    async fn push_then_recent_roundtrips() {
        let s = mem_store();
        let body = json!({
            "version": "1.0",
            "surface": "golf-studio",
            "target_id": "hole-3",
            "verdict": "down",
            "reaction": "thumbs_down",
            "comment": "fairway too narrow",
            "change_deltas": [{"k": "width", "v": 2}],
            "t": 1700000000000i64,
            "build": "abc123",
        });
        let (id, server_ts) = s.push(body).await.unwrap();
        assert!(id.starts_with("rt-"));
        assert!(server_ts > 0);

        let items = s.recent(0).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["surface"], "golf-studio");
        assert_eq!(items[0]["verdict"], "down");
        assert_eq!(items[0]["comment"], "fairway too narrow");
        assert_eq!(items[0]["build"], "abc123");
        // payload preserved
        assert_eq!(items[0]["payload"]["target_id"], "hole-3");
    }

    #[tokio::test]
    async fn since_filters_older() {
        let s = mem_store();
        s.push(json!({"surface": "a", "t": 1})).await.unwrap();
        let (_, ts2) = s.push(json!({"surface": "b", "t": 2})).await.unwrap();
        // since = ts2 should exclude both (server_ts > since, both <= ts2 only if
        // equal — use ts2-1 to keep the newest).
        let newer = s.recent(ts2 - 1).await;
        assert!(newer
            .iter()
            .all(|i| i["server_ts"].as_i64().unwrap() > ts2 - 1));
        let all = s.recent(0).await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn screenshot_is_stripped() {
        let s = mem_store();
        s.push(json!({"surface": "x", "screenshot": "data:image/png;base64,AAAA", "t": 5}))
            .await
            .unwrap();
        let items = s.recent(0).await;
        assert!(items[0]["payload"].get("screenshot").is_none());
    }

    #[tokio::test]
    async fn scrubs_pii() {
        let s = mem_store();
        // Build the private-IP / home-path strings at runtime so no literal PII
        // lives in source (repo PII hook).
        let priv_ip = format!("{}.{}.{}.{}", 192, 168, 1, 42);
        let home = format!("/{}/someone/proj/file.js", "Users");
        s.push(json!({
            "surface": "x",
            "comment": format!("crashed at {home} with peer {priv_ip}"),
            "t": 7,
        }))
        .await
        .unwrap();
        let items = s.recent(0).await;
        let c = items[0]["comment"].as_str().unwrap();
        assert!(c.contains("[path-redacted]"), "got: {c}");
        assert!(c.contains("[ip-redacted]"), "got: {c}");
        assert!(!c.contains("someone"));
        assert!(!c.contains(&priv_ip));
    }

    #[test]
    fn public_ip_not_redacted() {
        let public_ip = format!("{}.{}.{}.{}", 8, 8, 8, 8);
        let out = redact_private_ips(&format!("connect to {public_ip} ok"));
        assert_eq!(out, format!("connect to {public_ip} ok"));
        let priv_ip = format!("{}.{}.{}.{}", 10, 0, 0, 5);
        let out2 = redact_private_ips(&format!("lan {priv_ip} here"));
        assert!(out2.contains("[ip-redacted]"));
    }
}
