// Profile mode: the whole page measured while it's used. Off by default and
// free when off (nothing is wrapped until it is turned on); turned on from
// the palette or by opening the console with #profile, and remembered for
// the tab. While on it measures every widget's mount, update and dispose
// (and whether a redraw changed anything), every fetch (path, time, size,
// status, and the widget that asked), main-thread stalls (long tasks where
// the browser reports them, else gaps between animation frames, as in
// Safari), frame rate, the DOM's size and the JS heap where available, the
// state poll's size and parse time, tt-* events, clicks and keys, and
// errors. A bar along the bottom shows the last minute; badges show each
// panel's update time. Record collects a bounded event log; Stop uploads it
// to the console (/api/profiles, read on the host with
// services/console/scripts/profile-report) and offers it as a download.

const KEY = 'tt-profile';
// The page's own timers, unwrapped, so profiling doesn't measure itself.
const raf = window.requestAnimationFrame.bind(window), every = window.setInterval.bind(window);
const MAX_EVENTS = 20000, MAX_BYTES = 5e6;

/** True while profile mode is on; callers skip all measuring when false. */
export const prof = { on: false };

/** Whether this tab asked for profile mode (#profile, or turned on earlier). */
export function profileWanted() {
  try { return location.hash === '#profile' || sessionStorage.getItem(KEY) === '1'; } catch (_) { return location.hash === '#profile'; }
}

// Totals since profile mode started, and the last minute's events.
const t0 = performance.now();
const now = () => performance.now() - t0;
/** @type {Map<string, { view: string, mount: number, updates: number, total: number, max: number, last: number, changed: number, unchanged: number, dispose: number }>} */
const widgets = new Map();
/** @type {any[]} */ let recent = [];          // the last 60 s, for the bar
/** @type {any[] | null} */ let rec = null;    // the recording, while recording
let recBytes = 0, recStart = 0, recDropped = 0, recMeta = null;
let current = '', currentKey = '';             // the widget (and panel) whose code is running, synchronously
const samples = [];                            // every 5 s: dom, heap, fps
let frames = 0, fps = 0, lastFrame = 0;

function log(e) {
  e.t ??= Math.round(now());
  recent.push(e);
  if (rec) {
    if (rec.length >= MAX_EVENTS || recBytes >= MAX_BYTES) { recDropped++; return; }
    rec.push(e); recBytes += 60 + JSON.stringify(e).length;
  }
}

/** Times a widget's mount, update or dispose. */
export function timed(op, view, key, fn) {
  const w = widgets.get(key) || { view, mount: 0, updates: 0, total: 0, max: 0, last: 0, changed: 0, unchanged: 0, dispose: 0 };
  widgets.set(key, w);
  const prev = current, prevKey = currentKey; current = view; currentKey = key;
  const a = performance.now();
  try { return fn(); } finally {
    const ms = performance.now() - a;
    current = prev; currentKey = prevKey;
    if (op === 'update') { w.updates++; w.total += ms; w.max = Math.max(w.max, ms); w.last = ms; }
    else if (op === 'mount') w.mount = ms; else w.dispose++;
    if (op !== 'update' || ms >= 1) log({ k: 'w', op, v: view, p: key, ms: +ms.toFixed(2) });
  }
}

/** paint() reports whether a redraw changed the DOM. */
export function painted(changed) {
  const w = widgets.get(currentKey);
  if (w) changed ? w.changed++ : w.unchanged++;
}

/** The state poll's size and parse time. */
export function statePoll(bytes, parseMs) { log({ k: 'st', bytes, ms: +parseMs.toFixed(2) }); }
/** A channel's value changed (lib/channels.js). */
export function channelPublished(name) { log({ k: 'ev', type: `ch:${name}`, by: caller() }); }

