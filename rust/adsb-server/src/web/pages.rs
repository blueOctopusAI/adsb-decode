//! HTML page handlers — serves the dashboard UI.
//!
//! Each page is a complete HTML document composed from a shared base layout
//! and page-specific content (CSS + HTML + JS). Templates are embedded at
//! compile time via `include_str!`.

use axum::extract::Path;
use axum::response::{Html, IntoResponse};

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
    <a href="/about">About</a>
    <span style="margin-left:auto; display:flex; gap:16px; align-items:center;">
        <a href="https://github.com/blueOctopusAI/adsb-decode" target="_blank" rel="noopener" title="GitHub" style="color:#888;">GitHub</a>
        <a href="/register" style="color:#00ff88;">Register</a>
    </span>
</nav>"#;

const FOOTER_HTML: &str = r#"<footer style="background:#111; border-top:1px solid #333; padding:16px 24px; margin-top:32px; display:flex; justify-content:space-between; align-items:center; flex-wrap:wrap; gap:12px; font-size:12px;">
    <div style="color:#888;">
        Built by <a href="https://www.blueoctopustechnology.com" target="_blank" rel="noopener" style="color:#00ff88; text-decoration:none;">Blue Octopus Technology</a>
        &mdash; data systems that turn messy inputs into clear intelligence.
    </div>
    <div style="display:flex; gap:16px;">
        <a href="https://github.com/blueOctopusAI/adsb-decode" target="_blank" rel="noopener" style="color:#888; text-decoration:none;">GitHub</a>
        <a href="/about" style="color:#888; text-decoration:none;">About</a>
        <a href="https://www.blueoctopustechnology.com/contact" target="_blank" rel="noopener" style="color:#00ff88; text-decoration:none;">Contact</a>
    </div>
</footer>"#;

/// Per-page SEO metadata.
struct PageMeta {
    description: &'static str,
    path: &'static str,
}

fn render_page(title: &str, body: &str) -> Html<String> {
    render_page_with_meta(title, body, None)
}

