// ---------- render & polling ----------
// Databento jobs, through the console's /api/jobs proxy to the jobs service:
// a request form that prices before it submits, and every job through to
// filed in the store. The form is drawn once; only the list refreshes.
import { defineWidget } from '../lib/widgets.js';
import { jobsApi } from '../lib/api.js';
import { esc, bytes } from '../lib/format.js';

// What every Jobs panel shares: the job list and the provider's datasets and
// schemas. The list refreshes every 2 s while a panel is open.
const shared = { list: null, err: null, datasets: null, schemas: {} };
const panels = new Set();
let seq = 0;
async function loadJobs() {
  try { shared.list = await jobsApi('jobs'); shared.err = null; } catch (e) { shared.err = e.message; }
  for (const J of panels) J.redrawList();
}
setInterval(() => { if (panels.size) loadJobs(); }, 2000);
const usd = n => '$' + (n ?? 0).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const day = t => new Date(t * 1000).toISOString().slice(0, 10);
function jobsView(J) {
  const f = J.form, stypes = [['parent', 'parent (SPX.OPT)'], ['raw_symbol', 'raw symbol'], ['continuous', 'continuous'], ['instrument_id', 'instrument ID']];
  return `<div><h1>Jobs</h1><p class="muted" style="margin:0">Request historical data from a provider and download it into the store. Every request is priced first; anything that costs money needs your confirmation of the exact amount.</p></div>
  <div class="field" style="max-width:260px"><label for="${J.uid}-jf-provider">Provider</label><select id="${J.uid}-jf-provider"><option value="databento" selected>Databento</option></select></div>
  <section class="panel" id="${J.uid}-job-form-panel">
    <div class="sec-head" style="margin:0"><h2>New request</h2></div>
    <form class="${J.uid}-job-form" id="${J.uid}-job-form" autocomplete="off">
      <div class="field"><label for="${J.uid}-jf-dataset">Dataset</label><input id="${J.uid}-jf-dataset" name="dataset" list="${J.uid}-jf-datasets" value="${esc(f.dataset)}" spellcheck="false"><datalist id="${J.uid}-jf-datasets">${(shared.datasets || []).map(d => `<option value="${esc(d)}">`).join('')}</datalist></div>
      <div class="field"><label for="${J.uid}-jf-schema">Schema</label><select id="${J.uid}-jf-schema" name="schema"><option>${esc(f.schema)}</option></select></div>
      <div class="field" style="grid-column: span 2"><label for="${J.uid}-jf-symbols">Symbol</label><input id="${J.uid}-jf-symbols" name="symbols" value="${esc(f.symbols)}" placeholder="SPX.OPT" spellcheck="false"></div>
      <div class="field"><label for="${J.uid}-jf-stype">Symbol type</label><select id="${J.uid}-jf-stype" name="stype_in">${stypes.map(([v, l]) => `<option value="${v}"${v === f.stype_in ? ' selected' : ''}>${l}</option>`).join('')}</select></div>
      <div class="field"><label for="${J.uid}-jf-start">First day</label><input id="${J.uid}-jf-start" name="start" type="date" value="${esc(f.start)}"></div>
      <div class="field"><label for="${J.uid}-jf-end">Last day</label><input id="${J.uid}-jf-end" name="end" type="date" value="${esc(f.end)}"></div>
    </form>
    <div class="quote" id="${J.uid}-job-quote">${quoteLine(J)}</div>
  </section>
  <section><div class="sec-head"><h2>At Databento</h2><span class="muted" style="font-size:13px">Not yet expired at the provider; those needing attention first</span></div><div id="${J.uid}-jobs-list">${jobsTable(J)}</div></section>`;
}
function quoteLine(J) {
  const q = J.quote, current = q && J.quoteFor === JSON.stringify(J.form);
  const msg = J.msg ? `<span class="${J.msg.ok ? 'msg-ok' : 'msg-err'}">${esc(J.msg.text)}</span>` : '';
  const price = `<button type="button" class="btn${current ? '' : ' primary'}" data-jf-price${J.busy ? ' disabled' : ''}>${J.busy === 'price' ? 'Pricing…' : 'Price'}</button>`;
  if (!current) return price + msg;
  const paid = q.cost_usd > 0;
  const warn = (q.warnings || []).length ? `<span class="msg-warn">⚠ ${q.warnings.map(esc).join(' · ')}</span>` : '';
  return `${price}<span class="cost">${usd(q.cost_usd)}</span><span class="muted">${paid ? 'charged to the Databento account' : 'included, no charge'} · ${bytes(q.billable_bytes)} billable (uncompressed)</span>
    ${paid ? `<label><input type="checkbox" data-jf-confirm> Charge ${usd(q.cost_usd)}</label>` : ''}
    <button type="button" class="btn primary" data-jf-submit${paid || J.busy ? ' disabled' : ''}>${J.busy === 'submit' ? 'Submitting…' : paid ? 'Submit for ' + usd(q.cost_usd) : 'Submit'}</button>${warn}${msg}`;
}
function jobState(j) {
  const t = j.transfer, h = j.held;
  if (t && t.state === 'downloading') {
    const pct = t.total_bytes ? 100 * t.done_bytes / t.total_bytes : 0;
    return [`<span class="pill p-idle">Downloading ${pct.toFixed(0)}%</span>`, `${esc(t.file)} · ${bytes(t.done_bytes)} of ${bytes(t.total_bytes)}<div class="bar"><b style="width:${pct.toFixed(1)}%"></b></div>`];
  }
  if (t && t.state === 'waiting') return ['<span class="pill p-idle">Waiting to download</span>', ''];
  if (t && t.state === 'filing') return ['<span class="pill p-idle">Filing</span>', 'Checking every file against the manifest and filing it in the store'];
  if (t && t.state === 'failed') return [`<span class="pill p-crit">Failed</span> <button type="button" class="btn small" data-job-download="${esc(j.id)}">Retry</button>`, esc(t.error || '')];
  if (h && h.how === 'filed') return [`<span class="pill p-ok" title="Filed on ${esc(h.volume)}">In the store</span>`, ''];
  if (h && h.how === 'folder') return [`<span class="pill p-ok" title="Downloaded to a folder on ${esc(h.volume)}, not yet filed in the store layout">Downloaded</span>`, ''];
  if (h && h.how === 'incoming') return [`<span class="pill p-warn">Partly downloaded</span> <button type="button" class="btn small" data-job-download="${esc(j.id)}">Resume</button>`, ''];
  if (j.state === 'done' && j.unfileable) return ['<span class="pill p-idle">Not fileable</span>', `Can't be filed in the store: ${esc(j.unfileable)}`];
  if (j.state === 'done') return [`<span class="pill p-warn">Ready</span> <button type="button" class="btn small" data-job-download="${esc(j.id)}">Download</button>`, j.expires ? `Available until ${day(j.expires)}` : ''];
  if (j.state === 'expired') return ['<span class="pill p-idle">Expired</span>', ''];
  const what = j.state === 'queued' ? 'Queued' : `Processing${j.progress != null ? ' ' + j.progress + '%' : ''}`;
  return [`<span class="pill p-idle">${what}</span>`, j.follow ? 'Downloads into the store when done' : ''];
}
function jobsTable(J) {
  if (shared.err) return `<div class="box empty msg-err">${esc(shared.err)}</div>`;
  if (!shared.list || shared.list.updated_at == null) return '<div class="box empty">Loading jobs…</div>';
  // Jobs that need something (or are moving) first, then the rest; newest first within each.
  const settled = j => !!j.held && !j.transfer || j.state === 'expired';
  const all = (shared.list.jobs || []).slice().sort((a, b) => settled(a) - settled(b) || b.received - a.received);
  // Jobs already on disk are history; they're shown only on request.
  const onDisk = all.filter(j => j.held && !j.transfer).length;
  const list = J.showHeld ? all : all.filter(j => !(j.held && !j.transfer));
  const toggle = onDisk ? `<button type="button" class="btn small" data-jobs-held>${J.showHeld ? 'Hide' : 'Show'} ${onDisk} already on disk</button>` : '';
  if (!list.length) return `<div class="box empty">Nothing waiting: every job at Databento is already on disk.</div>${toggle}`;
  const rows = list.map(j => {
    let [st, sub] = jobState(j);
    if (J.rowErr[j.id] && !j.transfer) sub = `<span class="msg-err">${esc(J.rowErr[j.id])}</span>`;
    const last = day(j.end - 1), range = day(j.start) === last ? day(j.start) : `${day(j.start)} → ${last}`;
    return `<tr${sub ? ' class="has-sub"' : ''}><td class="mono" style="color:var(--fg)">${esc(j.id)}</td>
      <td class="mono">${esc(j.symbols)} <span class="muted">${esc(j.schema)}</span><div class="muted" style="font-size:12px">${esc(j.dataset)}</div></td>
      <td class="mono" style="white-space:nowrap">${range}</td>
      <td class="num mono">${j.bytes != null ? bytes(j.bytes) : '—'}</td>
      <td class="num mono">${j.cost_usd != null ? usd(j.cost_usd) : '—'}</td>
      <td style="white-space:nowrap">${st}</td></tr>${sub ? `<tr class="sub"><td></td><td colspan="5">${sub}</td></tr>` : ''}`;
  }).join('');
  return `<div class="box scroll"><table class="jobs-tbl"><thead><tr><th>Job</th><th>Request</th><th>Range</th><th style="text-align:right">Size</th><th style="text-align:right">Cost</th><th>State</th></tr></thead><tbody>${rows}</tbody></table></div>
    ${toggle}
    ${shared.list.store_reachable === false ? '<p class="msg-err">The store is not reachable, so what it already holds is not shown.</p>' : ''}`;
}