// The widget that called, from the stack (async code has left `current`).
function caller() {
  if (current) return current;
  // The nearest widget on the stack, else the nearest page module (lib/app.js: "app").
  const st = new Error().stack || '', w = st.match(/\/widgets\/([a-z]+)\.js/);
  if (w) return w[1];
  for (const m of st.matchAll(/\/lib\/([a-z]+)\.js/g)) if (m[1] !== 'profile') return m[1];
  const v = st.match(/\/vendor\/([a-z]+(?:-[a-z]+)*)/);
  return v ? v[1] : '';
}
const collapse = p => p.replace(/\/[A-Z][A-Z0-9.]{0,9}(?=\/|$)/g, '/:sym').replace(/\/p\d[\d-]*$/, '/:id');

function wrapFetch() {
  const native = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href);
    const by = caller(), a = performance.now();
    try {
      const r = await native(input, init);
      const ms = performance.now() - a, len = +(r.headers.get('content-length') || 0);
      const path = collapse(url.pathname), e = { k: 'f', path, ms: +ms.toFixed(1), status: r.status, by, bytes: len };
      // The body's parse, which runs on the main thread when the caller reads it.
      r.json = async () => { const text = await Response.prototype.text.call(r), b = performance.now(); const v = JSON.parse(text); const pm = performance.now() - b; if (pm >= 1) log({ k: 'p', path, by, bytes: text.length, ms: +pm.toFixed(1) }); return v; };
      if (len) log(e);
      // Without a length, the body's size (off the caller's path).
      else r.clone().blob().then(b => { e.bytes = b.size; log(e); }, () => log(e));
      return r;
    } catch (err) {
      log({ k: 'f', path: collapse(url.pathname), ms: +(performance.now() - a).toFixed(1), status: 0, by, bytes: 0, error: String(err) });
      throw err;
    }
  };
}

// Timer and animation-frame callbacks: timed when they take a millisecond or
// more, and attributed to the widget whose code scheduled them.
function wrapTimers() {
  const time = (owner, fn) => function (...a) {
    const prev = current, b = performance.now(); current = owner || current;
    try { return fn.apply(this, a); } finally {
      const ms = performance.now() - b; current = prev;
      if (ms >= 1) log({ k: 'w', op: 'timer', v: owner || '?', ms: +ms.toFixed(2) });
    }
  };
  for (const name of ['setTimeout', 'setInterval', 'requestAnimationFrame']) {
    const native = window[name].bind(window);
    window[name] = (fn, ...rest) => typeof fn === 'function' ? native(time(caller(), fn), ...rest) : native(fn, ...rest);
  }
}

function watchStalls() {
  const longtask = typeof PerformanceObserver !== 'undefined' && (PerformanceObserver.supportedEntryTypes || []).includes('longtask');
  if (longtask) new PerformanceObserver(l => { for (const e of l.getEntries()) log({ k: 's', ms: Math.round(e.duration), src: 'longtask', t: Math.round(e.startTime + e.duration - t0) }); }).observe({ type: 'longtask' });
  // Frame rate, and (without long tasks, as in Safari) gaps between frames.
  const frame = ts => {
    if (lastFrame && !document.hidden) {
      const gap = ts - lastFrame;
      if (!longtask && gap > 50) log({ k: 's', ms: Math.round(gap), src: 'frame-gap' });
    }
    lastFrame = ts; frames++;
    raf(frame);
  };
  raf(frame);
  document.addEventListener('visibilitychange', () => { lastFrame = 0; });
}

