// --- 3D globe config ---
// Paste a FREE Cesium Ion token (cesium.com/ion -> Access Tokens) here to turn on
// real elevation (mountains under the planes) + sun-shaded relief in the 3D globe.
// Empty = flat globe (the prior behavior).
const CESIUM_ION_TOKEN = '';
// Aircraft model for the 3D globe (banking planes). Ships with Cesium on the CDN.
const CESIUM_AIR_GLB = 'https://cdn.jsdelivr.net/npm/cesium@1.119/Build/Cesium/Apps/SampleData/models/CesiumAir/Cesium_Air.glb';
// If the model nose points the wrong way after deploy, nudge this (e.g. 90 / -90).
const MODEL_HEADING_OFFSET = 0;

// --- Map initialization ---
// URL params: ?lat=X&lon=Y&zoom=Z optionally with &focus=<splatlas-scene-id>
// drive an initial view. Splatlas deep-links into this map using those params
// so users can hop from a captured 3D scene to the live airspace context
// around it. mapCentered is set if params override the default so later
// auto-centering doesn't fight the deep-link.
let mapCentered = false;
const _params = new URLSearchParams(window.location.search);
const _paramLat = parseFloat(_params.get('lat'));
const _paramLon = parseFloat(_params.get('lon'));
const _paramZoom = parseFloat(_params.get('zoom'));
const _initCenter = (Number.isFinite(_paramLat) && Number.isFinite(_paramLon))
    ? [_paramLat, _paramLon] : [35.18, -83.38];
const _initZoom = Number.isFinite(_paramZoom) ? _paramZoom : 7;
if (Number.isFinite(_paramLat) && Number.isFinite(_paramLon)) mapCentered = true;

const map = L.map('map', {
    center: _initCenter,
    zoom: _initZoom,
    zoomControl: true,
    preferCanvas: true,  // Render polylines on canvas instead of SVG DOM nodes
});

// --- Map tile styles ---
const tileSets = {
    'dark': { url: 'https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png', attr: '&copy; OSM &copy; CARTO', maxZoom: 19 },
    'satellite': { url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}', attr: '&copy; Esri', maxZoom: 19 },
    'topo': { url: 'https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png', attr: '&copy; OSM &copy; OpenTopoMap', maxZoom: 17 },
    'streets': { url: 'https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', attr: '&copy; OSM', maxZoom: 19 },
    'dark-matter': { url: 'https://{s}.basemaps.cartocdn.com/dark_nolabels/{z}/{x}/{y}{r}.png', attr: '&copy; OSM &copy; CARTO', maxZoom: 19 },
    'voyager': { url: 'https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}{r}.png', attr: '&copy; OSM &copy; CARTO', maxZoom: 19 },
};
let currentTileLayer = null;
const mapThemes = {
    dark:         { bg: 'dark',  civilian: '#00ff88', military: '#ff4444', trailWeight: 2.5, trailMinOpacity: 0.3, outline: '#000', outlineWidth: 0.5 },
    satellite:    { bg: 'dark',  civilian: '#00ffcc', military: '#ff6666', trailWeight: 3,   trailMinOpacity: 0.4, outline: '#000', outlineWidth: 0.8 },
    topo:         { bg: 'light', civilian: '#006633', military: '#cc0000', trailWeight: 3,   trailMinOpacity: 0.5, outline: '#fff', outlineWidth: 0.8 },
    streets:      { bg: 'light', civilian: '#006633', military: '#cc0000', trailWeight: 3,   trailMinOpacity: 0.5, outline: '#fff', outlineWidth: 0.8 },
    'dark-matter': { bg: 'dark', civilian: '#00ff88', military: '#ff4444', trailWeight: 2.5, trailMinOpacity: 0.3, outline: '#000', outlineWidth: 0.5 },
    voyager:      { bg: 'light', civilian: '#007744', military: '#cc0000', trailWeight: 3,   trailMinOpacity: 0.5, outline: '#fff', outlineWidth: 0.8 },
};
let activeTheme = mapThemes['dark'];
const cesiumTiles = {
    'dark':        { url: 'https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png', subs: ['a','b','c','d'], max: 19 },
    'satellite':   { url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}', subs: undefined, max: 19 },
    'topo':        { url: 'https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png', subs: ['a','b','c'], max: 17 },
    'streets':     { url: 'https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', subs: ['a','b','c'], max: 19 },
    'dark-matter': { url: 'https://{s}.basemaps.cartocdn.com/dark_nolabels/{z}/{x}/{y}.png', subs: ['a','b','c','d'], max: 19 },
    'voyager':     { url: 'https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}.png', subs: ['a','b','c','d'], max: 19 },
};

async function setCesiumMapStyle(style) {
    if (!cesiumViewer) return;
    try {
        cesiumViewer.imageryLayers.removeAll();
        let imagery;
        if (style === 'satellite') {
            imagery = await Cesium.ArcGisMapServerImageryProvider.fromUrl(
                'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer'
            );
        } else {
            const ct = cesiumTiles[style] || cesiumTiles['dark'];
            imagery = new Cesium.UrlTemplateImageryProvider({
                url: ct.url,
                subdomains: ct.subs,
                maximumLevel: ct.max,
            });
        }
        cesiumViewer.imageryLayers.addImageryProvider(imagery);
        const isDark = (mapThemes[style] || mapThemes['dark']).bg === 'dark';
        cesiumViewer.scene.backgroundColor = Cesium.Color.fromCssColorString(isDark ? '#0a0a0a' : '#d0d8e0');
        cesiumViewer.scene.globe.baseColor = Cesium.Color.fromCssColorString(isDark ? '#0a0a0a' : '#d0d8e0');
    } catch(e) { console.error('Cesium tile switch failed:', e); }
}

function setMapStyle(style) {
    const ts = tileSets[style] || tileSets['dark'];
    if (currentTileLayer) map.removeLayer(currentTileLayer);
    currentTileLayer = L.tileLayer(ts.url, { attribution: ts.attr, maxZoom: ts.maxZoom }).addTo(map);
    document.getElementById('map-style').value = style;
    activeTheme = mapThemes[style] || mapThemes['dark'];
    if (is3DMode) setCesiumMapStyle(style);
    const bar = document.querySelector('#alt-legend .legend-bar');
    if (bar) {
        if (activeTheme.bg === 'light') {
            bar.style.background = 'linear-gradient(to top, #003cb3, #b45a00 50%, #cc0000)';
        } else {
            bar.style.background = 'linear-gradient(to top, #00ff00, #ffff00 50%, #ff0000)';
        }
    }
}
let is3DMode = false;
setMapStyle('dark');

const markers = {};
const trailLines = {};
// mapCentered already declared at top of file (URL-param init block); the
// historical re-declaration here moved into the init block above.

// --- Aircraft type classification ---
function classifyAircraft(p) {
    if (p.is_military) return 'military';
    if (p.speed_kts != null) {
        if (p.speed_kts > 250) return 'jet';
        if (p.speed_kts < 80 && p.altitude_ft != null && p.altitude_ft < 3000) return 'helicopter';
        if (p.speed_kts <= 180) return 'prop';
        if (p.speed_kts <= 250) return 'turboprop';
    }
    if (p.altitude_ft != null) {
        if (p.altitude_ft > 30000) return 'jet';
        if (p.altitude_ft < 5000) return 'prop';
    }
    return 'jet';
}

// --- Aircraft silhouette SVGs ---
const acSvgs = {
    jet: (color) => `<path d="M12,2 L14,8 L14,10 L22,14 L22,16 L14,14 L14,19 L17,21 L17,23 L12,21 L7,23 L7,21 L10,19 L10,14 L2,16 L2,14 L10,10 L10,8 Z" fill="${color}" stroke="#000" stroke-width="0.4"/>`,
    prop: (color) => `<path d="M12,3 L13.5,9 L13,10 L18,13 L18,14.5 L13,13 L13,19 L15,21 L15,22.5 L12,21 L9,22.5 L9,21 L11,19 L11,13 L6,14.5 L6,13 L11,10 L10.5,9 Z" fill="${color}" stroke="#000" stroke-width="0.4"/>`,
    turboprop: (color) => `<path d="M12,2.5 L13.5,8.5 L13.5,10 L20,13.5 L20,15 L13.5,13 L13.5,19 L16,21 L16,22.5 L12,21 L8,22.5 L8,21 L10.5,19 L10.5,13 L4,15 L4,13.5 L10.5,10 L10.5,8.5 Z" fill="${color}" stroke="#000" stroke-width="0.4"/>`,
    helicopter: (color) => `<circle cx="12" cy="13" r="4" fill="${color}" stroke="#000" stroke-width="0.4"/>
        <line x1="4" y1="9" x2="20" y2="9" stroke="${color}" stroke-width="1.5"/>
        <line x1="12" y1="9" x2="12" y2="13" stroke="${color}" stroke-width="1"/>
        <line x1="10" y1="17" x2="14" y2="17" stroke="${color}" stroke-width="1"/>
        <line x1="12" y1="17" x2="12" y2="21" stroke="${color}" stroke-width="1"/>
        <line x1="9" y1="21" x2="15" y2="21" stroke="${color}" stroke-width="1.2"/>`,
    military: (color) => `<path d="M12,1 L14.5,7 L14,9 L22,12 L22,14 L14,12.5 L14,18 L17,20.5 L17,22 L12,20 L7,22 L7,20.5 L10,18 L10,12.5 L2,14 L2,12 L10,9 L9.5,7 Z" fill="${color}" stroke="#000" stroke-width="0.5"/>`,
};

function acIcon(heading, isMilitary, acType) {
    const color = isMilitary ? activeTheme.military : activeTheme.civilian;
    const type = acType || (isMilitary ? 'military' : 'jet');
    const svgBody = (acSvgs[type] || acSvgs.jet)(color);
    const rotation = heading || 0;
    return L.divIcon({
        className: '',
        html: `<svg width="24" height="24" viewBox="0 0 24 24" style="transform:rotate(${rotation}deg);filter:drop-shadow(0 0 1px ${activeTheme.outline})">${svgBody}</svg>`,
        iconSize: [24, 24],
        iconAnchor: [12, 12],
    });
}

// --- Altitude color gradient ---
function altColor(alt) {
    if (alt == null) return activeTheme.bg === 'dark' ? '#888' : '#555';
    const t = Math.min(Math.max(alt / 40000, 0), 1);
    if (activeTheme.bg === 'light') {
        if (t < 0.5) {
            const s = t * 2;
            const r = Math.round(s * 180);
            const g = Math.round(40 + (1 - s) * 60);
            const b = Math.round((1 - s) * 180);
            return `rgb(${r},${g},${b})`;
        } else {
            const s = (t - 0.5) * 2;
            const r = Math.round(180 + s * 60);
            const g = Math.round(40 * (1 - s));
            return `rgb(${r},${g},0)`;
        }
    }
    if (t < 0.5) {
        const s = t * 2;
        return `rgb(${Math.round(s * 255)},255,0)`;
    } else {
        const s = (t - 0.5) * 2;
        return `rgb(255,${Math.round((1 - s) * 255)},0)`;
    }
}

// --- HTML escaping for XSS prevention ---
function esc(s) {
    if (s == null) return '';
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#x27;');
}

// --- Popup content ---
// Anomaly score thresholds. Rules-based scorer contributes 0.5\u20132.0 per rule;
// statistical scorer adds up to 5.0. Combined score in /api/positions for
// the live tracker path is rules-only (DB column has the combined value).
// `0.5` is the smallest single rule firing (stuck_position / nonmonotonic_time).
function anomalyClass(score) {
    if (score == null || score < 0.5) return null;
    if (score < 2.0) return 'low';
    if (score < 4.0) return 'med';
    return 'high';
}

function popupHtml(p, acType) {
    const mil = p.is_military ? '<span class="popup-mil"> [MIL]</span>' : '';
    const name = esc(p.callsign || p.registration || p.icao);
    const typeLabel = esc((acType || 'unknown').charAt(0).toUpperCase() + (acType || 'unknown').slice(1));
    const hexIcao = esc(p.icao.toUpperCase());
    const anomalyCls = anomalyClass(p.anomaly_score);
    const anomalyRow = anomalyCls
        ? `<div class="popup-row"><span class="popup-label">Anomaly</span><span class="popup-value popup-anomaly-${anomalyCls}">${p.anomaly_score.toFixed(2)} (${anomalyCls})</span></div>`
        : '';
    return `<div class="ac-popup">
        <div class="popup-title">${name}${mil}</div>
        <div class="popup-row"><span class="popup-label">ICAO</span><span class="popup-value">${esc(p.icao)}</span></div>
        <div class="popup-row"><span class="popup-label">Type</span><span class="popup-value">${typeLabel}</span></div>
        ${p.callsign ? `<div class="popup-row"><span class="popup-label">Callsign</span><span class="popup-value">${esc(p.callsign)}</span></div>` : ''}
        ${p.registration ? `<div class="popup-row"><span class="popup-label">Reg</span><span class="popup-value">${esc(p.registration)}</span></div>` : ''}
        ${p.country ? `<div class="popup-row"><span class="popup-label">Country</span><span class="popup-value">${esc(p.country)}</span></div>` : ''}
        <div class="popup-row"><span class="popup-label">Altitude</span><span class="popup-value">${p.altitude_ft != null ? p.altitude_ft.toLocaleString() + ' ft' : '--'}</span></div>
        <div class="popup-row"><span class="popup-label">Speed</span><span class="popup-value">${p.speed_kts != null ? Math.round(p.speed_kts) + ' kts' : '--'}</span></div>
        <div class="popup-row"><span class="popup-label">Heading</span><span class="popup-value">${p.heading_deg != null ? Math.round(p.heading_deg) + '\u00B0' : '--'}</span></div>
        <div class="popup-row"><span class="popup-label">VRate</span><span class="popup-value">${p.vertical_rate_fpm != null ? p.vertical_rate_fpm + ' fpm' : '--'}</span></div>
        ${anomalyRow}
        <div id="photo-${esc(p.icao)}" style="margin-top:6px;text-align:center;"></div>
        <div style="margin-top:6px; display:flex; gap:8px; flex-wrap:wrap;">
            <a href="/aircraft/${encodeURIComponent(p.icao)}">Detail &rarr;</a>
            ${p.registration ? `<a href="https://www.planespotters.net/search?q=${encodeURIComponent(p.registration)}" target="_blank" rel="noopener">Photos</a>` : `<a href="https://www.planespotters.net/hex/${encodeURIComponent(hexIcao)}" target="_blank" rel="noopener">Photos</a>`}
            <a href="https://globe.adsbexchange.com/?icao=${encodeURIComponent(hexIcao)}" target="_blank" rel="noopener">ADSBx</a>
        </div>
    </div>`;
}

// --- Aircraft photo loader ---
const photoCache = {};
function loadAircraftPhoto(icao) {
    const el = document.getElementById('photo-' + icao);
    if (!el) return;
    if (photoCache[icao] !== undefined) {
        renderPhoto(el, photoCache[icao]);
        return;
    }
    el.innerHTML = '<span style="color:#666;font-size:10px;">Loading photo...</span>';
    fetch('/api/photos/' + icao)
        .then(r => r.json())
        .then(data => {
            photoCache[icao] = data;
            renderPhoto(el, data);
        })
        .catch(() => { el.innerHTML = ''; });
}

