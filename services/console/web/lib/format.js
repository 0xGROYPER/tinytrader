// Formatting shared by the console and its widgets (decision 0010).

export const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

// Times are shown in New York time (ET), the market's, 24-hour, whatever the
// browser's own zone: the charts, the tape and the session's hours all use it.
export const NY = 'America/New_York';
/** A moment (ms) as ET: 14:05, or 14:05:09 with seconds. */
export const etTime = (ms, seconds = false) => new Date(ms).toLocaleTimeString('en-GB', { timeZone: NY, hour: '2-digit', minute: '2-digit', ...(seconds ? { second: '2-digit' } : {}) });
/** A moment (ms) as ET with its date unless today's: "14:05:09 ET", "Oct 14, 14:05 ET". */
export function etStamp(ms, seconds = false) {
  const day = t => new Date(t).toLocaleDateString('en-CA', { timeZone: NY });
  if (day(ms) === day(Date.now())) return `${etTime(ms, seconds)} ET`;
  return `${new Date(ms).toLocaleDateString('en-US', { timeZone: NY, month: 'short', day: 'numeric' })}, ${etTime(ms)} ET`;
}

export function bytes(n) {
  if (n == null) return '—';
  const u = ['B', 'KB', 'MB', 'GB', 'TB']; let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i ? n.toFixed(1) : n.toFixed(0)) + ' ' + u[i];
}

export function duration(s) {
  if (s == null || !isFinite(s)) return '—';
  s = Math.max(0, Math.floor(s));
  const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : m ? `${m}m ${s % 60}s` : `${s}s`;
}

// Display formats a topology can ask for. Nothing here knows about any particular service.
export function fmt(v, spec = {}) {
  if (v == null || (typeof v === 'number' && !isFinite(v))) return '—';
  let out;
  switch (spec.format) {
    case 'bytes': out = bytes(v); break;
    case 'count': out = Math.round(v).toLocaleString(); break;
    case 'age': out = v < 60 ? v.toFixed(1) + 's' : duration(v); break;
    case 'percent': out = v.toFixed(1) + '%'; break;
    default: { const d = spec.decimals ?? 2; out = v.toLocaleString(undefined, { minimumFractionDigits: d, maximumFractionDigits: d }); }
  }
  return (spec.prefix || '') + out + (spec.suffix || '');
}

export function ago(iso) {
  const s = (Date.now() - Date.parse(iso)) / 1000;
  if (!isFinite(s)) return '';
  return s < 60 ? 'just now' : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
}

// Stories may carry HTML entities (&#39;); decode them to text, which esc() then escapes.
export function unentity(str) { const t = document.createElement('textarea'); t.innerHTML = str || ''; return t.value; }

export const labelText = l => { const e = Object.entries(l || {}); return e.length ? '{' + e.map(([k, v]) => `${k}="${v}"`).join(',') + '}' : ''; };
export const clip = (s, n) => s.length > n ? s.slice(0, n - 1) + '…' : s;