function watchPage() {
  addEventListener('error', e => log({ k: 'e', msg: String(e.message).slice(0, 300), at: `${e.filename || ''}:${e.lineno || ''}` }));
  addEventListener('unhandledrejection', e => log({ k: 'e', msg: String(e.reason?.message || e.reason).slice(0, 300) }));
  const what = el => { const t = /** @type {HTMLElement} */ (el); return t?.closest?.('[data-view]')?.getAttribute('data-view') || (t?.tagName || '').toLowerCase(); };
  addEventListener('click', e => log({ k: 'i', type: 'click', on: what(e.target), text: (/** @type {HTMLElement} */ (e.target)?.textContent || '').trim().slice(0, 30) }), true);
  addEventListener('keydown', e => log({ k: 'i', type: 'key', on: what(e.target) }), true);
  addEventListener('wheel', (() => { let last = 0; return e => { if (performance.now() - last > 500) { last = performance.now(); log({ k: 'i', type: 'wheel', on: what(e.target) }); } }; })(), { capture: true, passive: true });
  // tt-* events between widgets.
  const dispatch = document.dispatchEvent.bind(document);
  document.dispatchEvent = ev => { if (ev.type.startsWith('tt-')) log({ k: 'ev', type: ev.type, by: caller() }); return dispatch(ev); };
  every(() => {
    fps = frames / 5; frames = 0;
    const s = { k: 'm', dom: document.getElementsByTagName('*').length, fps: +fps.toFixed(1) };
    const mem = /** @type {any} */ (performance).memory;
    if (mem) s.heapMB = +(mem.usedJSHeapSize / 1e6).toFixed(1);
    samples.push(s); if (samples.length > 720) samples.shift();
    log(s);
  }, 5000);
}

// ---------- the bar and the badges ----------
let bar = null, badges = null;
const fmtKB = b => b >= 1e6 ? `${(b / 1e6).toFixed(1)} MB` : `${Math.round(b / 1e3)} KB`;
function drawBar() {
  const cut = now() - 60000;
  recent = recent.filter(e => e.t >= cut);
  const stalls = recent.filter(e => e.k === 's'), bytes = recent.filter(e => e.k === 'f').reduce((s, e) => s + (e.bytes || 0), 0);
  const s = samples[samples.length - 1];
  const slow = [...widgets.values()].filter(w => w.updates).sort((a, b) => b.total / b.updates - a.total / a.updates)[0];
  const errs = recent.filter(e => e.k === 'e').length;
  const recText = rec ? `<b class="pf-rec">● REC ${Math.round((now() - recStart) / 1000)} s · ${rec.length} events</b><button type="button" data-pf="stop">Stop and save</button>`
    : '<button type="button" data-pf="record">Record</button>';
  bar.innerHTML = `<b>Profile</b><span>${fps ? fps.toFixed(0) : '—'} fps</span>
    <span>${stalls.length} stalls/min${stalls.length ? ` (max ${Math.max(...stalls.map(x => x.ms))} ms)` : ''}</span>
    <span>fetched ${fmtKB(bytes)}/min</span><span>DOM ${s ? s.dom.toLocaleString() : '—'}</span>${s?.heapMB ? `<span>heap ${s.heapMB} MB</span>` : ''}
    ${slow ? `<span>slowest ${slow.view} ${(slow.total / slow.updates).toFixed(1)} ms/update</span>` : ''}${errs ? `<span class="pf-err">${errs} errors</span>` : ''}
    <span class="pf-gap"></span>${recText}<span class="pf-saved"></span><button type="button" data-pf="off" title="Turn profile mode off (reloads the page)">Turn off</button>`;
  if (saved) bar.querySelector('.pf-saved').innerHTML = saved;
  // Badges over each visible panel: its last and average update.
  const shown = new Set();
  for (const body of document.querySelectorAll('.tile-body')) {
    const key = /** @type {HTMLElement} */ (body).dataset.pfKey, w = key && widgets.get(key);
    const r = body.getBoundingClientRect();
    if (!w || !w.updates || !r.width || !r.height) continue;
    let b = badges.querySelector(`[data-for="${key}"]`);
    if (!b) { b = document.createElement('span'); b.className = 'pf-badge'; b.dataset.for = key; badges.appendChild(b); }
    b.textContent = `${w.last.toFixed(1)} ms · avg ${(w.total / w.updates).toFixed(1)} · ${w.unchanged ? Math.round(100 * w.unchanged / (w.changed + w.unchanged)) + '% skipped' : w.changed ? 'redrew' : ''}`;
    b.style.left = `${r.right - b.offsetWidth - 8}px`; b.style.top = `${r.top + 4}px`;
    shown.add(key);
  }
  for (const b of [...badges.children]) if (!shown.has(/** @type {HTMLElement} */ (b).dataset.for)) b.remove();
}