function renderPhoto(el, data) {
    if (!data || !data.photos || data.photos.length === 0) {
        el.innerHTML = '';
        return;
    }
    const photo = data.photos[0];
    const thumb = photo.thumbnail_large || photo.thumbnail || {};
    const src = thumb.src || '';
    if (!src) { el.innerHTML = ''; return; }
    const credit = esc(photo.photographer || 'Unknown');
    const link = esc(photo.link || '#');
    el.innerHTML = `<a href="${link}" target="_blank" rel="noopener"><img src="${esc(src)}" style="max-width:100%;border-radius:3px;margin-top:4px;" alt="Aircraft photo"></a>
        <div style="font-size:9px;color:#666;">\u00A9 ${credit}</div>`;
}

// --- Draw trail (batched by altitude band for fewer canvas draw calls) ---
function drawTrail(icao, points) {
    if (trailLines[icao]) {
        trailLines[icao].forEach(seg => map.removeLayer(seg));
    }
    trailLines[icao] = [];
    if (!trailsEnabled) return;
    if (points.length < 2) return;

    // Break trail when consecutive points are too far apart (gap = sparse reception)
    const MAX_GAP_KM = 50;
    function haversineKm(a, b) {
        const R = 6371, toRad = Math.PI / 180;
        const dLat = (b[0] - a[0]) * toRad, dLon = (b[1] - a[1]) * toRad;
        const s = Math.sin(dLat/2)**2 + Math.cos(a[0]*toRad) * Math.cos(b[0]*toRad) * Math.sin(dLon/2)**2;
        return 2 * R * Math.asin(Math.sqrt(s));
    }

    // Batch consecutive points in the same altitude band (5000ft) into single polylines
    let batchCoords = [[points[0].lat, points[0].lon]];
    let batchColor = altColor(points[0].altitude_ft);
    let batchAltBand = points[0].altitude_ft != null ? Math.floor(points[0].altitude_ft / 5000) : -1;

    function flushBatch(opacity) {
        if (batchCoords.length >= 2) {
            const seg = L.polyline(batchCoords, {
                color: batchColor, weight: activeTheme.trailWeight, opacity: opacity
            }).addTo(map);
            trailLines[icao].push(seg);
        }
    }

    for (let i = 1; i < points.length; i++) {
        const curr = points[i];
        const currAltBand = curr.altitude_ft != null ? Math.floor(curr.altitude_ft / 5000) : -1;
        const coord = [curr.lat, curr.lon];
        const prev = batchCoords[batchCoords.length - 1];
        const gap = haversineKm(prev, coord);

        if (gap > MAX_GAP_KM) {
            // Distance gap — flush current batch and start fresh (no connecting line)
            const opacity = activeTheme.trailMinOpacity + (1 - activeTheme.trailMinOpacity) * (i / points.length);
            flushBatch(opacity);
            batchCoords = [coord];
            batchColor = altColor(curr.altitude_ft);
            batchAltBand = currAltBand;
        } else if (currAltBand === batchAltBand) {
            batchCoords.push(coord);
        } else {
            const opacity = activeTheme.trailMinOpacity + (1 - activeTheme.trailMinOpacity) * (i / points.length);
            flushBatch(opacity);
            batchCoords = [batchCoords[batchCoords.length - 1], coord];
            batchColor = altColor(curr.altitude_ft);
            batchAltBand = currAltBand;
        }
    }
    flushBatch(activeTheme.trailMinOpacity + (1 - activeTheme.trailMinOpacity));
}

// --- Dynamic map center from receiver ---
function centerOnReceiver() {
    if (mapCentered) return;
    fetch('/api/stats')
        .then(r => r.json())
        .then(data => {
            if (data.receiver && data.receiver.lat && data.receiver.lon) {
                map.setView([data.receiver.lat, data.receiver.lon], 8);
                mapCentered = true;
                L.circleMarker([data.receiver.lat, data.receiver.lon], {
                    radius: 6, color: '#00aaff', fillColor: '#00aaff',
                    fillOpacity: 0.8, weight: 1,
                }).addTo(map).bindTooltip(data.receiver.name || 'Receiver', {
                    permanent: true, direction: 'right',
                    className: 'dark-tooltip',
                });
            }
        })
        .catch(() => {});
}

// --- Stats overlay ---
// Threshold for the stale-feed banner. Live ADS-B traffic at the ridgeline
// site produces frames every few hundred ms; >60s of silence almost always
// means the receiver subprocess died or the network dropped. Server-side
// auto-recovery normally restarts the receiver within ~10s of the crash
// (see Pi `adsb-receiver` crash-on-capture-exit, 2026-05-05), so a banner
// that fires at 60s gives the user-visible signal only when the recovery
// is also stuck — without spamming on routine reconnects.
const FEED_STALE_THRESHOLD_SEC = 60;

function formatFeedAge(seconds) {
    if (seconds < 90) return `${Math.round(seconds)}s`;
    if (seconds < 3600) return `${Math.round(seconds / 60)}m`;
    return `${Math.floor(seconds / 3600)}h ${Math.round((seconds % 3600) / 60)}m`;
}

function applyFeedFreshness(feedAgeSeconds) {
    const banner = document.getElementById('feed-stale-banner');
    if (!banner) return;
    if (feedAgeSeconds == null || feedAgeSeconds < FEED_STALE_THRESHOLD_SEC) {
        banner.classList.remove('visible');
    } else {
        document.getElementById('feed-stale-age').textContent = formatFeedAge(feedAgeSeconds);
        banner.classList.add('visible');
    }
}

function updateStats() {
    fetch('/api/stats')
        .then(r => r.json())
        .then(s => {
            document.getElementById('so-positions').textContent = (s.positions || 0).toLocaleString();
            document.getElementById('so-events').textContent = s.events || 0;
            if (s.capture_start) {
                const elapsed = Date.now() / 1000 - s.capture_start;
                const h = Math.floor(elapsed / 3600);
                const m = Math.floor((elapsed % 3600) / 60);
                document.getElementById('so-uptime').textContent = h > 0 ? `${h}h ${m}m` : `${m}m`;
            }
            applyFeedFreshness(s.feed_age_seconds);
        })
        .catch(() => {});
}

// --- Cached trail data (refreshed separately from positions) ---
let cachedTrails = {};
let trailFetchPending = false;

function updateTrails() {
    if (trailFetchPending) return;
    trailFetchPending = true;
    fetch(`/api/trails?minutes=${trailMinutes}`)
        .then(r => r.json())
        .then(data => {
            cachedTrails = data.trails || data;
            trailFetchPending = false;
            // Redraw trails for current markers
            Object.keys(markers).forEach(key => drawTrail(key, cachedTrails[key] || []));
        })
        .catch(err => { trailFetchPending = false; console.error('Trail fetch failed:', err); });
}

// --- Main update loop (positions only — lightweight) ---
function updateMap() {
    fetch(`/api/positions?minutes=${trailMinutes}`).then(r => r.json()).then(applyPositions)
        .catch(err => console.error('Position fetch failed:', err));
}

function applyPositions(posData) {
    const trailData = cachedTrails;
    if (!Array.isArray(posData)) return;
    {
        document.getElementById('ac-count').textContent = posData.length;
        document.getElementById('so-aircraft').textContent = posData.length;
        const tbody = document.querySelector('#ac-table tbody');
        tbody.innerHTML = '';

        const seen = new Set();
        posData.forEach(p => {
            const key = p.icao;
            seen.add(key);

            const acType = classifyAircraft(p);
            if (markers[key]) {
                markers[key].setLatLng([p.lat, p.lon]);
                markers[key].setIcon(acIcon(p.heading_deg, p.is_military, acType));
                markers[key].setPopupContent(popupHtml(p, acType));
                markers[key].setOpacity(1.0);
            } else {
                markers[key] = L.marker([p.lat, p.lon], {
                    icon: acIcon(p.heading_deg, p.is_military, acType),
                }).addTo(map);
                markers[key].bindPopup(popupHtml(p, acType), { maxWidth: 280 });
                markers[key].on('popupopen', () => loadAircraftPhoto(key));
            }

            const label = p.callsign || p.registration || p.icao;
            markers[key].bindTooltip(label, {
                permanent: false,
                direction: 'right',
                className: 'dark-tooltip',
            });

            const cls = p.is_military ? 'mil' : '';
            const hdg = p.heading_deg != null ? Math.round(p.heading_deg) + '\u00B0' : '-';
            const anomalyCls = anomalyClass(p.anomaly_score);
            const rowCls = anomalyCls ? `ac-anomaly-${anomalyCls}` : '';
            const anomalyMark = anomalyCls
                ? `<span class="ac-anomaly-mark ac-anomaly-${anomalyCls}" title="anomaly score ${p.anomaly_score.toFixed(2)}">!</span>`
                : '';
            tbody.innerHTML += `<tr class="${rowCls}">
                <td><a href="/aircraft/${encodeURIComponent(p.icao)}" class="${cls}">${esc(p.icao)}</a>${anomalyMark}</td>
                <td>${esc(p.callsign || p.registration || '-')}</td>
                <td>${p.altitude_ft != null ? p.altitude_ft.toLocaleString() : '-'}</td>
                <td>${p.speed_kts ? Math.round(p.speed_kts) : '-'}</td>
                <td>${hdg}</td>
            </tr>`;
        });

        // Historical aircraft — only show at longer trail windows (>= 1h)
        if (trailMinutes >= 60) Object.keys(trailData).forEach(icao => {
            if (seen.has(icao)) return;
            const trail = trailData[icao];
            if (!trail || trail.length === 0) return;
            const last = trail[trail.length - 1];
            if (!last.lat || !last.lon) return;

            seen.add(icao);
            drawTrail(icao, trail);

            // Compute heading from last two trail points
            let heading = null;
            if (trail.length >= 2) {
                const prev = trail[trail.length - 2];
                const dLon = (last.lon - prev.lon) * Math.PI / 180;
                const lat1 = prev.lat * Math.PI / 180;
                const lat2 = last.lat * Math.PI / 180;
                const y = Math.sin(dLon) * Math.cos(lat2);
                const x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(dLon);
                heading = ((Math.atan2(y, x) * 180 / Math.PI) + 360) % 360;
            }

            const ghostP = { icao, lat: last.lat, lon: last.lon, altitude_ft: last.altitude_ft, speed_kts: last.speed_kts, heading_deg: heading, is_military: false };
            const acType = classifyAircraft(ghostP);

            if (markers[icao]) {
                markers[icao].setLatLng([last.lat, last.lon]);
                markers[icao].setIcon(acIcon(heading, false, acType));
                markers[icao].setPopupContent(popupHtml(ghostP, acType));
                markers[icao].setOpacity(0.35);
            } else {
                markers[icao] = L.marker([last.lat, last.lon], {
                    icon: acIcon(heading, false, acType),
                    opacity: 0.35,
                }).addTo(map);
                markers[icao].bindPopup(popupHtml(ghostP, acType), { maxWidth: 250 });
            }

            const minutesAgo = last.timestamp ? Math.round((Date.now() / 1000 - last.timestamp) / 60) : null;
            const agoText = minutesAgo != null ? `${minutesAgo}m ago` : '';
            markers[icao].bindTooltip(icao + (agoText ? ` (${agoText})` : ''), { permanent: false, direction: 'right', className: 'dark-tooltip' });

            const hdg = last.heading_deg != null ? Math.round(last.heading_deg) + '\u00B0' : '-';
            tbody.innerHTML += `<tr class="ac-ghost">
                <td><a href="/aircraft/${encodeURIComponent(icao)}">${esc(icao)}</a></td>
                <td>${agoText || '-'}</td>
                <td>${last.altitude_ft != null ? last.altitude_ft.toLocaleString() : '-'}</td>
                <td>${last.speed_kts ? Math.round(last.speed_kts) : '-'}</td>
                <td>${hdg}</td>
            </tr>`;
        });

        // Update aircraft count to include historical
        document.getElementById('ac-count').textContent = seen.size;
        document.getElementById('so-aircraft').textContent = seen.size;

        // Update route predictions
        drawPredictions(posData);

        // Update military highlight rings
        if (militaryHighlightEnabled) {
            posData.forEach(p => {
                const key = p.icao;
                if (p.is_military && p.lat && p.lon) {
                    if (!militaryRings[key]) {
                        militaryRings[key] = L.circleMarker([p.lat, p.lon], {
                            radius: 18, color: '#ff4444', fillColor: '#ff4444',
                            fillOpacity: 0.12, weight: 2, dashArray: '4,4',
                        }).addTo(militaryLayer);
                    } else {
                        militaryRings[key].setLatLng([p.lat, p.lon]);
                    }
                }
            });
        }

        Object.keys(markers).forEach(key => {
            if (!seen.has(key)) {
                map.removeLayer(markers[key]);
                delete markers[key];
                if (trailLines[key]) {
                    trailLines[key].forEach(seg => map.removeLayer(seg));
                    delete trailLines[key];
                }
                if (militaryRings[key]) {
                    militaryLayer.removeLayer(militaryRings[key]);
                    delete militaryRings[key];
                }
            }
        });
    }
}

// --- Airport layer ---
let airportLayer = L.layerGroup().addTo(map);
let allAirports = null;

const aptIcons = {
    large_airport: L.divIcon({ className: '', html: `<svg width="18" height="18" viewBox="0 0 18 18"><rect x="3" y="8" width="12" height="2.5" rx="1" fill="#ffaa00" opacity="0.9"/><rect x="7.5" y="2" width="2.5" height="14" rx="1" fill="#ffaa00" opacity="0.9"/></svg>`, iconSize: [18, 18], iconAnchor: [9, 9] }),
    medium_airport: L.divIcon({ className: '', html: `<svg width="14" height="14" viewBox="0 0 14 14"><rect x="2" y="6" width="10" height="2" rx="1" fill="#cc8800" opacity="0.8"/><rect x="6" y="1" width="2" height="12" rx="1" fill="#cc8800" opacity="0.8"/></svg>`, iconSize: [14, 14], iconAnchor: [7, 7] }),
    small_airport: L.divIcon({ className: '', html: `<svg width="10" height="10" viewBox="0 0 10 10"><circle cx="5" cy="5" r="3" fill="#997700" opacity="0.7"/></svg>`, iconSize: [10, 10], iconAnchor: [5, 5] }),
    major: L.divIcon({ className: '', html: `<svg width="18" height="18" viewBox="0 0 18 18"><rect x="3" y="8" width="12" height="2.5" rx="1" fill="#ffaa00" opacity="0.9"/><rect x="7.5" y="2" width="2.5" height="14" rx="1" fill="#ffaa00" opacity="0.9"/></svg>`, iconSize: [18, 18], iconAnchor: [9, 9] }),
    medium: L.divIcon({ className: '', html: `<svg width="14" height="14" viewBox="0 0 14 14"><rect x="2" y="6" width="10" height="2" rx="1" fill="#cc8800" opacity="0.8"/><rect x="6" y="1" width="2" height="12" rx="1" fill="#cc8800" opacity="0.8"/></svg>`, iconSize: [14, 14], iconAnchor: [7, 7] }),
    small: L.divIcon({ className: '', html: `<svg width="10" height="10" viewBox="0 0 10 10"><circle cx="5" cy="5" r="3" fill="#997700" opacity="0.7"/></svg>`, iconSize: [10, 10], iconAnchor: [5, 5] }),
};

