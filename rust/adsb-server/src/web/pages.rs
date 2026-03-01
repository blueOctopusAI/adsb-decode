//! HTML page handlers — serves the dashboard UI.
//!
//! Each page is a complete HTML document composed from a shared base layout
//! and page-specific content (CSS + HTML + JS). Templates are embedded at
//! compile time via `include_str!`.

use axum::extract::Path;
use axum::response::Html;

const BASE_CSS: &str = r#"* { margin: 0; padding: 0; box-sizing: border-box; }
body { font-family: 'Courier New', monospace; background: #0a0a0a; color: #e0e0e0; }
nav { background: #111; border-bottom: 1px solid #333; padding: 8px 16px; display: flex; align-items: center; gap: 24px; }
nav .brand { color: #00ff88; font-weight: bold; font-size: 14px; text-decoration: none; }
nav a { color: #888; text-decoration: none; font-size: 13px; }
nav a:hover, nav a.active { color: #00ff88; }
.container { padding: 16px; }
table { width: 100%; border-collapse: collapse; font-size: 13px; }
th { background: #1a1a1a; color: #00ff88; padding: 8px; text-align: left; border-bottom: 1px solid #333; cursor: pointer; }
td { padding: 6px 8px; border-bottom: 1px solid #1a1a1a; }
tr:hover { background: #111; }
.mil { color: #ff4444; font-weight: bold; }
.emergency { color: #ff8800; font-weight: bold; }
.stat-card { display: inline-block; background: #111; border: 1px solid #333; padding: 16px 24px; margin: 8px; border-radius: 4px; }
.stat-card .value { font-size: 32px; color: #00ff88; font-weight: bold; }
.stat-card .label { font-size: 12px; color: #888; margin-top: 4px; }
a { color: #00aaff; }"#;

const NAV_HTML: &str = r#"<nav>
    <a href="/" class="brand">adsb-decode</a>
    <a href="/">Map</a>
    <a href="/table">Table</a>
    <a href="/events">Events</a>
    <a href="/query">Query</a>
    <a href="/replay">Replay</a>
    <a href="/receivers">Receivers</a>
    <a href="/stats">Stats</a>
</nav>"#;

fn render_page(title: &str, body: &str) -> Html<String> {
    let mut s = String::with_capacity(body.len() + BASE_CSS.len() + NAV_HTML.len() + 512);
    s.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    s.push_str("<meta charset=\"UTF-8\">\n");
    s.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    s.push_str("<title>adsb-decode");
    if !title.is_empty() {
        s.push_str(" \u{2014} "); // em dash
        s.push_str(title);
    }
    s.push_str("</title>\n");
    s.push_str("<link rel=\"stylesheet\" href=\"https://unpkg.com/leaflet@1.9.4/dist/leaflet.css\" />\n");
    s.push_str("<style>\n");
    s.push_str(BASE_CSS);
    s.push_str("\n</style>\n");
    s.push_str("</head>\n<body>\n");
    s.push_str(NAV_HTML);
    s.push_str("\n");
    s.push_str(body);
    s.push_str("\n</body>\n</html>");
    Html(s)
}

// ---------------------------------------------------------------------------
// Page handlers
// ---------------------------------------------------------------------------

pub async fn page_map() -> Html<String> {
    render_page("Map", include_str!("../../templates/map.html"))
}

pub async fn page_table() -> Html<String> {
    render_page("Aircraft Table", include_str!("../../templates/table.html"))
}

pub async fn page_stats() -> Html<String> {
    render_page("Stats", include_str!("../../templates/stats.html"))
}

pub async fn page_events() -> Html<String> {
    render_page("Events", include_str!("../../templates/events.html"))
}

pub async fn page_detail(Path(_icao): Path<String>) -> Html<String> {
    render_page("Detail", include_str!("../../templates/detail.html"))
}

pub async fn page_query() -> Html<String> {
    render_page("Query", include_str!("../../templates/query.html"))
}

pub async fn page_replay() -> Html<String> {
    render_page("Replay", include_str!("../../templates/replay.html"))
}

pub async fn page_receivers() -> Html<String> {
    render_page("Receivers", include_str!("../../templates/receivers.html"))
}
