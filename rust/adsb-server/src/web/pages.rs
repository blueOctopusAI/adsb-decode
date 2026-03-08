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
a { color: #00aaff; }
.nav-hamburger { display: none; background: none; border: none; color: #888; font-size: 22px; cursor: pointer; padding: 4px 8px; line-height: 1; }
.nav-hamburger:hover { color: #00ff88; }
.nav-links { display: contents; }
.nav-right { margin-left: auto; display: flex; gap: 16px; align-items: center; }
footer .footer-inner { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 12px; }
footer .footer-links { display: flex; gap: 16px; }
@media (max-width: 768px) {
    body { overflow-x: hidden; }
    .container { padding: 12px; }
    nav { flex-wrap: wrap; padding: 8px 12px; gap: 0; }
    nav .brand { margin-right: auto; }
    .nav-hamburger { display: block; }
    .nav-links { display: none; flex-direction: column; width: 100%; padding: 8px 0 4px; gap: 2px; }
    .nav-links.open { display: flex; }
    .nav-links a { padding: 10px 12px; min-height: 44px; display: flex; align-items: center; font-size: 14px; border-radius: 4px; }
    .nav-links a:hover { background: rgba(255,255,255,0.05); }
    .nav-right { margin-left: 0; width: 100%; justify-content: stretch; gap: 8px; padding-top: 4px; border-top: 1px solid #222; margin-top: 4px; }
    .nav-right a { flex: 1; text-align: center; padding: 10px 12px; min-height: 44px; display: flex; align-items: center; justify-content: center; font-size: 14px; }
    footer .footer-inner { flex-direction: column; text-align: center; gap: 12px; }
    footer .footer-links { justify-content: center; }
    table { font-size: 12px; }
    th, td { padding: 6px 4px; }
    .stat-card { padding: 12px 16px; margin: 4px; }
    .stat-card .value { font-size: 24px; }
}"#;

const NAV_HTML: &str = r#"<nav>
    <a href="/" class="brand">adsb-decode</a>
    <button class="nav-hamburger" onclick="document.querySelector('.nav-links').classList.toggle('open')" aria-label="Toggle navigation">&#9776;</button>
    <div class="nav-links">
        <a href="/">Map</a>
        <a href="/table">Table</a>
        <a href="/events">Events</a>
        <a href="/query">Query</a>
        <a href="/replay">Replay</a>
        <a href="/receivers">Receivers</a>
        <a href="/stats">Stats</a>
        <a href="/about">About</a>
        <div class="nav-right">
            <a href="https://github.com/blueOctopusAI/adsb-decode" target="_blank" rel="noopener" title="GitHub" style="color:#888;">GitHub</a>
            <a href="/register" style="color:#00ff88;">Register</a>
        </div>
    </div>
</nav>"#;

const FOOTER_HTML: &str = r#"<footer style="background:#111; border-top:1px solid #333; padding:16px 24px; margin-top:32px; font-size:12px;">
    <div class="footer-inner">
        <div style="color:#888;">
            Built by <a href="https://www.blueoctopustechnology.com" target="_blank" rel="noopener" style="color:#00ff88; text-decoration:none;">Blue Octopus Technology</a>
            &mdash; data systems that turn messy inputs into clear intelligence.
        </div>
        <div class="footer-links">
            <a href="https://github.com/blueOctopusAI/adsb-decode" target="_blank" rel="noopener" style="color:#888; text-decoration:none;">GitHub</a>
            <a href="/about" style="color:#888; text-decoration:none;">About</a>
            <a href="https://www.blueoctopustechnology.com/contact" target="_blank" rel="noopener" style="color:#00ff88; text-decoration:none;">Contact</a>
        </div>
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

    const BASE_URL: &str = "https://adsb.blueoctopustechnology.com";

    // Favicon (inline SVG — radar sweep icon in brand green)
    s.push_str(r#"<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><circle cx='16' cy='16' r='14' fill='%230a0a0a' stroke='%2300ff88' stroke-width='2'/><path d='M16 16 L16 4 A12 12 0 0 1 27.4 10Z' fill='%2300ff88' opacity='0.6'/><circle cx='16' cy='16' r='3' fill='%2300ff88'/></svg>">"#);
    s.push('\n');

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
    s.push_str("<meta property=\"og:url\" content=\"");
    s.push_str(BASE_URL);
    s.push_str(canonical_path);
    s.push_str("\">\n");
    s.push_str("<meta property=\"og:image\" content=\"");
    s.push_str(BASE_URL);
    s.push_str("/og-image.png\">\n");
    s.push_str("<meta property=\"og:image:width\" content=\"1200\">\n");
    s.push_str("<meta property=\"og:image:height\" content=\"630\">\n");

    // Twitter Card
    s.push_str("<meta name=\"twitter:card\" content=\"summary_large_image\">\n");
    s.push_str("<meta name=\"twitter:title\" content=\"adsb-decode");
    if !title.is_empty() {
        s.push_str(" \u{2014} ");
        s.push_str(title);
    }
    s.push_str("\">\n");
    s.push_str("<meta name=\"twitter:description\" content=\"");
    s.push_str(description);
    s.push_str("\">\n");
    s.push_str("<meta name=\"twitter:image\" content=\"");
    s.push_str(BASE_URL);
    s.push_str("/og-image.png\">\n");

    // Canonical URL (absolute)
    s.push_str("<link rel=\"canonical\" href=\"");
    s.push_str(BASE_URL);
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

/// GET /og-image.png — Open Graph social share image (SVG served as image).
pub async fn og_image() -> impl IntoResponse {
    (
        [
            ("content-type", "image/svg+xml"),
            ("cache-control", "public, max-age=86400"),
        ],
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630" viewBox="0 0 1200 630">
  <rect width="1200" height="630" fill="#0a0a0a"/>
  <!-- Grid lines -->
  <g stroke="#1a1a1a" stroke-width="1">
    <line x1="0" y1="157" x2="1200" y2="157"/><line x1="0" y1="315" x2="1200" y2="315"/>
    <line x1="0" y1="472" x2="1200" y2="472"/><line x1="300" y1="0" x2="300" y2="630"/>
    <line x1="600" y1="0" x2="600" y2="630"/><line x1="900" y1="0" x2="900" y2="630"/>
  </g>
  <!-- Radar sweep -->
  <circle cx="850" cy="340" r="180" fill="none" stroke="#00ff88" stroke-width="1" opacity="0.2"/>
  <circle cx="850" cy="340" r="120" fill="none" stroke="#00ff88" stroke-width="1" opacity="0.15"/>
  <circle cx="850" cy="340" r="60" fill="none" stroke="#00ff88" stroke-width="1" opacity="0.1"/>
  <path d="M850 340 L850 160 A180 180 0 0 1 1005 255Z" fill="#00ff88" opacity="0.08"/>
  <circle cx="850" cy="340" r="4" fill="#00ff88"/>
  <!-- Aircraft dots -->
  <circle cx="920" cy="260" r="5" fill="#00aaff"/><circle cx="780" cy="290" r="4" fill="#00ff88"/>
  <circle cx="900" cy="400" r="4" fill="#ffaa00"/><circle cx="810" cy="370" r="3" fill="#ff4444"/>
  <!-- Trail line -->
  <polyline points="780,290 760,295 740,300 720,308 700,318" fill="none" stroke="#00ff88" stroke-width="2" opacity="0.5"/>
  <!-- Title -->
  <text x="80" y="260" font-family="monospace" font-size="72" font-weight="bold" fill="#00ff88">adsb-decode</text>
  <text x="80" y="320" font-family="monospace" font-size="28" fill="#888">Real-time aircraft tracking</text>
  <text x="80" y="365" font-family="monospace" font-size="20" fill="#555">ADS-B • AIS • 3D Globe • Military Alerts</text>
  <!-- Blue Octopus branding -->
  <text x="80" y="560" font-family="monospace" font-size="16" fill="#4FB4E8">Blue Octopus Technology</text>
</svg>"##,
    )
}