function getAptFilters() {
    return {
        large_airport: document.getElementById('apt-large').checked,
        medium_airport: document.getElementById('apt-medium').checked,
        small_airport: document.getElementById('apt-small').checked,
        major: document.getElementById('apt-large').checked,
        medium: document.getElementById('apt-medium').checked,
        small: document.getElementById('apt-small').checked,
    };
}

['apt-large', 'apt-medium', 'apt-small'].forEach(id => {
    document.getElementById(id).addEventListener('change', () => {
        if (!allAirports) {
            fetch('/api/airports').then(r => r.json()).then(data => {
                allAirports = data;
                renderVisibleAirports();
            });
        } else {
            renderVisibleAirports();
        }
    });
});

const aptSvgStrings = {
    large_airport: '<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18"><rect x="3" y="8" width="12" height="2.5" rx="1" fill="#ffaa00" opacity="0.9"/><rect x="7.5" y="2" width="2.5" height="14" rx="1" fill="#ffaa00" opacity="0.9"/></svg>',
    medium_airport: '<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 14 14"><rect x="2" y="6" width="10" height="2" rx="1" fill="#cc8800" opacity="0.8"/><rect x="6" y="1" width="2" height="12" rx="1" fill="#cc8800" opacity="0.8"/></svg>',
    small_airport: '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><circle cx="5" cy="5" r="3" fill="#997700" opacity="0.7"/></svg>',
};
aptSvgStrings.major = aptSvgStrings.large_airport;
aptSvgStrings.medium = aptSvgStrings.medium_airport;
aptSvgStrings.small = aptSvgStrings.small_airport;

function clearCesiumAirports() {
    cesiumAirportEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumAirportEntities = [];
}

function renderVisibleAirports() {
    if (is3DMode) {
        clearCesiumAirports();
        if (!allAirports || !cesiumViewer) return;
        const filters = getAptFilters();
        const anyChecked = Object.values(filters).some(v => v);
        if (!anyChecked) return;
        allAirports.forEach(apt => {
            if (!filters[apt.type]) return;
            const svgStr = aptSvgStrings[apt.type] || aptSvgStrings.small_airport;
            const typeLabel = apt.type === 'large_airport' || apt.type === 'major' ? 'Major' : apt.type === 'medium_airport' || apt.type === 'medium' ? 'Medium' : 'Small';
            const scale = apt.type === 'large_airport' || apt.type === 'major' ? 0.9 : apt.type === 'medium_airport' || apt.type === 'medium' ? 0.7 : 0.5;
            const entity = cesiumViewer.entities.add({
                position: Cesium.Cartesian3.fromDegrees(apt.lon, apt.lat, 0),
                billboard: {
                    image: 'data:image/svg+xml,' + encodeURIComponent(svgStr),
                    scale: scale,
                    verticalOrigin: Cesium.VerticalOrigin.CENTER,
                    heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                    disableDepthTestDistance: Number.POSITIVE_INFINITY,
                },
                label: {
                    text: apt.icao,
                    font: '10px monospace',
                    fillColor: Cesium.Color.fromCssColorString('#ffaa00'),
                    outlineColor: Cesium.Color.BLACK,
                    outlineWidth: 2,
                    style: Cesium.LabelStyle.FILL_AND_OUTLINE,
                    pixelOffset: new Cesium.Cartesian2(0, -14),
                    scale: 0.8,
                    distanceDisplayCondition: new Cesium.DistanceDisplayCondition(0, 200000),
                    disableDepthTestDistance: Number.POSITIVE_INFINITY,
                },
                description: `<div style="font:12px monospace;">
                    <b style="color:#ffaa00;">${apt.icao}</b><br>
                    ${apt.name}<br>Type: ${typeLabel}<br>
                    Elev: ${apt.elevation_ft.toLocaleString()} ft
                </div>`,
            });
            cesiumAirportEntities.push(entity);
        });
        cesiumViewer.scene.requestRender();
        return;
    }

    airportLayer.clearLayers();
    if (!allAirports) return;
    const filters = getAptFilters();
    const anyChecked = Object.values(filters).some(v => v);
    if (!anyChecked) return;
    const bounds = map.getBounds();
    allAirports.forEach(apt => {
        if (!filters[apt.type]) return;
        if (!bounds.contains([apt.lat, apt.lon])) return;
        const icon = aptIcons[apt.type] || aptIcons.small_airport;
        const typeLabel = apt.type === 'large_airport' || apt.type === 'major' ? 'Major' : apt.type === 'medium_airport' || apt.type === 'medium' ? 'Medium' : 'Small';
        const popupContent = `<div class="ac-popup">
            <div class="popup-title" style="color:#ffaa00;">${apt.icao}</div>
            <div class="popup-row"><span class="popup-label">Name</span><span class="popup-value">${apt.name}</span></div>
            <div class="popup-row"><span class="popup-label">Type</span><span class="popup-value">${typeLabel}</span></div>
            <div class="popup-row"><span class="popup-label">Elevation</span><span class="popup-value">${apt.elevation_ft.toLocaleString()} ft</span></div>
            <div class="popup-row"><span class="popup-label">Coords</span><span class="popup-value">${apt.lat.toFixed(4)}, ${apt.lon.toFixed(4)}</span></div>
            <div style="margin-top:6px;">
                <a href="https://www.airnav.com/airport/${apt.icao}" target="_blank">AirNav &rarr;</a>
                &nbsp;
                <a href="https://skyvector.com/airport/${apt.icao}" target="_blank">SkyVector &rarr;</a>
            </div>
        </div>`;
        L.marker([apt.lat, apt.lon], { icon, interactive: true })
            .bindTooltip(`${apt.icao} \u2014 ${apt.name}`, { permanent: false, direction: 'right', className: 'dark-tooltip' })
            .bindPopup(popupContent, { maxWidth: 280 })
            .addTo(airportLayer);
    });
}

map.on('moveend', renderVisibleAirports);

// --- Receiver layer ---
let receiverLayer = L.layerGroup().addTo(map);
let receiverData = null;

const receiverIcon = L.divIcon({
    className: '',
    html: `<svg width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="#00ff88" stroke-width="2" opacity="0.9"/><circle cx="8" cy="8" r="2.5" fill="#00ff88" opacity="0.9"/></svg>`,
    iconSize: [16, 16],
    iconAnchor: [8, 8]
});

const receiverIconOffline = L.divIcon({
    className: '',
    html: `<svg width="16" height="16" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6" fill="none" stroke="#666" stroke-width="2" opacity="0.7"/><circle cx="8" cy="8" r="2.5" fill="#666" opacity="0.7"/></svg>`,
    iconSize: [16, 16],
    iconAnchor: [8, 8]
});

function formatUptime(sec) {
    if (!sec || sec <= 0) return 'offline';
    if (sec < 3600) return Math.floor(sec / 60) + 'm';
    if (sec < 86400) return Math.floor(sec / 3600) + 'h ' + Math.floor((sec % 3600) / 60) + 'm';
    return Math.floor(sec / 86400) + 'd ' + Math.floor((sec % 86400) / 3600) + 'h';
}

function renderReceivers() {
    receiverLayer.clearLayers();
    if (!receiverData || !document.getElementById('receiver-toggle').checked) return;
    receiverData.forEach(rx => {
        if (rx.lat == null || rx.lon == null) return;
        const icon = rx.online ? receiverIcon : receiverIconOffline;
        const statusColor = rx.online ? '#00ff88' : '#ff4444';
        const statusText = rx.online ? 'Online' : 'Offline';
        const popupContent = `<div class="ac-popup">
            <div class="popup-title" style="color:#00ff88;">${esc(rx.name || 'Receiver')}</div>
            <div class="popup-row"><span class="popup-label">Status</span><span class="popup-value" style="color:${statusColor};">${statusText}</span></div>
            <div class="popup-row"><span class="popup-label">Uptime</span><span class="popup-value">${formatUptime(rx.uptime_sec)}</span></div>
            <div class="popup-row"><span class="popup-label">Frames</span><span class="popup-value">${(rx.frames_captured || 0).toLocaleString()}</span></div>
            <div class="popup-row"><span class="popup-label">Aircraft</span><span class="popup-value">${rx.active_aircraft || 0}</span></div>
            <div class="popup-row"><span class="popup-label">Coords</span><span class="popup-value">${rx.lat.toFixed(4)}, ${rx.lon.toFixed(4)}</span></div>
        </div>`;
        L.marker([rx.lat, rx.lon], { icon, interactive: true })
            .bindTooltip(esc(rx.name || 'Receiver') + (rx.online ? '' : ' (offline)'), { permanent: false, direction: 'right', className: 'dark-tooltip' })
            .bindPopup(popupContent, { maxWidth: 280 })
            .addTo(receiverLayer);
    });
}

document.getElementById('receiver-toggle').addEventListener('change', () => {
    if (document.getElementById('receiver-toggle').checked && !receiverData) {
        fetch('/api/v1/receivers').then(r => r.json()).then(data => {
            receiverData = data;
            renderReceivers();
        });
    } else {
        renderReceivers();
    }
});

// Refresh receiver data every 30 seconds if the toggle is on
setInterval(() => {
    if (document.getElementById('receiver-toggle').checked) {
        fetch('/api/v1/receivers').then(r => r.json()).then(data => {
            receiverData = data;
            renderReceivers();
        }).catch(() => {});
    }
}, 30000);

// --- Splatlas layer ---
// Pinned 3DGS scenes that pair with Splatlas (splatlas.blueoctopustechnology.com).
// Click a pin → opens the viewer at that scene with the canonical "first ref point"
// vantage. Lit hexes on the Splatlas dome show ADS-B traffic overhead from that
// real-world location, in real time.
//
// Scene list is hardcoded here for v1. When >3 scenes exist this should move to
// /api/v1/splatlas/scenes.
const SPLATLAS_BASE_URL = 'https://splatlas.blueoctopustechnology.com';
const SPLATLAS_SCENES = [
    {
        id: 'parkerMeadows',
        name: 'Parker Meadows',
        location: 'Franklin, NC',
        lat: 35.15326,
        lon: -83.45595,
        captured: '2026-05-21',
        description: 'Ballfield complex — pavilion tower as observer point. Watch live aircraft fly overhead the real captured scene.',
        // Scene perimeter in lat/lon, clockwise from NW. Generated from
        // Splatlas manifest precomputed_bbox via splatlas.getScenePerimeter().
        // Re-run that helper to refresh after retraining.
        polygon_latlon: [
            [35.15438478853976, -83.45693942913589], // NW
            [35.15438478853976, -83.45496057086412], // NE
            [35.15213521146025, -83.45496057086412], // SE
            [35.15213521146025, -83.45693942913589], // SW
        ],
        observation_points: [
            { id: 'pavilion-top', name: 'Top of the pavilion tower' },
            { id: 'complex-overhead', name: 'Overhead — full complex' },
            { id: 'pavilion-aerial', name: 'Pavilion aerial' },
            { id: 'diamond-3-4', name: 'Approach to the SE diamond' },
        ],
    },
    {
        id: 'cyberTruck',
        kind: 'receiver',
        name: 'Cybertruck',
        location: 'Franklin, NC',
        lat: 35.18,
        lon: -83.38,
        captured: '2026-05-19',
        description: 'The ADS-B receiver site itself — the antenna feeding this whole map sits right here, captured as a 3D scan. This pin marks the signal source.',
        observation_points: [
            { id: '', name: 'Enter the Cybertruck scan' },
        ],
    },
];

let splatlasLayer = L.layerGroup().addTo(map);

// Splatlas marker — pin shape with a dome glyph inside + two staggered
// pulse rings (CSS) to signal "this is an active capture site, not a
// regular receiver pin." Reads as premium / atlas-tier.
const splatlasIcon = L.divIcon({
    className: 'splatlas-icon',
    html: `<div class="splatlas-icon-pulse"></div>
           <div class="splatlas-icon-pulse delay-1"></div>
           <svg width="26" height="32" viewBox="0 0 26 32" style="position:relative;display:block;">
             <path d="M13 0 C6 0 0 6 0 13 C0 20 13 32 13 32 C13 32 26 20 26 13 C26 6 20 0 13 0 Z"
                   fill="#ff914d" opacity="0.95" stroke="#0a0a0a" stroke-width="1"/>
             <!-- dome glyph: hemisphere arc + observer dot -->
             <path d="M5 14 Q13 4 21 14" stroke="#0a0a0a" stroke-width="1.5" fill="none"/>
             <circle cx="13" cy="14" r="1.8" fill="#0a0a0a"/>
             <!-- two tick marks reading as 'sensor rays' -->
             <line x1="13" y1="14" x2="7" y2="9"  stroke="#0a0a0a" stroke-width="1"/>
             <line x1="13" y1="14" x2="19" y2="9" stroke="#0a0a0a" stroke-width="1"/>
           </svg>`,
    iconSize: [26, 32],
    iconAnchor: [13, 32],
});

// Receiver-site marker — distinct from a captured-scene pin: cyan (vs copper)
// with an antenna-mast + broadcast-arc glyph. Reads as "this is the signal
// source," not "this is a dome scene."
const receiverSiteIcon = L.divIcon({
    className: 'splatlas-icon',
    html: `<svg width="26" height="32" viewBox="0 0 26 32" style="position:relative;display:block;">
             <path d="M13 0 C6 0 0 6 0 13 C0 20 13 32 13 32 C13 32 26 20 26 13 C26 6 20 0 13 0 Z"
                   fill="#4fc3f7" opacity="0.95" stroke="#0a0a0a" stroke-width="1"/>
             <line x1="13" y1="18" x2="13" y2="10" stroke="#0a0a0a" stroke-width="1.6"/>
             <circle cx="13" cy="9.5" r="1.7" fill="#0a0a0a"/>
             <path d="M9.5 13 Q13 8.5 16.5 13" stroke="#0a0a0a" stroke-width="1.2" fill="none"/>
             <path d="M7 15 Q13 6 19 15" stroke="#0a0a0a" stroke-width="1" fill="none" opacity="0.65"/>
           </svg>`,
    iconSize: [26, 32],
    iconAnchor: [13, 32],
});