let saved = '';
async function stopRecording() {
  const events = rec || [];
  const recording = {
    meta: { ...recMeta, seconds: Math.round((now() - recStart) / 1000), events: events.length, dropped: recDropped, ended_at: new Date().toISOString() },
    widgets: Object.fromEntries([...widgets].map(([k, w]) => [k, { ...w, avg: w.updates ? +(w.total / w.updates).toFixed(2) : 0 }])),
    minutes: perMinute(events),
    events,
  };
  rec = null;
  const body = JSON.stringify(recording);
  const url = URL.createObjectURL(new Blob([body], { type: 'application/json' }));
  const name = `tinytrader-profile-${recording.meta.started_at.replace(/[:.]/g, '-')}.json`;
  let where = '';
  try {
    const r = await fetch('/api/profiles', { method: 'POST', headers: { 'content-type': 'application/json' }, body });
    const v = await r.json().catch(() => ({}));
    where = r.ok ? `saved as ${v.id}` : `not saved (${v.error || r.status})`;
  } catch (e) { where = `not saved (${e.message})`; }
  saved = `${where} · <a href="${url}" download="${name}">download</a>`;
  drawBar();
}
// Totals for each minute of a recording.
function perMinute(events) {
  const out = [];
  for (const e of events) {
    const m = Math.floor((e.t - (events[0]?.t || 0)) / 60000);
    const o = out[m] ||= { minute: m, updates: 0, update_ms: 0, fetches: 0, bytes: 0, stalls: 0, stall_ms: 0, errors: 0, dom: 0 };
    if (e.k === 'w' && e.op === 'update') { o.updates++; o.update_ms += e.ms; }
    if (e.k === 'f') { o.fetches++; o.bytes += e.bytes || 0; }
    if (e.k === 's') { o.stalls++; o.stall_ms += e.ms; }
    if (e.k === 'e') o.errors++;
    if (e.k === 'm') o.dom = e.dom;
  }
  return out.filter(Boolean);
}

/** Turns profile mode on for this tab. */
export function startProfile() {
  if (prof.on) return;
  prof.on = true;
  try { sessionStorage.setItem(KEY, '1'); } catch (_) {}
  watchStalls(); wrapFetch(); wrapTimers(); watchPage();
  bar = document.createElement('div'); bar.className = 'pf-bar'; bar.setAttribute('role', 'status');
  badges = document.createElement('div'); badges.className = 'pf-badges'; badges.setAttribute('aria-hidden', 'true');
  document.body.append(bar, badges);
  document.body.classList.add('pf-on');
  bar.addEventListener('click', e => {
    const b = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (e.target).closest('[data-pf]')); if (!b) return;
    if (b.dataset.pf === 'record') {
      rec = []; recBytes = 0; recDropped = 0; recStart = now(); saved = '';
      recMeta = { started_at: new Date().toISOString(), user_agent: navigator.userAgent, viewport: `${innerWidth}x${innerHeight}`, dpr: devicePixelRatio, url: location.pathname + location.hash,
        panels: [...document.querySelectorAll('.tile-body')].map(x => /** @type {HTMLElement} */ (x).dataset.view).filter(Boolean) };
      drawBar();
    } else if (b.dataset.pf === 'stop') stopRecording();
    else if (b.dataset.pf === 'off') stopProfile();
  });
  every(drawBar, 1000);
  drawBar();
}

/** Turns it off: reloads, so nothing stays wrapped. */
export function stopProfile() {
  try { sessionStorage.removeItem(KEY); } catch (_) {}
  if (location.hash === '#profile') history.replaceState(null, '', location.pathname);
  location.reload();
}