fn render_page_with_meta(title: &str, body: &str, meta: Option<&PageMeta>) -> Html<String> {
    let description = meta
        .map(|m| m.description)
        .unwrap_or("Real-time ADS-B aircraft and AIS vessel tracking dashboard with 3D globe, replay, and natural language queries.");
    let canonical_path = meta.map(|m| m.path).unwrap_or("/");

    let mut s = String::with_capacity(body.len() + BASE_CSS.len() + NAV_HTML.len() + 2048);
    s.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    s.push_str("<meta charset=\"UTF-8\">\n");
    s.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");

    // SEO meta tags
    s.push_str("<meta name=\"description\" content=\"");
    s.push_str(description);
    s.push_str("\">\n");
    s.push_str("<meta name=\"robots\" content=\"index, follow\">\n");

    // Open Graph
    s.push_str("<meta property=\"og:type\" content=\"website\">\n");
    s.push_str("<meta property=\"og:title\" content=\"adsb-decode");
    if !title.is_empty() {
        s.push_str(" \u{2014} ");
        s.push_str(title);
    }
    s.push_str("\">\n");
    s.push_str("<meta property=\"og:description\" content=\"");
    s.push_str(description);
    s.push_str("\">\n");

    // Canonical URL (relative — works behind any domain)
    s.push_str("<link rel=\"canonical\" href=\"");
    s.push_str(canonical_path);
    s.push_str("\">\n");

    s.push_str("<title>adsb-decode");
    if !title.is_empty() {
        s.push_str(" \u{2014} ");
        s.push_str(title);
    }
    s.push_str("</title>\n");
    s.push_str(
        "<link rel=\"stylesheet\" href=\"https://unpkg.com/leaflet@1.9.4/dist/leaflet.css\" />\n",
    );
    s.push_str("<style>\n");
    s.push_str(BASE_CSS);
    s.push_str("\n</style>\n");

    // JSON-LD structured data (homepage only)
    if canonical_path == "/" {
        s.push_str(r#"<script type="application/ld+json">
{
    "@context": "https://schema.org",
    "@type": "WebApplication",
    "name": "adsb-decode",
    "description": "Real-time ADS-B aircraft and AIS vessel tracking dashboard",
    "applicationCategory": "UtilitiesApplication",
    "operatingSystem": "Web",
    "offers": { "@type": "Offer", "price": "0", "priceCurrency": "USD" },
    "author": { "@type": "Organization", "name": "Blue Octopus Technology", "url": "https://blueoctopustechnology.com" }
}
</script>
"#);
    }

    s.push_str("</head>\n<body>\n");
    s.push_str(NAV_HTML);
    s.push('\n');
    s.push_str(body);
    s.push('\n');
    s.push_str(FOOTER_HTML);
    s.push_str("\n</body>\n</html>");
    Html(s)
}

// ---------------------------------------------------------------------------
// Page handlers
// ---------------------------------------------------------------------------

const META_MAP: PageMeta = PageMeta {
    description: "Live aircraft tracking map with ADS-B radar, 3D globe view, altitude-colored trails, and military aircraft alerts.",
    path: "/",
};
const META_TABLE: PageMeta = PageMeta {
    description: "Sortable table of all tracked aircraft with ICAO, callsign, altitude, speed, and heading data.",
    path: "/table",
};
const META_EVENTS: PageMeta = PageMeta {
    description: "Real-time aviation event log including military aircraft detection, emergency squawks, and unusual altitude changes.",
    path: "/events",
};
const META_QUERY: PageMeta = PageMeta {
    description: "Search and filter tracked aircraft by ICAO, altitude, speed, military status, and natural language queries.",
    path: "/query",
};
const META_REPLAY: PageMeta = PageMeta {
    description: "4D replay of historical aircraft movements. Select a time range and watch flights unfold on the map.",
    path: "/replay",
};
const META_RECEIVERS: PageMeta = PageMeta {
    description: "Status dashboard for ADS-B receiver stations in the network. View coverage areas, uptime, and frame counts.",
    path: "/receivers",
};
const META_STATS: PageMeta = PageMeta {
    description: "System statistics including total aircraft tracked, positions recorded, events detected, and receiver count.",
    path: "/stats",
};
const META_REGISTER: PageMeta = PageMeta {
    description: "Register your ADS-B receiver to contribute aircraft tracking data to the network and get an API key.",
    path: "/register",
};
const META_ABOUT: PageMeta = PageMeta {
    description: "About adsb-decode: a complete ADS-B aircraft tracking system built from scratch in Rust by Blue Octopus Technology.",
    path: "/about",
};
const META_HOW_IT_WORKS: PageMeta = PageMeta {
    description: "How ADS-B decoding works: signal capture, demodulation, CRC validation, CPR position decoding, and aircraft tracking.",
    path: "/how-it-works",
};
const META_FEATURES: PageMeta = PageMeta {
    description: "Features of adsb-decode: live map, 3D globe, military detection, NLP queries, 4D replay, multi-receiver network, and AIS vessel tracking.",
    path: "/features",
};
const META_SETUP: PageMeta = PageMeta {
    description: "Set up an ADS-B receiver: hardware requirements, feeder binary download, and step-by-step installation guide.",
    path: "/setup",
};

pub async fn page_map() -> Html<String> {
    render_page_with_meta(
        "Map",
        include_str!("../../templates/map.html"),
        Some(&META_MAP),
    )
}

pub async fn page_table() -> Html<String> {
    render_page_with_meta(
        "Aircraft Table",
        include_str!("../../templates/table.html"),
        Some(&META_TABLE),
    )
}

pub async fn page_stats() -> Html<String> {
    render_page_with_meta(
        "Stats",
        include_str!("../../templates/stats.html"),
        Some(&META_STATS),
    )
}

pub async fn page_events() -> Html<String> {
    render_page_with_meta(
        "Events",
        include_str!("../../templates/events.html"),
        Some(&META_EVENTS),
    )
}

pub async fn page_detail(Path(_icao): Path<String>) -> Html<String> {
    render_page("Detail", include_str!("../../templates/detail.html"))
}

pub async fn page_query() -> Html<String> {
    render_page_with_meta(
        "Query",
        include_str!("../../templates/query.html"),
        Some(&META_QUERY),
    )
}

pub async fn page_replay() -> Html<String> {
    render_page_with_meta(
        "Replay",
        include_str!("../../templates/replay.html"),
        Some(&META_REPLAY),
    )
}

pub async fn page_receivers() -> Html<String> {
    render_page_with_meta(
        "Receivers",
        include_str!("../../templates/receivers.html"),
        Some(&META_RECEIVERS),
    )
}

pub async fn page_register() -> Html<String> {
    render_page_with_meta(
        "Register",
        include_str!("../../templates/register.html"),
        Some(&META_REGISTER),
    )
}

pub async fn page_about() -> Html<String> {
    render_page_with_meta(
        "About",
        include_str!("../../templates/about.html"),
        Some(&META_ABOUT),
    )
}

pub async fn page_how_it_works() -> Html<String> {
    render_page_with_meta(
        "How It Works",
        include_str!("../../templates/how-it-works.html"),
        Some(&META_HOW_IT_WORKS),
    )
}

pub async fn page_features() -> Html<String> {
    render_page_with_meta(
        "Features",
        include_str!("../../templates/features.html"),
        Some(&META_FEATURES),
    )
}

pub async fn page_setup() -> Html<String> {
    render_page_with_meta(
        "Setup",
        include_str!("../../templates/setup.html"),
        Some(&META_SETUP),
    )
}

// ---------------------------------------------------------------------------
// AI/SEO utility routes
// ---------------------------------------------------------------------------

/// GET /robots.txt
pub async fn robots_txt() -> impl IntoResponse {
    (
        [("content-type", "text/plain")],
        "User-agent: *\nAllow: /\n\nSitemap: /sitemap.xml\n",
    )
}

/// GET /sitemap.xml
pub async fn sitemap_xml() -> impl IntoResponse {
    (
        [("content-type", "application/xml")],
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://adsb.blueoctopustechnology.com/</loc><priority>1.0</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/about</loc><priority>0.9</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/how-it-works</loc><priority>0.8</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/features</loc><priority>0.8</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/setup</loc><priority>0.8</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/table</loc><priority>0.7</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/events</loc><priority>0.6</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/query</loc><priority>0.6</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/replay</loc><priority>0.6</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/receivers</loc><priority>0.5</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/stats</loc><priority>0.5</priority></url>
    <url><loc>https://adsb.blueoctopustechnology.com/register</loc><priority>0.7</priority></url>
</urlset>"#,
    )
}

/// GET /llms.txt — AI model context file
pub async fn llms_txt() -> impl IntoResponse {
    (
        [("content-type", "text/plain")],
        r#"# adsb-decode

> Real-time ADS-B aircraft and AIS vessel tracking dashboard by Blue Octopus Technology.

## What this site does

This is a live aircraft and vessel tracking system. It receives radio signals from aircraft (ADS-B on 1090 MHz) and ships (AIS), decodes them, and displays positions on a real-time map.

## Key features

- Live aircraft map with altitude-colored trails
- 3D globe view (Cesium)
- Military aircraft and emergency squawk detection
- Natural language queries ("show me military jets above FL300")
- 4D replay of historical flights
- Multi-receiver network aggregation
- AIS vessel tracking (ships, ferries, cargo)
- Aircraft photo integration
- Geofence alerting

## API

REST API available at /api/. Key endpoints:
- GET /api/positions — current aircraft positions
- GET /api/trails — recent flight trails
- GET /api/aircraft — aircraft metadata
- GET /api/events — detected events
- GET /api/vessels — tracked vessels
- GET /api/stats — system statistics
- POST /api/register — register a new receiver

## Built with

Rust (Axum, SQLx), Leaflet, CesiumJS, PostgreSQL/TimescaleDB

## Contact

Blue Octopus Technology — https://blueoctopustechnology.com
"#,
    )
}