function renderSplatlasScenes() {
    splatlasLayer.clearLayers();
    if (!document.getElementById('splatlas-toggle').checked) return;
    SPLATLAS_SCENES.forEach(scene => {
        if (scene.lat == null || scene.lon == null) return;
        const vantageButtons = scene.observation_points.map(p =>
            `<a href="${SPLATLAS_BASE_URL}/?scene=${encodeURIComponent(scene.id)}&vantage=${encodeURIComponent(p.id)}" target="_blank" rel="noopener">→ ${esc(p.name)}</a>`
        ).join('');
        const popupContent = `<div class="splatlas-popup" style="min-width:240px;">
            <div class="splatlas-popup-eyebrow">${scene.kind === 'receiver' ? 'Splatlas · Receiver Site' : 'Splatlas · Captured Scene'}</div>
            <div class="splatlas-popup-title">${esc(scene.name)}</div>
            <div class="popup-row"><span class="popup-label">Location</span><span class="popup-value">${esc(scene.location)}</span></div>
            <div class="popup-row"><span class="popup-label">Captured</span><span class="popup-value">${esc(scene.captured)}</span></div>
            <div style="font-size:11px;color:#aaa;margin:8px 0 4px;line-height:1.4;">${esc(scene.description)}</div>
            <div class="splatlas-popup-actions" style="margin-top:8px;">
                <div style="font-size:9px;color:#888;text-transform:uppercase;letter-spacing:0.18em;margin-bottom:4px;">Enter from</div>
                ${vantageButtons}
            </div>
            <div class="splatlas-popup-footer">
                3D Gaussian Splat + live ADS-B + line-of-sight occlusion ·
                <a href="${SPLATLAS_BASE_URL}" target="_blank" rel="noopener">splatlas.blueoctopustechnology.com</a>
            </div>
        </div>`;
        // Observation dome — three concentric copper rings reading as a
        // hemisphere from above, not just a flat ring. Outer = max
        // detection radius (~150 km), middle = primary coverage,
        // inner = high-confidence zone close to the watch point.
        // Filled at successively higher opacity so visually it reads
        // "denser at the center, fading outward" — dome-from-above.
        // Receiver pins mark a point antenna (the signal source) — no big
        // coverage rings, which would overlap messily with a nearby scene dome.
        if (scene.kind !== 'receiver') {
        const domeRadiusM = (scene.dome_radius_km || 150) * 1000;
        L.circle([scene.lat, scene.lon], {
            radius: domeRadiusM,
            color: '#ff914d', weight: 1.5, opacity: 0.5,
            dashArray: '2 5',
            fillColor: '#ff914d', fillOpacity: 0.04,
            interactive: false,
        }).addTo(splatlasLayer);
        L.circle([scene.lat, scene.lon], {
            radius: domeRadiusM * 0.66,
            color: '#ff914d', weight: 1, opacity: 0.4,
            dashArray: '2 4',
            fillColor: '#ff914d', fillOpacity: 0.06,
            interactive: false,
        }).addTo(splatlasLayer);
        L.circle([scene.lat, scene.lon], {
            radius: domeRadiusM * 0.33,
            color: '#ff914d', weight: 1, opacity: 0.35,
            dashArray: '2 4',
            fillColor: '#ff914d', fillOpacity: 0.09,
            interactive: false,
        }).addTo(splatlasLayer);
        }
        // Draw the scene perimeter polygon NEXT so the marker sits on top.
        // Dashed copper outline + light fill — atlas register, doesn't fight
        // other map layers but clearly demarcates the captured envelope.
        if (scene.polygon_latlon && scene.polygon_latlon.length >= 3) {
            L.polygon(scene.polygon_latlon, {
                color: '#ff914d',
                weight: 2,
                opacity: 0.95,
                dashArray: '6 4',
                fillColor: '#ff914d',
                fillOpacity: 0.08,
                interactive: false,
            }).addTo(splatlasLayer);
        }
        L.marker([scene.lat, scene.lon], { icon: scene.kind === 'receiver' ? receiverSiteIcon : splatlasIcon, interactive: true })
            .bindTooltip(esc(scene.name) + ' · Splatlas', { permanent: false, direction: 'right', className: 'dark-tooltip' })
            .bindPopup(popupContent, { maxWidth: 300, className: 'splatlas-popup-wrapper' })
            .addTo(splatlasLayer);
    });
}

document.getElementById('splatlas-toggle').addEventListener('change', renderSplatlasScenes);
renderSplatlasScenes();

// --- Satellite layer ---
// Renders satellite ground-track positions on the map using TLE data
// from /api/v1/tle/:group + SGP4 propagation via satellite.js (loaded
// in map.html). Same data source Splatlas uses; one canonical feeds
// service feeds both surfaces.
//
// Performance budget: TLE refresh every 6h, position propagation every
// 5s. Default group: starlink (200 active satellites).
const SAT_GROUP = 'starlink';
const SAT_REFRESH_MS = 6 * 60 * 60 * 1000;
const SAT_TICK_MS = 5000;
let satLayer = L.layerGroup().addTo(map);
let satEnabled = false;
let satrecs = []; // [{ name, satrec }]
let satTickTimer = null;
let satRefreshTimer = null;

const satIcon = L.divIcon({
    className: 'sat-icon',
    html: `<svg width="14" height="14" viewBox="0 0 14 14">
        <circle cx="7" cy="7" r="4" fill="#b89aff" stroke="#0a0a0a" stroke-width="1"/>
        <circle cx="7" cy="7" r="1.5" fill="#0a0a0a"/>
    </svg>`,
    iconSize: [14, 14],
    iconAnchor: [7, 7],
});

async function fetchSatelliteTles() {
    try {
        const res = await fetch(`/api/v1/tle/${SAT_GROUP}`, { cache: 'no-store' });
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        const text = await res.text();
        const lines = text.split(/\r?\n/);
        const parsed = [];
        for (let i = 0; i + 2 < lines.length; i++) {
            const name = lines[i];
            const tle1 = lines[i + 1];
            const tle2 = lines[i + 2];
            if (tle1?.startsWith('1 ') && tle2?.startsWith('2 ')) {
                try {
                    const rec = satellite.twoline2satrec(tle1, tle2);
                    if (!rec.error) parsed.push({ name: name.trim(), satrec: rec });
                } catch (_) {}
                i += 2;
            }
        }
        satrecs = parsed;
        console.log(`[sat] loaded ${parsed.length} TLEs`);
    } catch (e) {
        console.warn(`[sat] TLE fetch failed: ${e.message}`);
    }
}

// Latest satellite positions snapshot (lat/lon/alt) for the right-panel
// table. Re-computed every propagation tick.
let satSnapshot = [];

function propagateSatellites() {
    if (!satEnabled || satrecs.length === 0) return;
    satLayer.clearLayers();
    const now = new Date();
    const gmst = satellite.gstime(now);
    // Only propagate/render sats currently in the map view. Starlink alone is
    // ~6,000 birds worldwide; rendering all of them buried the map and the
    // table. Filtering to the viewport keeps it to the sats actually overhead
    // (e.g. "over the USA" when you're looking at the USA). moveend re-runs this.
    const bounds = map.getBounds();
    satSnapshot = [];
    for (const { name, satrec } of satrecs) {
        const pv = satellite.propagate(satrec, now);
        if (!pv?.position) continue;
        const gd = satellite.eciToGeodetic(pv.position, gmst);
        const lat = satellite.degreesLat(gd.latitude);
        const lon = satellite.degreesLong(gd.longitude);
        if (!bounds.contains([lat, lon])) continue;
        const altKm = gd.height;
        const marker = L.marker([lat, lon], { icon: satIcon, interactive: true });
        marker.bindTooltip(`🛰 ${esc(name)}<br>${Math.round(altKm)} km`, {
            className: 'dark-tooltip', direction: 'right',
        });
        marker.addTo(satLayer);
        satSnapshot.push({ name, lat, lon, altKm });
    }
    renderSatTable();
    if (is3DMode) renderCesiumSats();
}

// 3D satellites — the 2D layer above is a hidden Leaflet layer in 3D mode, so
// sats were invisible on the Cesium globe. Render satSnapshot (already
// viewport-filtered) as points at true altitude. Clear+rebuild each tick;
// after filtering the count is small.
let cesiumSatEntities = [];
function clearCesiumSats() {
    if (!cesiumViewer) return;
    cesiumSatEntities.forEach(e => cesiumViewer.entities.remove(e));
    cesiumSatEntities = [];
}
function renderCesiumSats() {
    if (!is3DMode || !cesiumViewer) return;
    clearCesiumSats();
    satSnapshot.forEach(s => {
        cesiumSatEntities.push(cesiumViewer.entities.add({
            position: Cesium.Cartesian3.fromDegrees(s.lon, s.lat, s.altKm * 1000),
            point: {
                pixelSize: 5,
                color: Cesium.Color.fromCssColorString('#b89aff'),
                outlineColor: Cesium.Color.fromCssColorString('#1a0a2a'),
                outlineWidth: 1,
                disableDepthTestDistance: Number.POSITIVE_INFINITY,
            },
            description: `<div style="font:12px monospace;">🛰 ${esc(s.name)}<br>${Math.round(s.altKm)} km</div>`,
        }));
    });
}

document.getElementById('sat-toggle')?.addEventListener('change', async (e) => {
    satEnabled = e.target.checked;
    if (satEnabled) {
        if (satrecs.length === 0) await fetchSatelliteTles();
        propagateSatellites();
        satTickTimer = setInterval(propagateSatellites, SAT_TICK_MS);
        satRefreshTimer = setInterval(fetchSatelliteTles, SAT_REFRESH_MS);
    } else {
        satLayer.clearLayers();
        clearCesiumSats();
        clearInterval(satTickTimer);
        clearInterval(satRefreshTimer);
        satTickTimer = null;
        satRefreshTimer = null;
    }
});
// Re-propagate when the map view changes so sats outside the new bounds drop.
map.on('moveend', () => { if (satEnabled) propagateSatellites(); });

// Render the satellite table on the right panel (closest-overhead sorted)
function renderSatTable() {
    const tbody = document.querySelector('#sat-table tbody');
    const countEl = document.getElementById('sat-count-tab');
    if (!tbody) return;
    // Sort by altitude (lowest first) — closer to overhead first
    const sorted = [...satSnapshot].sort((a, b) => a.altKm - b.altKm);
    tbody.innerHTML = sorted.map((s) =>
        `<tr><td>${esc(s.name)}</td><td>${Math.round(s.altKm)}</td><td>${s.lat.toFixed(2)}</td><td>${s.lon.toFixed(2)}</td></tr>`
    ).join('');
    if (countEl) countEl.textContent = satSnapshot.length;
}

// Right-panel tab switching: Aircraft / Sats
document.querySelectorAll('.list-tab').forEach((tab) => {
    tab.addEventListener('click', () => {
        const target = tab.dataset.tab;
        document.querySelectorAll('.list-tab').forEach((t) => t.classList.toggle('active', t === tab));
        document.querySelectorAll('.list-table').forEach((tbl) => {
            tbl.classList.toggle('hidden', tbl.dataset.tabTarget !== target);
        });
    });
});

// --- Heatmap layer ---
let heatLayer = null;
let heatmapEnabled = false;
const heatDurSteps = [15, 30, 60, 120, 360, 720, 1440, 10080];
const heatDurLabels = ['15m', '30m', '1h', '2h', '6h', '12h', '24h', '7d'];
let heatMinutes = 1440;
const heatDurSlider = document.getElementById('heatmap-dur-slider');
const heatDurLabel = document.getElementById('heatmap-dur-label');
const heatDurRow = document.getElementById('heatmap-dur-row');

function clearCesiumHeatmap() {
    cesiumHeatmapEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumHeatmapEntities = [];
}

document.getElementById('heatmap-toggle').addEventListener('change', function() {
    heatmapEnabled = this.checked;
    heatDurRow.style.display = heatmapEnabled ? '' : 'none';
    if (!heatmapEnabled) {
        if (heatLayer) { map.removeLayer(heatLayer); heatLayer = null; }
        clearCesiumHeatmap();
    }
    if (heatmapEnabled) updateHeatmap();
});

heatDurSlider.addEventListener('input', function() {
    const idx = parseInt(this.value);
    heatMinutes = heatDurSteps[idx];
    heatDurLabel.textContent = heatDurLabels[idx];
    saveState();
    updateHeatmap();
});

function heatColor3D(t) {
    if (t < 0.15) return new Cesium.Color(0.0, 0.2, 0.67, 0.4);
    if (t < 0.3) return new Cesium.Color(0.0, 0.53, 1.0, 0.45);
    if (t < 0.5) return new Cesium.Color(0.0, 1.0, 0.53, 0.5);
    if (t < 0.7) return new Cesium.Color(1.0, 1.0, 0.0, 0.55);
    if (t < 0.85) return new Cesium.Color(1.0, 0.53, 0.0, 0.6);
    return new Cesium.Color(1.0, 0.0, 0.0, 0.65);
}

function updateHeatmap() {
    if (!heatmapEnabled) return;

    if (is3DMode && cesiumViewer) {
        const camHeight = cesiumViewer.camera.positionCartographic.height;
        const resolution = camHeight > 200000 ? 0.05 : camHeight > 50000 ? 0.02 : camHeight > 20000 ? 0.01 : 0.005;
        fetch('/api/heatmap?minutes=' + heatMinutes + '&resolution=' + resolution)
            .then(r => r.json())
            .then(data => {
                clearCesiumHeatmap();
                if (data.length === 0) return;
                const maxCount = Math.max(...data.map(c => c.count));
                const halfRes = resolution / 2;
                data.forEach(c => {
                    const norm = Math.max(0.1, c.count / maxCount);
                    const entity = cesiumViewer.entities.add({
                        rectangle: {
                            coordinates: Cesium.Rectangle.fromDegrees(
                                c.lon - halfRes, c.lat - halfRes,
                                c.lon + halfRes, c.lat + halfRes
                            ),
                            material: heatColor3D(norm),
                            heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                        },
                    });
                    cesiumHeatmapEntities.push(entity);
                });
                cesiumViewer.scene.requestRender();
            })
            .catch(e => { console.error('3D heatmap update failed:', e); });
        return;
    }

    const zoom = map.getZoom();
    const resolution = zoom >= 12 ? 0.005 : zoom >= 10 ? 0.01 : zoom >= 8 ? 0.02 : 0.05;
    fetch('/api/heatmap?minutes=' + heatMinutes + '&resolution=' + resolution)
        .then(r => r.json())
        .then(data => {
            if (heatLayer) map.removeLayer(heatLayer);
            if (data.length === 0) return;
            const maxCount = Math.max(...data.map(c => c.count));
            const points = data.map(c => [c.lat, c.lon, Math.max(0.1, c.count / maxCount)]);
            const radius = zoom >= 12 ? 15 : zoom >= 10 ? 20 : zoom >= 8 ? 25 : 30;
            heatLayer = L.heatLayer(points, {
                radius: radius, blur: 12, maxZoom: 17, minOpacity: 0.3,
                gradient: {0.1: '#0033aa', 0.3: '#0088ff', 0.5: '#00ff88', 0.7: '#ffff00', 0.85: '#ff8800', 1.0: '#ff0000'},
            }).addTo(map);
        })
        .catch((e) => { console.error('Heatmap update failed:', e); });
}

