/* adsb-decode — error-report.js (client error -> server logging)
 *
 * Captures uncaught browser errors on the live tracking UI (map / 3D globe /
 * replay / NLP query) and POSTs a structured JSON payload to /api/clientlog so
 * the full stack surfaces in the server's tracing logs — readable with:
 *   journalctl -u adsb-decode | grep '\[clientlog\]'
 *
 * Adapted from the splatlas pattern. Plain (non-module) IIFE, served via
 * include_str! like map.js and loaded first in <head> so it catches everything.
 *
 * Captures: window 'error', window 'unhandledrejection'. The Cesium 3D globe is
 * the most likely shader/WebGL failure surface; app code can also report
 * explicitly via window.__reportError({...}) from a Cesium renderError hook.
 *
 * Design rules:
 *   - FAIL SILENTLY. Logging must never break the page.
 *   - DEDUPE + THROTTLE. Send first occurrence, suppress repeats within 60s,
 *     hard-cap at 50 POSTs per page load.
 *   - NO PII. UA + a random per-page sessionId only.
 */
(function () {
  'use strict';

  var ENDPOINT = '/api/clientlog';
  var MAX_MESSAGE = 8000;
  var MAX_STACK = 4000;
  var MAX_PER_SESSION = 50;
  var DEDUPE_WINDOW_MS = 60000;

  function makeSessionId() {
    try {
      if (typeof crypto !== 'undefined' && crypto.randomUUID) return crypto.randomUUID();
    } catch (e) { /* */ }
    return 'sid-' + Math.random().toString(36).slice(2) + '-' + Date.now().toString(36);
  }

  function truncate(s, max) {
    if (typeof s !== 'string') {
      if (s == null) return undefined;
      try { s = String(s); } catch (e) { return undefined; }
    }
    if (s.length <= max) return s;
    return s.slice(0, max) + '…[+' + (s.length - max) + ' chars truncated]';
  }

  function detectShaderType(message) {
    if (typeof message !== 'string') return undefined;
    var m = message.toLowerCase();
    var hasF = m.indexOf('fragment shader') !== -1;
    var hasV = m.indexOf('vertex shader') !== -1;
    if (hasF && !hasV) return 'fragment';
    if (hasV && !hasF) return 'vertex';
    if (hasF && hasV) return 'both';
    if (m.indexOf('shader') !== -1 && (m.indexOf('compile') !== -1 || m.indexOf('glsl') !== -1)) return 'unknown';
    return undefined;
  }

  var SESSION_ID = makeSessionId();
  var seen = {};
  var totalSent = 0;

  function buildPayload(input) {
    input = input || {};
    var error = input.error;
    var message = input.message;
    var source = input.source || 'unknown';
    var extra = input.extra;

    var msg = message, name, stack;
    if (error && typeof error === 'object') {
      if (msg == null) msg = error.message;
      name = error.name;
      stack = error.stack;
    } else if (error != null && msg == null) {
      msg = error;
    }

    var finalMessage = truncate(msg != null ? msg : '(no message)', MAX_MESSAGE);
    var payload = {
      ts: Date.now(),
      source: source,
      url: (typeof location !== 'undefined') ? location.href : undefined,
      message: finalMessage,
      name: name || undefined,
      stack: truncate(stack, MAX_STACK),
      shaderType: detectShaderType(finalMessage),
      ua: truncate((typeof navigator !== 'undefined') ? navigator.userAgent : undefined, 1024),
      sessionId: SESSION_ID
    };
    if (extra && typeof extra === 'object') payload.extra = extra;
    for (var k in payload) {
      if (payload.hasOwnProperty(k) && payload[k] === undefined) delete payload[k];
    }
    return payload;
  }

  function shouldSend(payload) {
    if (totalSent >= MAX_PER_SESSION) return false;
    var key = [payload.source, payload.name || '', payload.message || '', payload.shaderType || ''].join('|');
    var now = payload.ts;
    var last = seen[key];
    if (last != null && now - last < DEDUPE_WINDOW_MS) return false;
    seen[key] = now;
    totalSent += 1;
    return true;
  }

  function post(payload) {
    try {
      var body = JSON.stringify(payload);
      if (typeof navigator !== 'undefined' && navigator.sendBeacon) {
        var blob = new Blob([body], { type: 'application/json' });
        if (navigator.sendBeacon(ENDPOINT, blob)) return;
      }
      if (typeof fetch === 'function') {
        fetch(ENDPOINT, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: body,
          keepalive: true
        }).catch(function () { /* swallow */ });
      }
    } catch (e) { /* never throw out of logging */ }
  }

  function reportError(input) {
    try {
      var payload = buildPayload(input);
      if (!shouldSend(payload)) return false;
      post(payload);
      return true;
    } catch (e) {
      return false;
    }
  }

  try {
    if (typeof window !== 'undefined' && window.addEventListener) {
      window.addEventListener('error', function (e) {
        try {
          reportError({
            source: 'window-error',
            error: e.error,
            message: e.message,
            extra: e.filename ? { filename: e.filename, line: e.lineno, col: e.colno } : undefined
          });
        } catch (err) { /* */ }
      });
      window.addEventListener('unhandledrejection', function (e) {
        try {
          reportError({ source: 'unhandledrejection', error: e && e.reason });
        } catch (err) { /* */ }
      });
    }
    if (typeof window !== 'undefined') window.__reportError = reportError;
  } catch (e) { /* never throw out of install */ }
})();
