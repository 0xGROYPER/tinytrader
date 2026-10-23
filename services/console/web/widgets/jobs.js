// ---------- render & polling ----------
// Databento jobs, through the console's /api/jobs proxy to the jobs service:
// a request form that prices only what's missing before anything is bought,
// the purchases awaiting a person's approval, and every job through to filed
// in the store. A free request is submitted from the form; a paid one waits
// in the approval list, where Approve (confirmed with a second click, and
// sent with the order token as a live order is) charges exactly the quoted
// price, and Decline drops the quote. The form is drawn once; only the
// quote, the approval list and the job list redraw.
import { defineWidget } from '../lib/widgets.js';
import { jobsApi, orderFetch, refusedText } from '../lib/api.js';
import { esc, bytes } from '../lib/format.js';

// What every Jobs panel shares: the job list and the provider's datasets and
// schemas. The list refreshes every 2 s while a panel is open.
const shared = { list: null, err: null, datasets: null, schemas: {}, quotes: null, qErr: null };
const panels = new Set();
let seq = 0;
async function loadJobs() {
  try { shared.list = await jobsApi('jobs'); shared.err = null; } catch (e) { shared.err = e.message; }
  try { shared.quotes = (await jobsApi('quotes')).quotes; shared.qErr = null; } catch (e) { shared.qErr = e.message; }
  for (const J of panels) { J.redrawList(); J.redrawApprovals(); }
}
setInterval(() => { if (panels.size) loadJobs(); }, 2000);
const usd = n => '$' + (n ?? 0).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const day = t => new Date(t * 1000).toISOString().slice(0, 10);
/** A request window `[start, end]` as the jobs service stamps it, briefly. @param {string[]} w */
const span = w => (w || []).map(x => String(x).replace('T', ' ').replace(/:00\.0+Z$|\.0+Z$/, 'Z')).join(' → ');
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
function jobsView(J) {
  const f = J.form, stypes = [['parent', 'parent (SPX.OPT)'], ['raw_symbol', 'raw symbol']];
  return `<div><h1>Jobs</h1><p class="muted" style="margin:0">Request historical data from a provider and download it into the store. Every request is priced first, and only what the store, the provider's jobs and earlier purchases don't already have is bought. Anything that costs money waits for your approval below.</p></div>
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
  <section><div class="sec-head"><h2>Awaiting approval</h2><span class="muted" style="font-size:13px">Paid purchases; each charges exactly its quoted price</span></div><div id="${J.uid}-jobs-approvals">${approvals(J)}</div></section>
  <section><div class="sec-head"><h2>At Databento</h2><span class="muted" style="font-size:13px">Not yet expired at the provider; those needing attention first</span></div><div id="${J.uid}-jobs-list">${jobsTable(J)}</div></section>`;
}
function quoteLine(J) {
  const q = J.quote, current = q && J.quoteFor === JSON.stringify(J.form);
  const msg = J.msg ? `<span class="${J.msg.ok ? 'msg-ok' : 'msg-err'}">${esc(J.msg.text)}</span>` : '';
  const price = `<button type="button" class="btn${current ? '' : ' primary'}" data-jf-price${J.busy ? ' disabled' : ''}>${J.busy === 'price' ? 'Pricing…' : 'Price'}</button>`;
  if (!current) return price + msg;
  if (q.state === 'pricing') return `${price}<span class="muted">Pricing: ${q.priced ?? 0} of ${q.of || '…'} requests…</span>${msg}`;
  const reqs = q.requests || [], paid = q.cost_usd !== 0;
  const notes = (q.notes || []).length ? `<span class="msg-warn">⚠ ${q.notes.slice(0, 4).map(esc).join(' · ')}${q.notes.length > 4 ? ` · ${q.notes.length - 4} more` : ''}</span>` : '';
  if (!reqs.length) return `${price}<span class="muted">Nothing to buy: everything asked for is already had.</span>${notes}${msg}`;
  const what = `${plural(reqs.length, 'request', 'requests')} · ${bytes(q.billable_bytes)} billable (uncompressed)`;
  return `${price}<span class="cost">${usd(q.cost_usd)}</span><span class="muted">${what}</span>
    ${paid ? '<span class="muted">Waits for approval below</span>' : `<button type="button" class="btn primary" data-jf-submit${J.busy ? ' disabled' : ''}>${J.busy === 'submit' ? 'Submitting…' : 'Submit (no charge)'}</button>`}${notes}${msg}`;
}
/** One request row of a quote. */
function requestLine(r) {
  const days = r.first_day === r.last_day ? r.first_day : `${r.first_day} → ${r.last_day}`;
  const what = r.kind === 'whole' ? esc(r.symbol) : `${plural(r.contracts, 'contract', 'contracts')}${r.skipped_held || r.skipped_pending ? ` <span class="muted">(skipped ${r.skipped_held} held, ${r.skipped_pending} pending or bought)</span>` : ''}`;
  return `<tr><td class="mono">${days}</td><td class="mono">${what}</td><td class="mono">${r.hours ? esc(span(r.window)) : 'whole days'}</td><td class="num mono">${usd(r.cost_usd)}</td></tr>`;
}
/** The quotes awaiting a person's approval (and any still being priced). */
function approvals(J) {
  if (shared.qErr) return `<div class="box empty msg-err">${esc(shared.qErr)}</div>`;
  if (!shared.quotes) return '<div class="box empty">Loading…</div>';
  const list = shared.quotes.filter(q => q.state === 'pricing' || (q.state === 'ready' && q.needs_approval));
  if (!list.length) return '<div class="box empty">Nothing waits for approval.</div>';
  return list.map(q => {
    const head = `<b class="mono">${esc(q.dataset)} ${esc(q.schema)} ${esc(q.symbol)}</b> <span class="mono">${esc(q.start)}${q.end !== q.start ? ' → ' + esc(q.end) : ''}</span>`;
    if (q.state === 'pricing') return `<div class="box" style="margin-bottom:8px">${head}<div class="muted">Pricing: ${q.priced} of ${q.of || '…'} requests</div></div>`;
    const a = J.acts[q.quote] || {}, reqs = q.requests || [];
    const asked = [q.contracts != null ? `${q.contracts.toLocaleString()} contracts over ${plural(q.contract_days, 'day', 'days')}` : q.rule ? 'contracts by rule' : 'whole chain or symbol',
      q.hours ? `hours ${esc(q.hours.start)}–${esc(q.hours.end)} ${esc(q.hours.tz)}` : 'whole days',
      (q.again || []).length ? `buys again ${q.again.map(esc).join(', ')}` : ''].filter(Boolean).join(' · ');
    const shown = reqs.slice(0, 12).map(requestLine).join('');
    const more = reqs.length > 12 ? `<tr><td colspan="4" class="muted">and ${reqs.length - 12} more requests</td></tr>` : '';
    const notes = (q.notes || []).length ? `<details><summary class="muted">${plural(q.notes.length, 'note', 'notes')}: what was skipped, and why</summary><ul>${q.notes.map(n => `<li>${esc(n)}</li>`).join('')}</ul></details>` : '';
    const confirming = a.confirm > Date.now();
    const approve = a.busy ? '<span class="muted">Sending…</span>'
      : `<button type="button" class="btn primary${confirming ? ' danger' : ''}" data-q-approve="${esc(q.quote)}">${confirming ? `Confirm: charge ${usd(q.cost_usd)}` : `Approve ${usd(q.cost_usd)}`}</button>${confirming ? `<button type="button" class="btn" data-q-unconfirm="${esc(q.quote)}">Not yet</button>` : ''}
        <button type="button" class="btn" data-q-decline="${esc(q.quote)}">Decline</button>`;
    return `<div class="box" style="margin-bottom:8px">
      <div style="display:flex;flex-wrap:wrap;gap:6px 12px;align-items:baseline">${head}<span class="cost">${usd(q.cost_usd)}</span><span class="muted">${plural(reqs.length, 'request', 'requests')} · ${bytes(q.billable_bytes)} billable · expires in ${Math.max(0, Math.round(q.valid_seconds / 60))} min</span></div>
      <div class="muted" style="font-size:13px">${asked}</div>
      <div class="scroll"><table class="jobs-tbl"><thead><tr><th>Days</th><th>What</th><th>Window (UTC)</th><th style="text-align:right">Cost</th></tr></thead><tbody>${shown}${more}</tbody></table></div>
      ${notes}
      <div style="display:flex;flex-wrap:wrap;gap:8px;align-items:center;margin-top:6px">${approve}${a.msg ? `<span class="${a.ok ? 'msg-ok' : 'msg-err'}">${esc(a.msg)}</span>` : ''}</div>
    </div>`;
  }).join('');
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
  const all = (shared.list.jobs || []).slice().sort((a, b) => Number(settled(a)) - Number(settled(b)) || b.received - a.received);
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
      quote: null, quoteFor: '', msg: null, busy: /** @type {false | 'price' | 'submit'} */ (false), showHeld: false, rowErr: {},
      /** Approve or Decline in progress, by quote: { confirm (until, ms), busy, msg, ok }. @type {Record<string, any>} */ acts: {} };
    /** @type {(id: string) => HTMLInputElement | null} */
    const $$ = id => el.querySelector('#' + J.uid + '-' + id);
    const redrawQuote = () => { const q = $$('job-quote'); if (q) q.innerHTML = quoteLine(J); };
    J.redrawList = () => { const l = $$('jobs-list'); if (l) l.innerHTML = jobsTable(J); };
    J.redrawApprovals = () => { const l = $$('jobs-approvals'); if (l) l.innerHTML = approvals(J); };
    /** An approval or a decline through the console's write gate, with the order token as for a live order. */
    async function act(id, what, body) {
      /** @type {{ busy: boolean, msg?: string, ok?: boolean }} */
      const a = J.acts[id] = { busy: true }; J.redrawApprovals();
      try {
        const r = await orderFetch(`/api/job-quotes/${encodeURIComponent(id)}/${what}`, body, 'POST', {}, true);
        let v = null; try { v = await r.json(); } catch (_) {}
        if (!r.ok) { a.msg = refusedText(r, v); }
        else { a.ok = true; a.msg = what === 'decline' ? 'Declined.' : `Submitted ${(v.submitted || []).map(x => x.job).join(', ')}; it downloads into the store when done.`; }
      } catch (err) { a.msg = err.message; }
      a.busy = false; J.redrawApprovals(); loadJobs();
    }
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
      if (e.target.closest('form')) { J.form[e.target.name] = e.target.value; redrawQuote(); }
    };
    const click = async e => {
      if (e.target.closest('[data-jf-price]')) {
        J.busy = 'price'; J.msg = null; redrawQuote();
        const snapshot = JSON.stringify(J.form);
        try {
          J.quote = await jobsApi('price', J.form); J.quoteFor = snapshot;
          // A long price is worked out after the answer: follow it.
          while (J.quote.state === 'pricing' && J.quoteFor === snapshot) {
            redrawQuote(); await new Promise(res => setTimeout(res, 2000));
            J.quote = await jobsApi('quotes/' + encodeURIComponent(J.quote.quote));
          }
          if (J.quote.state === 'failed') throw new Error(J.quote.error);
        } catch (err) { J.quote = null; J.msg = { text: err.message }; }
        J.busy = false; redrawQuote(); loadJobs(); return;
      }
      if (e.target.closest('[data-jf-submit]')) {
        // Free requests only: a paid one is approved from the list below.
        const q = J.quote; if (!q || q.cost_usd !== 0) return;
        J.busy = 'submit'; redrawQuote();
        try {
          const r = await jobsApi('submit', { quote: q.quote });
          J.msg = { ok: true, text: `Submitted ${(r.submitted || []).map(x => x.job).join(', ')}; it downloads into the store when done.` };
        } catch (err) { J.msg = { text: err.message }; }
        J.quote = null; J.busy = false; redrawQuote(); loadJobs(); return;
      }
      const ap = e.target.closest('[data-q-approve]');
      if (ap) {
        const id = ap.dataset.qApprove, q = (shared.quotes || []).find(x => x.quote === id), a = J.acts[id] || (J.acts[id] = {});
        if (!q) return;
        // A second click within 6 s confirms, as for a live order.
        if (!(a.confirm > Date.now())) { a.confirm = Date.now() + 6000; a.msg = ''; J.redrawApprovals(); setTimeout(J.redrawApprovals, 6100); return; }
        a.confirm = 0;
        act(id, 'approve', { confirm_usd: q.cost_usd, again: q.again });
        return;
      }
      const un = e.target.closest('[data-q-unconfirm]');
      if (un) { const a = J.acts[un.dataset.qUnconfirm]; if (a) a.confirm = 0; J.redrawApprovals(); return; }
      const de = e.target.closest('[data-q-decline]');
      if (de) { act(de.dataset.qDecline, 'decline', {}); return; }
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