// --- Trail toggle ---
let trailsEnabled = true;
const trailToggle = document.getElementById('trail-toggle');
trailToggle.addEventListener('change', function() {
    trailsEnabled = this.checked;
    if (!trailsEnabled) {
        Object.keys(trailLines).forEach(k => {
            trailLines[k].forEach(seg => map.removeLayer(seg));
        });
    }
    saveState();
});

// --- Trail duration slider ---
const trailDurSteps = [5, 15, 30, 60, 120, 360, 720, 1440];
const trailDurLabels = ['5m', '15m', '30m', '1h', '2h', '6h', '12h', '24h'];
let trailMinutes = 5;
const trailDurSlider = document.getElementById('trail-dur-slider');
const trailDurLabel = document.getElementById('trail-dur-label');
trailDurSlider.addEventListener('input', function() {
    const idx = parseInt(this.value);
    trailMinutes = trailDurSteps[idx];
    trailDurLabel.textContent = trailDurLabels[idx];
    saveState();
    updateTrails(); // Immediate trail refresh on slider change
    // Reconnect WebSocket with the new minutes so live pushes match the slider window.
    if (typeof reconnectPositionsWebSocket === 'function') reconnectPositionsWebSocket();
});

// --- Persist state in localStorage ---
const STATE_KEY = 'adsb-map-state';
function saveState() {
    const state = {
        trails: document.getElementById('trail-toggle').checked,
        trailDur: parseInt(trailDurSlider.value),
        heatmap: document.getElementById('heatmap-toggle').checked,
        heatDur: parseInt(heatDurSlider.value),
        aptLarge: document.getElementById('apt-large').checked,
        aptMedium: document.getElementById('apt-medium').checked,
        aptSmall: document.getElementById('apt-small').checked,
        notify: document.getElementById('notify-toggle').checked,
        audio: document.getElementById('audio-toggle').checked,
        events: document.getElementById('events-toggle').checked,
        milHighlight: document.getElementById('military-highlight').checked,
        mapStyle: document.getElementById('map-style').value,
        predict: document.getElementById('predict-toggle').checked,
        predictDur: parseInt(predictDurSlider.value),
        rangeRings: document.getElementById('range-toggle').checked,
        airspace: document.getElementById('airspace-toggle').checked,
        vessels: document.getElementById('vessel-toggle').checked,
        weather: document.getElementById('weather-toggle').checked,
        zoom: map.getZoom(),
        center: [map.getCenter().lat, map.getCenter().lng],
    };
    localStorage.setItem(STATE_KEY, JSON.stringify(state));
}
function restoreState() {
    try {
        const raw = localStorage.getItem(STATE_KEY);
        if (!raw) return;
        const s = JSON.parse(raw);
        if (s.trails !== undefined) { trailToggle.checked = s.trails; trailsEnabled = s.trails; }
        if (s.trailDur !== undefined) {
            trailDurSlider.value = s.trailDur;
            trailMinutes = trailDurSteps[s.trailDur];
            trailDurLabel.textContent = trailDurLabels[s.trailDur];
        }
        if (s.heatmap) {
            document.getElementById('heatmap-toggle').checked = true;
            heatmapEnabled = true;
            heatDurRow.style.display = '';
        }
        if (s.heatDur !== undefined) {
            heatDurSlider.value = s.heatDur;
            heatMinutes = heatDurSteps[s.heatDur];
            heatDurLabel.textContent = heatDurLabels[s.heatDur];
        }
        if (s.aptLarge) document.getElementById('apt-large').checked = true;
        if (s.aptMedium) document.getElementById('apt-medium').checked = true;
        if (s.aptSmall) document.getElementById('apt-small').checked = true;
        if (s.notify && 'Notification' in window && Notification.permission === 'granted') {
            document.getElementById('notify-toggle').checked = true; notificationsEnabled = true;
        }
        if (s.audio) { document.getElementById('audio-toggle').checked = true; audioEnabled = true; }
        if (s.events) {
            document.getElementById('events-toggle').checked = true;
            eventsEnabled = true; eventsLayer.addTo(map);
        }
        if (s.milHighlight) {
            document.getElementById('military-highlight').checked = true;
            militaryHighlightEnabled = true; militaryLayer.addTo(map);
        }
        if (s.mapStyle && tileSets[s.mapStyle]) setMapStyle(s.mapStyle);
        if (s.predict) {
            predictToggle.checked = true; predictEnabled = true;
            predictDurRow.style.display = '';
        }
        if (s.predictDur !== undefined) {
            predictDurSlider.value = s.predictDur;
            predictMinutes = predictDurSteps[s.predictDur];
            predictDurLabel.textContent = predictDurLabels[s.predictDur];
        }
        if (s.rangeRings) {
            document.getElementById('range-toggle').checked = true;
            rangeEnabled = true;
            rangeLayer.addTo(map);
            fetchAndDrawRangeRings();
        }
        if (s.airspace) {
            document.getElementById('airspace-toggle').checked = true;
            airspaceEnabled = true;
            airspaceLayer.addTo(map);
            fetchAirspace();
        }
        if (s.vessels) {
            document.getElementById('vessel-toggle').checked = true;
            vesselsEnabled = true;
            vesselLayer.addTo(map);
            fetchVessels();
        }
        if (s.weather) {
            document.getElementById('weather-toggle').checked = true;
            enableWeather();
        }
        if (s.center && s.zoom) { map.setView(s.center, s.zoom); mapCentered = true; }
        if (s.aptLarge || s.aptMedium || s.aptSmall) {
            if (!allAirports) {
                fetch('/api/airports').then(r => r.json()).then(data => {
                    allAirports = data;
                    renderVisibleAirports();
                });
            }
        }
        if (s.heatmap) updateHeatmap();
    } catch(e) {}
}
map.on('moveend', saveState);
['heatmap-toggle', 'apt-large', 'apt-medium', 'apt-small', 'notify-toggle', 'audio-toggle', 'events-toggle', 'military-highlight', 'predict-toggle', 'range-toggle', 'vessel-toggle'].forEach(id => {
    document.getElementById(id).addEventListener('change', saveState);
});
document.getElementById('map-style').addEventListener('change', function() {
    setMapStyle(this.value);
    saveState();
    updateMap();
});

// --- Geofence management ---
let geofenceCircles = {};
let placingGeofence = false;

document.getElementById('add-geofence-btn').addEventListener('click', function() {
    placingGeofence = !placingGeofence;
    this.style.background = placingGeofence ? '#00ff88' : '#222';
    this.style.color = placingGeofence ? '#000' : '#00ff88';
    map.getContainer().style.cursor = placingGeofence ? 'crosshair' : '';
});

map.on('click', function(e) {
    if (!placingGeofence) return;
    placingGeofence = false;
    const btn = document.getElementById('add-geofence-btn');
    btn.style.background = '#222';
    btn.style.color = '#00ff88';
    map.getContainer().style.cursor = '';

    const name = prompt('Geofence name:', 'Zone ' + (Object.keys(geofenceCircles).length + 1));
    if (!name) return;
    const radiusStr = prompt('Radius (nautical miles):', '10');
    if (!radiusStr) return;
    const radiusNm = parseFloat(radiusStr);
    if (isNaN(radiusNm) || radiusNm <= 0) return;

    fetch('/api/geofences', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({name, lat: e.latlng.lat, lon: e.latlng.lng, radius_nm: radiusNm}),
    }).then(r => r.json()).then(fence => {
        if (fence.id) drawGeofence(fence);
        refreshGeofenceList();
    });
});

function drawGeofence(fence) {
    if (geofenceCircles[fence.id]) map.removeLayer(geofenceCircles[fence.id]);
    const radiusMeters = fence.radius_nm * 1852;
    const circle = L.circle([fence.lat, fence.lon], {
        radius: radiusMeters, color: '#ff8800', fillColor: '#ff8800',
        fillOpacity: 0.08, weight: 1.5, dashArray: '6,4',
    }).addTo(map);
    circle.bindTooltip(fence.name, {permanent: true, direction: 'center', className: 'dark-tooltip'});
    geofenceCircles[fence.id] = circle;
}

function refreshGeofenceList() {
    fetch('/api/geofences').then(r => r.json()).then(data => {
        const el = document.getElementById('geofence-list');
        if (data.length === 0) { el.innerHTML = ''; return; }
        el.innerHTML = data.map(f =>
            `<div style="display:flex;justify-content:space-between;align-items:center;padding:1px 0;">
                <span style="color:#ff8800;">${esc(f.name)}</span>
                <span>${Number(f.radius_nm) || 0}nm
                    <a href="#" onclick="deleteGeofence(${parseInt(f.id, 10) || 0});return false;" style="color:#ff4444;margin-left:4px;">\u00D7</a>
                </span>
            </div>`
        ).join('');
        data.forEach(f => { if (!geofenceCircles[f.id]) drawGeofence(f); });
    });
}

function deleteGeofence(id) {
    fetch('/api/geofences/' + id, {method: 'DELETE'}).then(() => {
        if (geofenceCircles[id]) { map.removeLayer(geofenceCircles[id]); delete geofenceCircles[id]; }
        refreshGeofenceList();
    });
}

// --- Browser notifications + audio alerts ---
let notificationsEnabled = false;
let audioEnabled = false;
let lastEventId = 0;

document.getElementById('notify-toggle').addEventListener('change', function() {
    if (this.checked) {
        if ('Notification' in window && Notification.permission === 'default') {
            Notification.requestPermission().then(p => {
                notificationsEnabled = (p === 'granted');
                this.checked = notificationsEnabled;
            });
        } else {
            notificationsEnabled = ('Notification' in window && Notification.permission === 'granted');
            this.checked = notificationsEnabled;
        }
    } else {
        notificationsEnabled = false;
    }
    saveState();
});

document.getElementById('audio-toggle').addEventListener('change', function() {
    audioEnabled = this.checked;
    saveState();
});

function playAlertBeep() {
    if (!audioEnabled) return;
    try {
        const ctx = new (window.AudioContext || window.webkitAudioContext)();
        const osc = ctx.createOscillator();
        const gain = ctx.createGain();
        osc.connect(gain);
        gain.connect(ctx.destination);
        osc.frequency.value = 880;
        osc.type = 'sine';
        gain.gain.value = 0.3;
        osc.start();
        gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + 0.4);
        osc.stop(ctx.currentTime + 0.4);
    } catch(e) {}
}

function pollEvents() {
    if (!notificationsEnabled && !audioEnabled) return;
    fetch('/api/events?limit=5').then(r => r.json()).then(data => {
        if (!data || data.length === 0) return;
        const latestId = data[0].id;
        if (lastEventId === 0) { lastEventId = latestId; return; }
        if (latestId <= lastEventId) return;

        const newEvents = data.filter(e => e.id > lastEventId);
        lastEventId = latestId;

        newEvents.forEach(evt => {
            if (notificationsEnabled && 'Notification' in window && Notification.permission === 'granted') {
                new Notification('adsb-decode: ' + evt.event_type, {
                    body: evt.description,
                    tag: 'adsb-' + evt.id,
                });
            }
            playAlertBeep();
        });
    }).catch(() => {});
}

// Load geofences on startup
refreshGeofenceList();

// --- Event markers layer ---
let eventsLayer = L.layerGroup();
let eventsEnabled = false;
let maxEventId = 0;

const eventColors = {
    military_detected: '#ff6600',
    emergency_squawk: '#ff0000',
    rapid_descent: '#ff4444',
    circling: '#4488ff',
    geofence_entry: '#aa44ff',
    proximity: '#ff44aa',
    low_altitude: '#ffaa00',
};

document.getElementById('events-toggle').addEventListener('change', function() {
    eventsEnabled = this.checked;
    if (eventsEnabled) {
        eventsLayer.addTo(map);
        maxEventId = 0;
        fetchEventMarkers();
    } else {
        eventsLayer.clearLayers();
        map.removeLayer(eventsLayer);
        maxEventId = 0;
    }
    saveState();
});

function fetchEventMarkers() {
    if (!eventsEnabled) return;
    fetch('/api/events?limit=200').then(r => r.json()).then(data => {
        if (!data || data.length === 0) return;
        data.forEach(evt => {
            if (evt.lat == null || evt.lon == null) return;
            if (maxEventId > 0 && evt.id <= maxEventId) return;
            const color = eventColors[evt.event_type] || '#888888';
            const time = evt.timestamp ? new Date(evt.timestamp * 1000).toLocaleTimeString() : '';
            const typeName = (evt.event_type || '').replace(/_/g, ' ');
            const marker = L.circleMarker([evt.lat, evt.lon], {
                radius: 7, color: color, fillColor: color,
                fillOpacity: 0.6, weight: 1.5,
            }).addTo(eventsLayer);
            marker.bindTooltip(`${typeName} — ${time}`, { direction: 'right', className: 'dark-tooltip' });
            marker.bindPopup(`<div class="ac-popup">
                <div class="popup-title" style="color:${color};">${esc(typeName)}</div>
                <div class="popup-row"><span class="popup-label">ICAO</span><span class="popup-value"><a href="/aircraft/${encodeURIComponent(evt.icao)}">${esc(evt.icao)}</a></span></div>
                <div class="popup-row"><span class="popup-label">Description</span><span class="popup-value" style="max-width:180px;white-space:normal;">${esc(evt.description || '')}</span></div>
                <div class="popup-row"><span class="popup-label">Time</span><span class="popup-value">${time}</span></div>
                ${evt.altitude_ft != null ? `<div class="popup-row"><span class="popup-label">Altitude</span><span class="popup-value">${evt.altitude_ft.toLocaleString()} ft</span></div>` : ''}
                <div style="margin-top:6px;"><a href="/aircraft/${encodeURIComponent(evt.icao)}">Detail &rarr;</a></div>
            </div>`, { maxWidth: 300 });
        });
        if (data.length > 0) {
            const ids = data.filter(e => e.id != null).map(e => e.id);
            if (ids.length > 0) maxEventId = Math.max(...ids);
        }
    }).catch(() => {});
}

// --- Military highlight layer ---
let militaryLayer = L.layerGroup();
let militaryHighlightEnabled = false;
const militaryRings = {};

document.getElementById('military-highlight').addEventListener('change', function() {
    militaryHighlightEnabled = this.checked;
    if (militaryHighlightEnabled) {
        militaryLayer.addTo(map);
    } else {
        militaryLayer.clearLayers();
        map.removeLayer(militaryLayer);
        Object.keys(militaryRings).forEach(k => delete militaryRings[k]);
    }
    saveState();
});

// --- 3D CesiumJS globe ---
let cesiumViewer = null;
let cesiumLoaded = false;

function acBillboardUri(type, color, heading) {
    const svg = (acSvgs[type] || acSvgs.jet)(color);
    const rot = heading != null ? heading : 0;
    return 'data:image/svg+xml,' + encodeURIComponent(
        `<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24"><g transform="rotate(${rot} 12 12)">${svg}</g></svg>`
    );
}

function altColorRGBA(alt) {
    if (alt == null) return [136, 136, 136, 255];
    const t = Math.min(Math.max(alt / 40000, 0), 1);
    if (t < 0.5) {
        const s = t * 2;
        return [Math.round(s * 255), 255, 0, 255];
    } else {
        const s = (t - 0.5) * 2;
        return [255, Math.round((1 - s) * 255), 0, 255];
    }
}