defineWidget('jobs', {
  title: 'Jobs',
  mount(el) {
    const J = { uid: 'jobs' + seq++, form: { dataset: 'OPRA.PILLAR', schema: 'cmbp-1', symbols: '', stype_in: 'parent', start: '', end: '' },
      quote: null, quoteFor: '', msg: null, busy: false, showHeld: false, rowErr: {} };
    const $$ = id => el.querySelector('#' + J.uid + '-' + id);
    const redrawQuote = () => { const q = $$('job-quote'); if (q) q.innerHTML = quoteLine(J); };
    J.redrawList = () => { const l = $$('jobs-list'); if (l) l.innerHTML = jobsTable(J); };
    function fillSchemas() {
      const sel = $$('jf-schema'); if (!sel) return;
      const list = shared.schemas[J.form.dataset] || [J.form.schema];
      if (!list.includes(J.form.schema)) J.form.schema = list.includes('cmbp-1') ? 'cmbp-1' : list[0];
      sel.innerHTML = list.map(x => `<option${x === J.form.schema ? ' selected' : ''}>${esc(x)}</option>`).join('');
    }
    async function loadSchemas(ds) {
      if (ds && !(ds in shared.schemas)) { try { shared.schemas[ds] = (await jobsApi('schemas?dataset=' + encodeURIComponent(ds))).schemas; } catch (_) { shared.schemas[ds] = null; } }
      fillSchemas();
    }
    const input = e => {
      const t = e.target; if (!t.closest('form')) return;
      J.form[t.name] = t.value; J.msg = null;
      if (t.name === 'symbols' && /\.(OPT|FUT)\b/i.test(t.value)) { J.form.stype_in = 'parent'; $$('jf-stype').value = 'parent'; }
      if (t.name === 'start' && (!J.form.end || J.form.end < t.value)) { J.form.end = t.value; $$('jf-end').value = t.value; }
      if (t.name === 'dataset') loadSchemas(t.value.trim());
      redrawQuote();
    };
    const change = e => {
      if (e.target.matches('[data-jf-confirm]')) { const b = el.querySelector('[data-jf-submit]'); if (b) b.disabled = !e.target.checked || !!J.busy; return; }
      if (e.target.closest('form')) { J.form[e.target.name] = e.target.value; redrawQuote(); }
    };
    const click = async e => {
      if (e.target.closest('[data-jf-price]')) {
        J.busy = 'price'; J.msg = null; redrawQuote();
        const snapshot = JSON.stringify(J.form);
        try { J.quote = await jobsApi('price', J.form); J.quoteFor = snapshot; }
        catch (err) { J.quote = null; J.msg = { text: err.message }; }
        J.busy = false; redrawQuote(); return;
      }
      if (e.target.closest('[data-jf-submit]')) {
        const q = J.quote; if (!q) return;
        if (q.cost_usd > 0 && !el.querySelector('[data-jf-confirm]')?.checked) return;
        J.busy = 'submit'; redrawQuote();
        try {
          const r = await jobsApi('submit', { quote: q.quote, confirm_usd: q.cost_usd > 0 ? q.cost_usd : null });
          J.msg = { ok: true, text: `Submitted ${r.job}; it downloads into the store when done.` };
        } catch (err) { J.msg = { text: err.message }; }
        J.quote = null; J.busy = false; redrawQuote(); loadJobs(); return;
      }
      if (e.target.closest('[data-jobs-held]')) { J.showHeld = !J.showHeld; J.redrawList(); return; }
      const d = e.target.closest('[data-job-download]');
      if (d) {
        d.disabled = true; delete J.rowErr[d.dataset.jobDownload];
        try { await jobsApi('download/' + encodeURIComponent(d.dataset.jobDownload), {}); }
        catch (err) { J.rowErr[d.dataset.jobDownload] = err.message; }
        loadJobs();
      }
    };
    el.addEventListener('input', input); el.addEventListener('change', change); el.addEventListener('click', click);
    // The form is drawn once; only the quote and the list redraw.
    el.innerHTML = jobsView(J);
    panels.add(J); loadJobs(); loadSchemas(J.form.dataset);
    if (!shared.datasets) jobsApi('datasets').then(v => { shared.datasets = v.datasets; }, () => { shared.datasets = []; }).then(() => {
      for (const P of panels) { const dl = el.ownerDocument.getElementById(P.uid + '-jf-datasets'); if (dl) dl.innerHTML = shared.datasets.map(x => `<option value="${esc(x)}">`).join(''); }
    });
    return {
      dispose() { panels.delete(J); el.removeEventListener('input', input); el.removeEventListener('change', change); el.removeEventListener('click', click); },
    };
  },
});