function buildCZML(positions) {
    const byIcao = {};
    positions.forEach(p => {
        if (p.lat == null || p.lon == null) return;
        if (!byIcao[p.icao]) byIcao[p.icao] = [];
        byIcao[p.icao].push(p);
    });

    const doc = [{ id: 'document', name: 'ADS-B Replay', version: '1.0' }];

    for (const [icao, pts] of Object.entries(byIcao)) {
        pts.sort((a, b) => a.timestamp - b.timestamp);
        if (pts.length < 2) continue;

        const epoch = new Date(pts[0].timestamp * 1000).toISOString();
        const endTime = new Date(pts[pts.length - 1].timestamp * 1000).toISOString();

        // Set document clock on first entity
        if (doc[0].clock === undefined) {
            doc[0].clock = {
                interval: `${epoch}/${endTime}`,
                currentTime: epoch,
                multiplier: 60,
                range: 'LOOP_STOP',
                step: 'SYSTEM_CLOCK_MULTIPLIER',
            };
        }

        const cartDeg = [];
        for (const p of pts) {
            cartDeg.push(
                p.timestamp - pts[0].timestamp,
                p.lon,
                p.lat,
                (p.altitude_ft || 0) * 0.3048
            );
        }

        const avgAlt = pts.reduce((s, p) => s + (p.altitude_ft || 0), 0) / pts.length;
        const isMil = pts.some(p => p.is_military);
        const acType = isMil ? 'military' : classifyAircraft(pts[pts.length - 1]);
        const rgba = isMil ? [255, 68, 68, 255] : altColorRGBA(avgAlt);
        const hexColor = isMil ? '#ff4444' : altColor(avgAlt);

        doc.push({
            id: icao,
            name: pts[0].callsign || icao,
            availability: `${epoch}/${endTime}`,
            position: {
                epoch: epoch,
                cartographicDegrees: cartDeg,
                interpolationAlgorithm: 'LAGRANGE',
                interpolationDegree: 1,
            },
            billboard: {
                image: acBillboardUri(acType, hexColor),
                scale: 0.7,
                verticalOrigin: 'CENTER',
                heightReference: 'NONE',
            },
            path: {
                material: { solidColor: { color: { rgba: rgba } } },
                width: 2,
                trailTime: 180,
                leadTime: 0,
            },
        });
    }

    return doc;
}

// Build a model orientation from heading + a bank roll derived from turn-rate
// (change in heading between ticks). pitch=0; bank exaggerated + clamped for
// visibility, so the planes roll into their turns.
function acOrientation(lon, lat, altM, headingDeg, prevHeadingDeg) {
    let bankDeg = 0;
    if (prevHeadingDeg != null && headingDeg != null) {
        let d = headingDeg - prevHeadingDeg;
        while (d > 180) d -= 360;
        while (d < -180) d += 360;
        bankDeg = Math.max(-30, Math.min(30, d * 3));
    }
    const pos = Cesium.Cartesian3.fromDegrees(lon, lat, altM);
    const hpr = new Cesium.HeadingPitchRoll(
        Cesium.Math.toRadians((headingDeg || 0) + MODEL_HEADING_OFFSET),
        0,
        Cesium.Math.toRadians(bankDeg),
    );
    return Cesium.Transforms.headingPitchRollQuaternion(pos, hpr);
}

function loadCesiumJS() {
    return new Promise((resolve, reject) => {
        if (cesiumLoaded) { resolve(); return; }

        window.CESIUM_BASE_URL = 'https://cdn.jsdelivr.net/npm/cesium@1.119/Build/Cesium/';

        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = 'https://cdn.jsdelivr.net/npm/cesium@1.119/Build/Cesium/Widgets/widgets.css';
        document.head.appendChild(link);

        const script = document.createElement('script');
        script.src = 'https://cdn.jsdelivr.net/npm/cesium@1.119/Build/Cesium/Cesium.js';
        script.onload = () => { cesiumLoaded = true; resolve(); };
        script.onerror = () => reject(new Error('Failed to load CesiumJS'));
        document.head.appendChild(script);
    });
}

async function initCesiumViewer() {
    if (cesiumViewer) return;

    // Ion token (top of file) enables Cesium World Terrain. Empty = flat globe.
    Cesium.Ion.defaultAccessToken = CESIUM_ION_TOKEN || '';

    // Cesium 1.107+ removed the `imageryProvider` constructor option. Passing
    // it to `new Viewer({...})` does not attach the layer — the viewer renders
    // black until something else calls imageryLayers.addImageryProvider().
    // Build the viewer with no initial imagery, then route through
    // setCesiumMapStyle() (the same path the style dropdown uses) so first
    // activation matches subsequent style toggles.
    cesiumViewer = new Cesium.Viewer('cesium-container', {
        baseLayerPicker: false,
        geocoder: false,
        homeButton: false,
        sceneModePicker: false,
        navigationHelpButton: false,
        infoBox: true,
        selectionIndicator: true,
        animation: true,
        timeline: true,
        requestRenderMode: false,
        skyBox: false,
        // NOTE: do NOT pass `skyAtmosphere: true` — that option takes a
        // SkyAtmosphere instance or `false`; a bare `true` is stored verbatim
        // and Cesium then calls `true.setDynamicLighting()` every frame and
        // crashes the renderer. Omitting it lets Cesium build a real one.
    });

    // Cinematic depth (Air Loom borrow): horizon sky-glow + ground-atmosphere
    // haze + distance fog so the globe reads as a real atmosphere instead of a
    // flat dark ball. Sun lighting is left OFF on purpose so the night side
    // stays readable for a 24/7 traffic dashboard.
    const scene = cesiumViewer.scene;
    scene.globe.showGroundAtmosphere = true;
    scene.fog.enabled = true;
    scene.fog.density = 0.0006;
    if (scene.skyAtmosphere) { scene.skyAtmosphere.show = true; scene.skyAtmosphere.brightnessShift = 0.2; }

    // Real terrain (mountains under the planes) when an Ion token is configured.
    if (CESIUM_ION_TOKEN) {
        try {
            cesiumViewer.terrainProvider = await Cesium.createWorldTerrainAsync();
            scene.globe.depthTestAgainstTerrain = true; // planes/trails hide behind ridges
        } catch (e) { console.warn('Cesium terrain load failed:', e); }
    }

    const style = document.getElementById('map-style').value;
    await setCesiumMapStyle(style);
}

let cesiumLiveInterval = null;
const cesiumEntities = {};
let cesiumHeatmapEntities = [];
let cesiumAirportEntities = [];

async function activate3D() {
    is3DMode = true;
    document.getElementById('map').style.display = 'none';
    document.getElementById('cesium-container').style.display = 'block';
    document.getElementById('alt-legend').style.display = 'none';
    document.getElementById('mode-3d-btn').textContent = '2D';
    document.getElementById('mode-3d-btn').style.color = '#00ff88';

    await initCesiumViewer();

    // Set clock to real time (live mode)
    cesiumViewer.clock.shouldAnimate = true;
    cesiumViewer.clock.clockRange = Cesium.ClockRange.UNBOUNDED;
    cesiumViewer.clock.multiplier = 1;
    cesiumViewer.timeline.container.style.display = 'none';
    cesiumViewer.animation.container.style.display = 'none';

    // Fly to current map center at an oblique angle
    const c = map.getCenter();
    cesiumViewer.camera.flyTo({
        destination: Cesium.Cartesian3.fromDegrees(c.lng, c.lat - 2.0, 150000),
        orientation: {
            heading: Cesium.Math.toRadians(0),
            pitch: Cesium.Math.toRadians(-25),
            roll: 0,
        },
        duration: 0,
    });

    // Trackpad-friendly: make Shift+drag more responsive for tilting
    const ssc = cesiumViewer.scene.screenSpaceCameraController;
    ssc.minimumZoomDistance = 500;
    ssc.maximumZoomDistance = 5000000;

    // Initial load + start polling
    updateCesium3D();
    cesiumLiveInterval = setInterval(updateCesium3D, 2000);

    // Sync toggle state — render features already enabled in 2D
    if (heatmapEnabled) updateHeatmap();
    if (allAirports && getAptFilters && Object.values(getAptFilters()).some(v => v)) renderVisibleAirports();
    if (rangeEnabled) { clearCesiumRangeRings(); fetchAndDrawRangeRings(); }
    if (airspaceEnabled && airspaceData) renderAirspace3D(airspaceData);
    if (vesselsEnabled && vesselData) renderVessels3D(vesselData);
    if (weatherEnabled && weatherFrameUrl) applyWeatherTileUrl(weatherFrameUrl);
    if (satEnabled) renderCesiumSats();
}

function updateCesium3D() {
    if (!is3DMode || !cesiumViewer) return;
    Promise.all([
        fetch(`/api/positions?minutes=${trailMinutes}`).then(r => r.json()),
        fetch(`/api/trails?minutes=${trailMinutes}`).then(r => r.json()),
    ]).then(([posData, rawTrailData]) => {
        const trailData = rawTrailData.trails || rawTrailData;
        const seen = new Set();

        posData.forEach(p => {
            if (p.lat == null || p.lon == null) return;
            const key = p.icao;
            seen.add(key);
            const altM = (p.altitude_ft || 0) * 0.3048;
            const acType = classifyAircraft(p);
            const color = p.is_military ? '#ff4444' : altColor(p.altitude_ft);
            const rgba = p.is_military ? [255, 68, 68, 255] : altColorRGBA(p.altitude_ft);
            const trail = trailData[key] || [];

            if (!cesiumEntities[key]) {
                // Build polyline positions from trail
                const trailPositions = trail.map(t =>
                    Cesium.Cartesian3.fromDegrees(t.lon, t.lat, (t.altitude_ft || 0) * 0.3048)
                );
                trailPositions.push(Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM));

                const entity = cesiumViewer.entities.add({
                    id: key,
                    name: p.callsign || p.registration || key,
                    position: Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM),
                    orientation: acOrientation(p.lon, p.lat, altM, p.heading_deg, null),
                    model: {
                        uri: CESIUM_AIR_GLB,
                        minimumPixelSize: 36,
                        maximumScale: 12000,
                        color: Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 255),
                        colorBlendMode: Cesium.ColorBlendMode.MIX,
                        colorBlendAmount: 0.7,
                        silhouetteColor: Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 255),
                        silhouetteSize: 1.0,
                    },
                    label: {
                        text: (p.callsign || key) + (p.altitude_ft != null ? '\n' + Math.round(p.altitude_ft / 100) : ''),
                        font: '11px monospace',
                        fillColor: Cesium.Color.fromCssColorString('#e0e0e0'),
                        outlineColor: Cesium.Color.BLACK,
                        outlineWidth: 2,
                        style: Cesium.LabelStyle.FILL_AND_OUTLINE,
                        pixelOffset: new Cesium.Cartesian2(14, -14),
                        scale: 0.9,
                        distanceDisplayCondition: new Cesium.DistanceDisplayCondition(0, 500000),
                        disableDepthTestDistance: Number.POSITIVE_INFINITY,
                    },
                    polyline: (trailsEnabled && trailPositions.length > 1) ? {
                        positions: trailPositions,
                        width: 4,
                        material: new Cesium.PolylineGlowMaterialProperty({
                            glowPower: 0.2,
                            taperPower: 0.4,
                            color: Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 235),
                        }),
                        clampToGround: false,
                    } : undefined,
                    description: `<div style="font:12px monospace;">
                        <b>${esc(p.callsign || key)}</b>${p.is_military ? ' <span style="color:#f44">[MIL]</span>' : ''}<br>
                        Alt: ${p.altitude_ft != null ? p.altitude_ft.toLocaleString() + ' ft' : '--'}<br>
                        Spd: ${p.speed_kts ? Math.round(p.speed_kts) + ' kts' : '--'}<br>
                        Hdg: ${p.heading_deg != null ? Math.round(p.heading_deg) + '°' : '--'}<br>
                        ${p.country ? 'Country: ' + esc(p.country) + '<br>' : ''}
                    </div>`,
                });

                // Altitude stalk — thin line from ground to aircraft
                if (altM > 100) {
                    cesiumViewer.entities.add({
                        id: key + '_stalk',
                        polyline: {
                            positions: [
                                Cesium.Cartesian3.fromDegrees(p.lon, p.lat, 0),
                                Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM),
                            ],
                            width: 1,
                            material: new Cesium.ColorMaterialProperty(
                                Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 60)
                            ),
                        },
                    });
                }

                entity._hdg = p.heading_deg;
                cesiumEntities[key] = entity;
            } else {
                // Update existing entity
                const entity = cesiumEntities[key];
                entity.position = Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM);
                entity.orientation = acOrientation(p.lon, p.lat, altM, p.heading_deg, entity._hdg);
                entity._hdg = p.heading_deg;
                if (entity.model) entity.model.color = Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 255);
                entity.label.text = (p.callsign || key) + (p.altitude_ft != null ? '\n' + Math.round(p.altitude_ft / 100) : '');

                // Update trail
                if (trailsEnabled && trail.length > 0) {
                    const trailPositions = trail.map(t =>
                        Cesium.Cartesian3.fromDegrees(t.lon, t.lat, (t.altitude_ft || 0) * 0.3048)
                    );
                    trailPositions.push(Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM));
                    if (entity.polyline) {
                        entity.polyline.positions = trailPositions;
                    } else {
                        entity.polyline = new Cesium.PolylineGraphics({
                            positions: trailPositions,
                            width: 4,
                            material: new Cesium.PolylineGlowMaterialProperty({
                                glowPower: 0.2,
                                taperPower: 0.4,
                                color: Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 235),
                            }),
                            clampToGround: false,
                        });
                    }
                } else if (!trailsEnabled && entity.polyline) {
                    entity.polyline = undefined;
                }

                // Update altitude stalk
                const stalk = cesiumViewer.entities.getById(key + '_stalk');
                if (stalk) {
                    stalk.polyline.positions = [
                        Cesium.Cartesian3.fromDegrees(p.lon, p.lat, 0),
                        Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM),
                    ];
                }
            }
        });

        // Historical aircraft in 3D — only show at longer trail windows (>= 1h)
        if (trailMinutes >= 60) Object.keys(trailData).forEach(icao => {
            if (seen.has(icao)) return;
            const trail = trailData[icao];
            if (!trail || trail.length === 0) return;
            const last = trail[trail.length - 1];
            if (!last.lat || !last.lon) return;

            seen.add(icao);
            const altM = (last.altitude_ft || 0) * 0.3048;
            const acType = classifyAircraft({ speed_kts: last.speed_kts, altitude_ft: last.altitude_ft });
            const color = altColor(last.altitude_ft);
            const rgba = altColorRGBA(last.altitude_ft);

            // Compute heading from last two trail points
            let ghostHeading = null;
            if (trail.length >= 2) {
                const prev = trail[trail.length - 2];
                const dLon = (last.lon - prev.lon) * Math.PI / 180;
                const lat1 = prev.lat * Math.PI / 180;
                const lat2 = last.lat * Math.PI / 180;
                const y = Math.sin(dLon) * Math.cos(lat2);
                const x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(dLon);
                ghostHeading = ((Math.atan2(y, x) * 180 / Math.PI) + 360) % 360;
            }
            const trailPositions = trail.map(t =>
                Cesium.Cartesian3.fromDegrees(t.lon, t.lat, (t.altitude_ft || 0) * 0.3048)
            );

            if (!cesiumEntities[icao]) {
                const entity = cesiumViewer.entities.add({
                    id: icao,
                    name: icao,
                    position: Cesium.Cartesian3.fromDegrees(last.lon, last.lat, altM),
                    billboard: {
                        image: acBillboardUri(acType, color, ghostHeading),
                        scale: 0.6,
                        verticalOrigin: Cesium.VerticalOrigin.CENTER,
                        heightReference: Cesium.HeightReference.NONE,
                        disableDepthTestDistance: Number.POSITIVE_INFINITY,
                        color: new Cesium.Color(1, 1, 1, 0.35),
                    },
                    label: {
                        text: icao + (last.altitude_ft != null ? '\n' + Math.round(last.altitude_ft / 100) : ''),
                        font: '11px monospace',
                        fillColor: new Cesium.Color(0.88, 0.88, 0.88, 0.4),
                        outlineColor: Cesium.Color.BLACK,
                        outlineWidth: 2,
                        style: Cesium.LabelStyle.FILL_AND_OUTLINE,
                        pixelOffset: new Cesium.Cartesian2(14, -14),
                        scale: 0.9,
                        distanceDisplayCondition: new Cesium.DistanceDisplayCondition(0, 500000),
                        disableDepthTestDistance: Number.POSITIVE_INFINITY,
                    },
                    polyline: (trailsEnabled && trailPositions.length > 1) ? {
                        positions: trailPositions,
                        width: 2,
                        material: new Cesium.ColorMaterialProperty(
                            Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 100)
                        ),
                        clampToGround: false,
                    } : undefined,
                });

                if (altM > 100) {
                    cesiumViewer.entities.add({
                        id: icao + '_stalk',
                        polyline: {
                            positions: [
                                Cesium.Cartesian3.fromDegrees(last.lon, last.lat, 0),
                                Cesium.Cartesian3.fromDegrees(last.lon, last.lat, altM),
                            ],
                            width: 1,
                            material: new Cesium.ColorMaterialProperty(
                                Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], 30)
                            ),
                        },
                    });
                }

                cesiumEntities[icao] = entity;
            }
        });

        // Remove stale entities
        Object.keys(cesiumEntities).forEach(key => {
            if (!seen.has(key)) {
                cesiumViewer.entities.remove(cesiumEntities[key]);
                const stalk = cesiumViewer.entities.getById(key + '_stalk');
                if (stalk) cesiumViewer.entities.remove(stalk);
                delete cesiumEntities[key];
            }
        });

        // Update 3D route predictions
        drawPredictions3D(posData);

        cesiumViewer.scene.requestRender();
    }).catch(err => console.error('3D update failed:', err));
}

function deactivate3D() {
    is3DMode = false;
    if (cesiumLiveInterval) { clearInterval(cesiumLiveInterval); cesiumLiveInterval = null; }
    // Clean up all entities
    cesiumHeatmapEntities = [];
    cesiumAirportEntities = [];
    cesiumRangeEntities = [];
    cesiumPredictEntities = [];
    if (cesiumViewer) {
        cesiumViewer.entities.removeAll();
        Object.keys(cesiumEntities).forEach(k => delete cesiumEntities[k]);
    }
    // Re-render 2D features that were active
    if (heatmapEnabled) updateHeatmap();
    if (allAirports && Object.values(getAptFilters()).some(v => v)) renderVisibleAirports();
    if (rangeEnabled) { rangeLayer.clearLayers(); rangeLayer.addTo(map); fetchAndDrawRangeRings(); }
    if (airspaceEnabled) { clearCesiumAirspace(); airspaceLayer.clearLayers(); airspaceLayer.addTo(map); if (airspaceData) renderAirspace(airspaceData); }
    if (vesselsEnabled) { clearCesiumVessels(); vesselLayer.clearLayers(); vesselLayer.addTo(map); if (vesselData) renderVessels(vesselData); }
    if (weatherCesiumLayer && cesiumViewer) { cesiumViewer.imageryLayers.remove(weatherCesiumLayer, false); weatherCesiumLayer = null; }
    if (weatherEnabled && weatherLeafletLayer && !map.hasLayer(weatherLeafletLayer)) weatherLeafletLayer.addTo(map);
    document.getElementById('cesium-container').style.display = 'none';
    document.getElementById('map').style.display = 'block';
    document.getElementById('alt-legend').style.display = '';
    document.getElementById('mode-3d-btn').textContent = '3D';
    document.getElementById('mode-3d-btn').style.color = '#00aaff';
    map.invalidateSize();
}

document.getElementById('mode-3d-btn').addEventListener('click', function() {
    if (is3DMode) {
        deactivate3D();
    } else {
        this.textContent = '...';
        this.disabled = true;
        loadCesiumJS().then(() => {
            this.disabled = false;
            try {
                activate3D();
            } catch(err) {
                console.error('3D activation error:', err);
                this.textContent = '3D';
                is3DMode = false;
                document.getElementById('cesium-container').style.display = 'none';
                document.getElementById('map').style.display = 'block';
                document.getElementById('alt-legend').style.display = '';
            }
        }).catch(err => {
            console.error('CesiumJS load error:', err);
            this.textContent = '3D';
            this.disabled = false;
        });
    }
});

// --- Range rings layer ---
let rangeLayer = L.layerGroup();
let rangeEnabled = false;
let cesiumRangeEntities = [];
const rangeRadiiNm = [25, 50, 100, 150, 200];
const NM_TO_METERS = 1852;

document.getElementById('range-toggle').addEventListener('change', function() {
    rangeEnabled = this.checked;
    if (rangeEnabled) {
        rangeLayer.addTo(map);
        fetchAndDrawRangeRings();
    } else {
        rangeLayer.clearLayers();
        map.removeLayer(rangeLayer);
        clearCesiumRangeRings();
    }
    saveState();
});

function clearCesiumRangeRings() {
    cesiumRangeEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumRangeEntities = [];
}

function fetchAndDrawRangeRings() {
    if (!rangeEnabled) return;

    // Try receivers API first
    fetch('/api/v1/receivers').then(r => r.json()).then(data => {
        const receivers = data.filter(rx => rx.lat && rx.lon);
        if (receivers.length > 0) {
            receivers.forEach(rx => drawRangeRingsAt(rx.lat, rx.lon, rx.name || 'Receiver'));
        } else {
            // Fallback: use map center / receiver from stats
            fetch('/api/stats').then(r => r.json()).then(stats => {
                if (stats.receiver && stats.receiver.lat && stats.receiver.lon) {
                    drawRangeRingsAt(stats.receiver.lat, stats.receiver.lon, stats.receiver.name || 'Receiver');
                } else {
                    // Last resort: use current map center
                    const c = map.getCenter();
                    drawRangeRingsAt(c.lat, c.lng, 'Center');
                }
            }).catch(() => {
                const c = map.getCenter();
                drawRangeRingsAt(c.lat, c.lng, 'Center');
            });
        }
    }).catch(() => {
        // Receivers API not available — use stats or center
        fetch('/api/stats').then(r => r.json()).then(stats => {
            if (stats.receiver && stats.receiver.lat && stats.receiver.lon) {
                drawRangeRingsAt(stats.receiver.lat, stats.receiver.lon, stats.receiver.name || 'Receiver');
            }
        }).catch(() => {});
    });
}

function drawRangeRingsAt(lat, lon, label) {
    if (is3DMode && cesiumViewer) {
        rangeRadiiNm.forEach(nm => {
            const entity = cesiumViewer.entities.add({
                position: Cesium.Cartesian3.fromDegrees(lon, lat),
                ellipse: {
                    semiMajorAxis: nm * NM_TO_METERS,
                    semiMinorAxis: nm * NM_TO_METERS,
                    fill: false,
                    outline: true,
                    outlineColor: new Cesium.Color(0.3, 0.3, 0.3, 0.6),
                    outlineWidth: 1,
                    heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                },
                label: {
                    text: nm + ' nm',
                    font: '10px monospace',
                    fillColor: new Cesium.Color(0.5, 0.5, 0.5, 0.7),
                    outlineColor: Cesium.Color.BLACK,
                    outlineWidth: 1,
                    style: Cesium.LabelStyle.FILL_AND_OUTLINE,
                    pixelOffset: new Cesium.Cartesian2(0, -4),
                    scale: 0.8,
                    disableDepthTestDistance: Number.POSITIVE_INFINITY,
                },
            });
            cesiumRangeEntities.push(entity);
        });
        cesiumViewer.scene.requestRender();
        return;
    }

    // 2D Leaflet range rings
    rangeRadiiNm.forEach(nm => {
        const circle = L.circle([lat, lon], {
            radius: nm * NM_TO_METERS,
            color: '#444',
            weight: 1,
            fill: false,
            dashArray: '8,6',
            interactive: false,
        }).addTo(rangeLayer);

        // Label at north cardinal point
        const labelLat = lat + (nm * NM_TO_METERS) / 111320;
        L.marker([labelLat, lon], {
            icon: L.divIcon({
                className: '',
                html: `<span style="font-size:10px;color:#666;background:rgba(10,10,10,0.8);padding:0 3px;border-radius:2px;">${nm}nm</span>`,
                iconAnchor: [16, 10],
            }),
            interactive: false,
        }).addTo(rangeLayer);
    });
}

// --- Airspace overlay ---
let airspaceLayer = L.layerGroup();
let airspaceEnabled = false;
let airspaceData = null;
let cesiumAirspaceEntities = [];

const airspaceColors = {
    B: { fill: 'rgba(0,100,255,0.12)', stroke: '#0066ff', label: 'Class B' },
    C: { fill: 'rgba(128,0,200,0.10)', stroke: '#8800cc', label: 'Class C' },
    D: { fill: 'rgba(0,120,80,0.10)', stroke: '#008855', label: 'Class D' },
    R: { fill: 'rgba(255,0,0,0.12)', stroke: '#ff0000', label: 'Restricted' },
    P: { fill: 'rgba(255,0,0,0.15)', stroke: '#cc0000', label: 'Prohibited' },
    A: { fill: 'rgba(255,160,0,0.10)', stroke: '#ff8800', label: 'Alert' },
    W: { fill: 'rgba(255,200,0,0.10)', stroke: '#ffaa00', label: 'Warning' },
};

document.getElementById('airspace-toggle').addEventListener('change', function() {
    airspaceEnabled = this.checked;
    if (airspaceEnabled) {
        airspaceLayer.addTo(map);
        fetchAirspace();
    } else {
        airspaceLayer.clearLayers();
        map.removeLayer(airspaceLayer);
        clearCesiumAirspace();
    }
    saveState();
});

function clearCesiumAirspace() {
    cesiumAirspaceEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumAirspaceEntities = [];
}

function fetchAirspace() {
    if (!airspaceEnabled) return;
    if (airspaceData) { renderAirspace(airspaceData); return; }
    fetch('/api/airspace').then(r => r.json()).then(data => {
        airspaceData = data;
        renderAirspace(data);
    }).catch(err => console.error('Airspace fetch failed:', err));
}

function renderAirspace(data) {
    if (!data || !data.features) return;
    if (is3DMode && cesiumViewer) { renderAirspace3D(data); return; }

    airspaceLayer.clearLayers();
    for (const feature of data.features) {
        const props = feature.properties || {};
        const classKey = props.CLASS || props.TYPE_CODE || '?';
        const style = airspaceColors[classKey] || { fill: 'rgba(128,128,128,0.08)', stroke: '#666', label: classKey };
        const name = props.NAME || props.IDENT || '';
        const lowerVal = props.LOWER_VAL != null ? props.LOWER_VAL : 'SFC';
        const upperVal = props.UPPER_VAL != null ? props.UPPER_VAL : '?';

        if (!feature.geometry || !feature.geometry.coordinates) continue;
        const coords = feature.geometry.coordinates;
        // GeoJSON polygons: [[ring], ...] where ring = [[lon,lat], ...]
        const rings = (feature.geometry.type === 'MultiPolygon')
            ? coords.flatMap(poly => poly)
            : coords;

        const latLngs = rings.map(ring => ring.map(c => [c[1], c[0]]));
        const polygon = L.polygon(latLngs, {
            color: style.stroke,
            weight: 1,
            fillColor: style.fill,
            fillOpacity: 0.3,
            opacity: 0.6,
        }).addTo(airspaceLayer);
        polygon.bindPopup(`<b>${name}</b><br>${style.label}<br>${lowerVal} – ${upperVal} ft`);
    }
}

function renderAirspace3D(data) {
    clearCesiumAirspace();
    if (!cesiumViewer || !data || !data.features) return;

    for (const feature of data.features) {
        const props = feature.properties || {};
        const classKey = props.CLASS || props.TYPE_CODE || '?';
        const style = airspaceColors[classKey];
        if (!style) continue;
        if (!feature.geometry || !feature.geometry.coordinates) continue;

        const coords = feature.geometry.coordinates;
        const rings = (feature.geometry.type === 'MultiPolygon')
            ? coords.flatMap(poly => poly)
            : coords;

        for (const ring of rings) {
            const positions = ring.flat();
            const rgba = style.stroke;
            const r = parseInt(rgba.slice(1,3), 16) / 255;
            const g = parseInt(rgba.slice(3,5), 16) / 255;
            const b = parseInt(rgba.slice(5,7), 16) / 255;

            const entity = cesiumViewer.entities.add({
                polygon: {
                    hierarchy: Cesium.Cartesian3.fromDegreesArray(positions),
                    material: new Cesium.Color(r, g, b, 0.15),
                    outline: true,
                    outlineColor: new Cesium.Color(r, g, b, 0.6),
                    heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                },
                name: props.NAME || classKey,
                description: `${style.label}<br>${props.LOWER_VAL || 'SFC'} – ${props.UPPER_VAL || '?'} ft`,
            });
            cesiumAirspaceEntities.push(entity);
        }
    }
    cesiumViewer.scene.requestRender();
}

// --- Route prediction lines ---
let predictEnabled = false;
let predictLines = {};
let cesiumPredictEntities = [];
const predictDurSteps = [5, 10, 15, 20, 30];
const predictDurLabels = ['5 min', '10 min', '15 min', '20 min', '30 min'];
let predictMinutes = 10;
const predictToggle = document.getElementById('predict-toggle');
const predictDurSlider = document.getElementById('predict-dur-slider');
const predictDurLabel = document.getElementById('predict-dur-label');
const predictDurRow = document.getElementById('predict-dur-row');

predictToggle.addEventListener('change', function() {
    predictEnabled = this.checked;
    predictDurRow.style.display = predictEnabled ? '' : 'none';
    if (!predictEnabled) clearPredictions();
    saveState();
});

predictDurSlider.addEventListener('input', function() {
    const idx = parseInt(this.value);
    predictMinutes = predictDurSteps[idx];
    predictDurLabel.textContent = predictDurLabels[idx];
    saveState();
});

function clearPredictions() {
    Object.values(predictLines).forEach(line => map.removeLayer(line));
    predictLines = {};
    cesiumPredictEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumPredictEntities = [];
}

function predictPosition(lat, lon, headingDeg, speedKts, minutesAhead) {
    // Great circle approximation for short distances
    const speedNmPerMin = speedKts / 60;
    const distNm = speedNmPerMin * minutesAhead;
    const headingRad = headingDeg * Math.PI / 180;
    const latRad = lat * Math.PI / 180;

    // Approximate: 1nm = 1/60 degree latitude
    const dLat = (distNm * Math.cos(headingRad)) / 60;
    const dLon = (distNm * Math.sin(headingRad)) / (60 * Math.cos(latRad));

    return [lat + dLat, lon + dLon];
}

function drawPredictions(posData) {
    // Clear old predictions
    Object.values(predictLines).forEach(line => map.removeLayer(line));
    predictLines = {};

    if (!predictEnabled) return;

    posData.forEach(p => {
        if (p.heading_deg == null || p.speed_kts == null) return;
        if (p.speed_kts < 30) return; // Skip ground/parked aircraft

        const points = [[p.lat, p.lon]];
        const steps = 4; // Intermediate points for smoother line
        for (let i = 1; i <= steps; i++) {
            const mins = (predictMinutes / steps) * i;
            points.push(predictPosition(p.lat, p.lon, p.heading_deg, p.speed_kts, mins));
        }

        const baseColor = altColor(p.altitude_ft);
        const line = L.polyline(points, {
            color: baseColor,
            weight: 1.5,
            opacity: 0.4,
            dashArray: '6,8',
            interactive: false,
        }).addTo(map);
        predictLines[p.icao] = line;
    });
}

function drawPredictions3D(posData) {
    cesiumPredictEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumPredictEntities = [];

    if (!predictEnabled || !cesiumViewer) return;

    posData.forEach(p => {
        if (p.heading_deg == null || p.speed_kts == null) return;
        if (p.speed_kts < 30) return;

        const altM = (p.altitude_ft || 0) * 0.3048;
        const endPos = predictPosition(p.lat, p.lon, p.heading_deg, p.speed_kts, predictMinutes);
        const rgba = p.is_military ? [255, 68, 68, 80] : altColorRGBA(p.altitude_ft);
        rgba[3] = 80; // Lower alpha for prediction

        const entity = cesiumViewer.entities.add({
            polyline: {
                positions: [
                    Cesium.Cartesian3.fromDegrees(p.lon, p.lat, altM),
                    Cesium.Cartesian3.fromDegrees(endPos[1], endPos[0], altM),
                ],
                width: 1.5,
                material: new Cesium.PolylineDashMaterialProperty({
                    color: Cesium.Color.fromBytes(rgba[0], rgba[1], rgba[2], rgba[3]),
                    dashLength: 12,
                }),
            },
        });
        cesiumPredictEntities.push(entity);
    });
    cesiumViewer.scene.requestRender();
}

// --- Vessel (AIS) overlay ---
let vesselsEnabled = false;
let vesselLayer = L.layerGroup();
let vesselData = null;
let cesiumVesselEntities = [];

const vesselTypeColors = {
    'Cargo': '#9e9e9e',
    'Tanker': '#e53935',
    'Passenger': '#1e88e5',
    'Military': '#43a047',
    'Fishing': '#ff9800',
    'Tug': '#795548',
    'Sailing': '#ab47bc',
};

function vesselColor(type) {
    return vesselTypeColors[type] || '#607d8b';
}

// Ship icon SVG (simple triangle/arrow shape)
function shipIcon(color, heading) {
    const rotation = heading || 0;
    return L.divIcon({
        html: `<div style="transform:rotate(${rotation}deg);width:16px;height:16px;display:flex;align-items:center;justify-content:center;">
            <svg width="16" height="16" viewBox="0 0 16 16">
                <polygon points="8,0 14,14 8,10 2,14" fill="${color}" stroke="#000" stroke-width="0.5" opacity="0.9"/>
            </svg>
        </div>`,
        className: '',
        iconSize: [16, 16],
        iconAnchor: [8, 8],
    });
}

document.getElementById('vessel-toggle').addEventListener('change', function() {
    vesselsEnabled = this.checked;
    if (vesselsEnabled) {
        vesselLayer.addTo(map);
        fetchVessels();
    } else {
        vesselLayer.clearLayers();
        map.removeLayer(vesselLayer);
        clearCesiumVessels();
    }
    saveState();
});

function clearCesiumVessels() {
    cesiumVesselEntities.forEach(e => { if (cesiumViewer) cesiumViewer.entities.remove(e); });
    cesiumVesselEntities = [];
}

function fetchVessels() {
    if (!vesselsEnabled) return;
    fetch('/api/vessel-positions/latest?limit=200')
        .then(r => r.json())
        .then(data => {
            vesselData = data;
            renderVessels(data);
        })
        .catch(err => console.error('Vessel fetch failed:', err));
}

function renderVessels(data) {
    vesselLayer.clearLayers();
    if (is3DMode && cesiumViewer) { renderVessels3D(data); return; }

    // Get vessel metadata for colors
    fetch('/api/vessels?limit=200')
        .then(r => r.json())
        .then(vessels => {
            const vesselMap = {};
            vessels.forEach(v => { vesselMap[v.mmsi] = v; });

            data.forEach(pos => {
                const vessel = vesselMap[pos.mmsi] || {};
                const color = vesselColor(vessel.vessel_type);
                const icon = shipIcon(color, pos.course_deg || pos.heading_deg);

                const marker = L.marker([pos.lat, pos.lon], { icon })
                    .bindTooltip(esc(vessel.name || pos.mmsi), { permanent: false })
                    .bindPopup(`
                        <div style="font-family:monospace;font-size:12px;">
                            <b style="color:${color};">${esc(vessel.name || 'Unknown')}</b><br>
                            MMSI: ${esc(pos.mmsi)}<br>
                            Type: ${esc(vessel.vessel_type || '-')}<br>
                            Flag: ${esc(vessel.flag || '-')}<br>
                            Speed: ${pos.speed_kts ? pos.speed_kts.toFixed(1) + ' kts' : '-'}<br>
                            Course: ${pos.course_deg ? pos.course_deg.toFixed(0) + '\u00B0' : '-'}<br>
                        </div>
                    `);
                vesselLayer.addLayer(marker);
            });
        });
}

function renderVessels3D(data) {
    clearCesiumVessels();
    if (!cesiumViewer) return;

    fetch('/api/vessels?limit=200')
        .then(r => r.json())
        .then(vessels => {
            const vesselMap = {};
            vessels.forEach(v => { vesselMap[v.mmsi] = v; });

            data.forEach(pos => {
                const vessel = vesselMap[pos.mmsi] || {};
                const color = vesselColor(vessel.vessel_type);
                const [r, g, b] = hexToRgb(color);

                const entity = cesiumViewer.entities.add({
                    position: Cesium.Cartesian3.fromDegrees(pos.lon, pos.lat, 0),
                    point: {
                        pixelSize: 8,
                        color: Cesium.Color.fromBytes(r, g, b, 220),
                        outlineColor: Cesium.Color.BLACK,
                        outlineWidth: 1,
                        heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                    },
                    label: {
                        text: vessel.name || pos.mmsi,
                        font: '10px monospace',
                        fillColor: Cesium.Color.fromBytes(r, g, b),
                        style: Cesium.LabelStyle.FILL_AND_OUTLINE,
                        outlineColor: Cesium.Color.BLACK,
                        outlineWidth: 2,
                        pixelOffset: new Cesium.Cartesian2(0, -12),
                        heightReference: Cesium.HeightReference.CLAMP_TO_GROUND,
                    },
                    name: vessel.name || pos.mmsi,
                    description: `MMSI: ${pos.mmsi}<br>Type: ${vessel.vessel_type || '-'}<br>Speed: ${pos.speed_kts ? pos.speed_kts.toFixed(1) + ' kts' : '-'}`,
                });
                cesiumVesselEntities.push(entity);
            });
            cesiumViewer.scene.requestRender();
        });
}

function hexToRgb(hex) {
    const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
    return result ? [parseInt(result[1], 16), parseInt(result[2], 16), parseInt(result[3], 16)] : [128, 128, 128];
}

// Refresh vessel positions periodically (every 10 seconds)
setInterval(fetchVessels, 10000);

// --- Weather radar overlay (RainViewer) ---
// RainViewer is a free, key-less public radar tile service. Manifest gives
// the latest frame; tiles render on Leaflet (2D) and Cesium (3D) the same way.
let weatherEnabled = false;
let weatherLeafletLayer = null;
let weatherCesiumLayer = null;
let weatherFrameUrl = null;
let weatherRefreshTimer = null;
const RAINVIEWER_MANIFEST = 'https://api.rainviewer.com/public/weather-maps.json';
const RAINVIEWER_REFRESH_MS = 5 * 60 * 1000;

// Build the tile-URL template for the latest radar frame.
// Returns a string like "https://tilecache.rainviewer.com/v2/radar/{ts}/256/{z}/{x}/{y}/4/1_1.png"
// or null if the manifest doesn't have a usable past frame.
function buildWeatherTileUrl(manifest) {
    if (!manifest || !manifest.host || !manifest.radar || !Array.isArray(manifest.radar.past)) return null;
    const past = manifest.radar.past;
    if (past.length === 0) return null;
    const latest = past[past.length - 1];
    if (!latest || !latest.path) return null;
    return `${manifest.host}${latest.path}/256/{z}/{x}/{y}/4/1_1.png`;
}

function fetchWeatherManifest() {
    return fetch(RAINVIEWER_MANIFEST, { cache: 'no-store' })
        .then(r => r.json())
        .then(manifest => buildWeatherTileUrl(manifest));
}

function applyWeatherTileUrl(tileUrl) {
    if (!tileUrl) return;
    weatherFrameUrl = tileUrl;
    if (weatherLeafletLayer) { map.removeLayer(weatherLeafletLayer); weatherLeafletLayer = null; }
    weatherLeafletLayer = L.tileLayer(tileUrl, {
        opacity: 0.6,
        attribution: '&copy; <a href="https://rainviewer.com">RainViewer</a>',
        maxZoom: 18,
        tileSize: 256,
    });
    if (!is3DMode) weatherLeafletLayer.addTo(map);

    if (cesiumViewer) {
        if (weatherCesiumLayer) { cesiumViewer.imageryLayers.remove(weatherCesiumLayer, false); weatherCesiumLayer = null; }
        const provider = new Cesium.UrlTemplateImageryProvider({
            url: tileUrl,
            credit: 'RainViewer',
            tileWidth: 256,
            tileHeight: 256,
        });
        weatherCesiumLayer = cesiumViewer.imageryLayers.addImageryProvider(provider);
        weatherCesiumLayer.alpha = 0.6;
    }
}

function enableWeather() {
    weatherEnabled = true;
    fetchWeatherManifest().then(applyWeatherTileUrl).catch(err => console.error('Weather fetch failed:', err));
    if (!weatherRefreshTimer) {
        weatherRefreshTimer = setInterval(() => {
            if (weatherEnabled) {
                fetchWeatherManifest().then(applyWeatherTileUrl).catch(err => console.error('Weather refresh failed:', err));
            }
        }, RAINVIEWER_REFRESH_MS);
    }
}

function disableWeather() {
    weatherEnabled = false;
    if (weatherLeafletLayer) { map.removeLayer(weatherLeafletLayer); weatherLeafletLayer = null; }
    if (weatherCesiumLayer && cesiumViewer) { cesiumViewer.imageryLayers.remove(weatherCesiumLayer, false); weatherCesiumLayer = null; }
    if (weatherRefreshTimer) { clearInterval(weatherRefreshTimer); weatherRefreshTimer = null; }
    weatherFrameUrl = null;
}

document.getElementById('weather-toggle').addEventListener('change', function() {
    if (this.checked) enableWeather();
    else disableWeather();
    saveState();
});

// --- Panel toggle ---
document.getElementById('panel-header').addEventListener('click', function() {
    const body = document.getElementById('panel-body');
    const toggle = document.getElementById('panel-toggle');
    body.classList.toggle('collapsed');
    toggle.innerHTML = body.classList.contains('collapsed') ? '&#9650;' : '&#9660;';
});

// --- WebSocket position stream ---
// Single persistent connection replaces the 2-second /api/positions poll.
// Server pushes the same JSON shape on the same cadence, but a tab keeps one
// connection instead of opening 30 HTTP requests/min. Polling is the fallback
// when WebSocket isn't available (proxy strips upgrade, network blocks, etc.).
let positionsWs = null;
let positionsPollHandle = null;
let positionsWsBackoffMs = 2000;

function startPositionsPollingFallback() {
    if (positionsPollHandle) return;
    positionsPollHandle = setInterval(updateMap, 2000);
}

function stopPositionsPollingFallback() {
    if (!positionsPollHandle) return;
    clearInterval(positionsPollHandle);
    positionsPollHandle = null;
}

function connectPositionsWebSocket() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/ws/positions?minutes=${trailMinutes}`;
    let ws;
    try {
        ws = new WebSocket(url);
    } catch (e) {
        console.warn('WebSocket init failed; using polling.', e);
        startPositionsPollingFallback();
        return;
    }
    positionsWs = ws;

    ws.onopen = () => {
        positionsWsBackoffMs = 2000;
        stopPositionsPollingFallback();
    };
    ws.onmessage = (ev) => {
        try {
            const data = JSON.parse(ev.data);
            applyPositions(data);
        } catch (e) {
            console.error('WebSocket parse error', e);
        }
    };
    ws.onerror = () => {
        // Browsers fire error before close. Don't double-restart polling here;
        // onclose will handle it.
    };
    ws.onclose = () => {
        if (positionsWs === ws) positionsWs = null;
        startPositionsPollingFallback();
        const backoff = positionsWsBackoffMs;
        positionsWsBackoffMs = Math.min(positionsWsBackoffMs * 1.5, 30000);
        setTimeout(connectPositionsWebSocket, backoff);
    };
}

function reconnectPositionsWebSocket() {
    if (positionsWs && positionsWs.readyState === WebSocket.OPEN) {
        positionsWs.close();
        // onclose will trigger reconnect with the new trailMinutes.
    } else {
        connectPositionsWebSocket();
    }
}

// --- Initialize ---
restoreState();
centerOnReceiver();
updateMap();
updateTrails();
updateStats();
connectPositionsWebSocket();
setInterval(updateTrails, 10000);  // Trail data: every 10s
setInterval(updateStats, 10000);
setInterval(updateHeatmap, 30000); // Heatmap: every 30s
setInterval(pollEvents, 5000);
setInterval(fetchEventMarkers, 10000);
pollEvents();
if (eventsEnabled) fetchEventMarkers();
