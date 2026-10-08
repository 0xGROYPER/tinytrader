// Queue: the job queue (decision 0012), through the console's /api/queue
// proxy. Jobs waiting, running and finished, with what each waits for; a
// waiting job can be edited (start time, condition) or run now, a running
// one cancelled, any one's log read. New jobs come only from the
// deployment's templates, with their parameters. Times are New York time.
import { defineWidget } from '../lib/widgets.js';
import { queueApi } from '../lib/api.js';
import { esc, duration } from '../lib/format.js';

// What every Queue panel shares: the jobs and templates, refreshed every 3 s
// while a panel is open.
const shared = { jobs: null, templates: null, err: null };
const panels = new Set();
let seq = 0;
async function load() {
  try {
    shared.jobs = (await queueApi('jobs')).jobs;
    if (!shared.templates) shared.templates = (await queueApi('templates')).templates;
    shared.err = null;
  } catch (e) { shared.err = e.message; }
  for (const Q of panels) Q.redraw();
}
setInterval(() => { if (panels.size) load(); }, 3000);

const NY = 'America/New_York';
const nyText = t => t == null ? '—' : new Date(t * 1000).toLocaleString('en-US', { timeZone: NY, month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false });
// `YYYY-MM-DDTHH:MM` as New York wall time, to Unix seconds.
function nyToUnix(s) {
  const [d, t] = s.split('T'); const [y, m, dd] = d.split('-').map(Number); const [h, mi] = t.split(':').map(Number);
  const guess = Date.UTC(y, m - 1, dd, h, mi) / 1000;
  const parts = Object.fromEntries(new Intl.DateTimeFormat('en-US', { timeZone: NY, hourCycle: 'h23', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    .formatToParts(new Date(guess * 1000)).map(p => [p.type, p.value]));
  const shown = Date.UTC(+parts.year, +parts.month - 1, +parts.day, +parts.hour, +parts.minute) / 1000;
  return guess + (guess - shown);
}
// Unix seconds as the `datetime-local` value in New York time.
function nyInput(t) {
  const p = Object.fromEntries(new Intl.DateTimeFormat('en-US', { timeZone: NY, hourCycle: 'h23', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    .formatToParts(new Date(t * 1000)).map(x => [x.type, x.value]));
  return `${p.year}-${p.month}-${p.day}T${p.hour}:${p.minute}`;
}
const WHEN = [['anytime', 'Any time'], ['after-close', 'After the close'], ['background-may-run', 'When background work may run']];
const whenLabel = w => (WHEN.find(x => x[0] === w) || [w, w])[1];
const PILL = { Waiting: 'p-idle', Pending: 'p-idle', Running: 'p-warn', Succeeded: 'p-ok', Failed: 'p-crit', Cancelled: 'p-idle' };

function conditions(j) {
  const c = [];
  if (j.notBefore) c.push('not before ' + nyText(j.notBefore));
  if (j.when && j.when !== 'anytime') c.push(whenLabel(j.when).toLowerCase());
  if (j.after?.length) c.push('after ' + j.after.join(', '));
  return c.join(' · ') || 'none';
}

function row(Q, j) {
  const params = Object.entries(j.params || {}).map(([k, v]) => `<span class="muted">${esc(k)}=</span>${esc(v)}`).join(' ');
  const took = j.started ? duration((j.finished ?? Date.now() / 1000) - j.started) : '';
  const acts = [];
  if (j.state === 'Waiting') acts.push(`<button type="button" class="btn small" data-q-run="${esc(j.id)}">Run now</button>`, `<button type="button" class="btn small" data-q-edit="${esc(j.id)}">Edit</button>`);
  if (!['Succeeded', 'Failed', 'Cancelled'].includes(j.state)) acts.push(`<button type="button" class="btn small" data-q-cancel="${esc(j.id)}"${j.cancelRequested ? ' disabled' : ''}>${j.cancelRequested ? 'Stopping…' : 'Cancel'}</button>`);
  acts.push(`<button type="button" class="btn small" data-q-log="${esc(j.id)}">${Q.log === j.id ? 'Hide log' : 'Log'}</button>`);
  let sub = '';
  if (Q.editing === j.id) {
    sub = `<div class="q-edit"><label>Not before (ET) <input type="datetime-local" data-q-nb value="${j.notBefore ? nyInput(j.notBefore) : ''}"></label>
      <label>When <select data-q-when>${WHEN.map(([v, l]) => `<option value="${v}"${v === j.when ? ' selected' : ''}>${l}</option>`).join('')}</select></label>
      <button type="button" class="btn small primary" data-q-save="${esc(j.id)}">Save</button> <button type="button" class="btn small" data-q-edit="">Close</button></div>`;
  }
  if (Q.log === j.id) sub += `<pre class="q-log mono">${esc(Q.logText || 'Loading…')}</pre>`;
  if (Q.rowErr[j.id]) sub += `<div class="msg-err">${esc(Q.rowErr[j.id])}</div>`;
  return `<tr${sub ? ' class="has-sub"' : ''}><td class="mono" style="color:var(--fg)">${esc(j.id)}<div class="muted" style="font-size:12px">${esc(j.runner)}${j.addedBy ? ' · by ' + esc(j.addedBy) : ''}</div></td>
    <td class="mono">${params || '<span class="muted">—</span>'}</td>
    <td>${esc(conditions(j))}</td>
    <td style="white-space:nowrap"><span class="pill ${PILL[j.state] || 'p-idle'}">${esc(j.state)}</span><div class="muted" style="font-size:12px">${esc(j.reason || '')}${j.exitCode != null && j.state !== 'Succeeded' ? ` (exit ${j.exitCode})` : ''}</div></td>
    <td class="mono" style="white-space:nowrap">${nyText(j.started ?? j.created)}${took ? `<div class="muted" style="font-size:12px">${took}</div>` : ''}</td>
    <td style="white-space:nowrap">${acts.join(' ')}</td></tr>${sub ? `<tr class="sub"><td colspan="6">${sub}</td></tr>` : ''}`;
}

function list(Q) {
  if (shared.err) return `<div class="box empty msg-err">${esc(shared.err)}</div>`;
  if (!shared.jobs) return '<div class="box empty">Loading the queue…</div>';
  const open = shared.jobs.filter(j => !['Succeeded', 'Failed', 'Cancelled'].includes(j.state));
  const done = shared.jobs.filter(j => ['Succeeded', 'Failed', 'Cancelled'].includes(j.state));
  const table = (rows, empty) => rows.length
    ? `<div class="box scroll"><table class="jobs-tbl"><thead><tr><th>Job</th><th>Parameters</th><th>Conditions</th><th>State</th><th>Started</th><th></th></tr></thead><tbody>${rows.map(j => row(Q, j)).join('')}</tbody></table></div>`
    : `<div class="box empty">${empty}</div>`;
  return `<section><div class="sec-head"><h2>Waiting and running</h2></div>${table(open, 'Nothing queued.')}</section>
    <section><div class="sec-head"><h2>Finished</h2><span class="muted" style="font-size:13px">Newest first</span></div>${table(done.slice(0, Q.showAll ? 100 : 15), 'Nothing yet.')}
    ${done.length > 15 ? `<button type="button" class="btn small" data-q-all>${Q.showAll ? 'Show fewer' : `Show all ${done.length}`}</button>` : ''}</section>`;
}

function addForm(Q) {
  const ts = shared.templates || [];
  if (!ts.length) return '';
  const t = ts.find(x => x.name === Q.form.template) || ts[0];
  Q.form.template = t.name;
  const params = Object.entries(t.parameters || {}).map(([k, p]) => `<div class="field"><label for="${Q.uid}-p-${esc(k)}">${esc(k)}</label><input id="${Q.uid}-p-${esc(k)}" data-q-param="${esc(k)}" value="${esc(Q.form.params[k] || '')}" placeholder="${esc(p.pattern)}" spellcheck="false" title="${esc(p.description || p.pattern)}"></div>`).join('');
  return `<section class="panel"><div class="sec-head" style="margin:0"><h2>Add a job</h2></div>
    <form id="${Q.uid}-form" autocomplete="off" class="q-form">
      <div class="field"><label for="${Q.uid}-t">Template</label><select id="${Q.uid}-t" data-q-template>${ts.map(x => `<option value="${esc(x.name)}"${x.name === t.name ? ' selected' : ''}>${esc(x.name)} (${esc(x.runner)})</option>`).join('')}</select></div>
      ${params}
      <div class="field"><label for="${Q.uid}-w">When</label><select id="${Q.uid}-w" data-q-form-when>${WHEN.map(([v, l]) => `<option value="${v}"${v === Q.form.when ? ' selected' : ''}>${l}</option>`).join('')}</select></div>
      <div class="field"><label for="${Q.uid}-nb">Not before (ET)</label><input id="${Q.uid}-nb" type="datetime-local" data-q-form-nb value="${esc(Q.form.notBefore)}"></div>
    </form>
    <div class="quote"><span class="muted mono" style="font-size:12px">${esc((t.command || []).join(' '))}${t.workingDir ? ' · in ' + esc(t.workingDir) : ''}</span>
      <button type="button" class="btn primary" data-q-add>Add</button>${Q.msg ? `<span class="${Q.msg.ok ? 'msg-ok' : 'msg-err'}">${esc(Q.msg.text)}</span>` : ''}</div></section>`;
}

defineWidget('queue', {
  title: 'Queue',
  mount(el) {
    const Q = { uid: 'queue' + seq++, editing: '', log: '', logText: '', showAll: false, rowErr: {}, msg: null,
      form: { template: '', params: {}, when: 'after-close', notBefore: '' } };
    Q.redraw = () => {
      const l = el.querySelector('#' + Q.uid + '-list'); if (l) l.innerHTML = list(Q);
      const f = el.querySelector('#' + Q.uid + '-add'); if (f && !f.contains(el.ownerDocument.activeElement)) f.innerHTML = addForm(Q);
    };
    let logTimer = 0;
    async function readLog() {
      if (!Q.log) return;
      try { Q.logText = (await queueApi('jobs/' + encodeURIComponent(Q.log) + '/log?tail=300')).log || '(empty)'; } catch (e) { Q.logText = e.message; }
      Q.redraw();
    }
    const act = async (id, path, body, method) => {
      delete Q.rowErr[id];
      try { await queueApi(path, body, method); } catch (e) { Q.rowErr[id] = e.message; }
      load();
    };
    const click = async e => {
      const b = e.target.closest('button'); if (!b) return;
      const d = b.dataset;
      if (d.qRun !== undefined) return act(d.qRun, `jobs/${encodeURIComponent(d.qRun)}/run-now`, {});
      if (d.qCancel !== undefined) return act(d.qCancel, `jobs/${encodeURIComponent(d.qCancel)}/cancel`, {});
      if (d.qEdit !== undefined) { Q.editing = Q.editing === d.qEdit ? '' : d.qEdit; return Q.redraw(); }
      if (d.qSave !== undefined) {
        const nb = /** @type {HTMLInputElement} */ (el.querySelector('[data-q-nb]')).value;
        const when = /** @type {HTMLSelectElement} */ (el.querySelector('[data-q-when]')).value;
        Q.editing = '';
        return act(d.qSave, `jobs/${encodeURIComponent(d.qSave)}`, { notBefore: nb ? nyToUnix(nb) : null, when }, 'PATCH');
      }
      if (d.qLog !== undefined) {
        Q.log = Q.log === d.qLog ? '' : d.qLog; Q.logText = '';
        clearInterval(logTimer); if (Q.log) { readLog(); logTimer = setInterval(readLog, 2000); }
        return Q.redraw();
      }
      if (d.qAll !== undefined) { Q.showAll = !Q.showAll; return Q.redraw(); }
      if (d.qAdd !== undefined) {
        b.disabled = true; Q.msg = null;
        const body = { template: Q.form.template, params: Q.form.params, when: Q.form.when, addedBy: 'console',
          notBefore: Q.form.notBefore ? nyToUnix(Q.form.notBefore) : null };
        try { const j = await queueApi('jobs', body); Q.msg = { ok: true, text: `Added ${j.id}.` }; Q.form.params = {}; Q.form.notBefore = ''; }
        catch (err) { Q.msg = { text: err.message }; }
        const f = el.querySelector('#' + Q.uid + '-add'); if (f) f.innerHTML = addForm(Q);
        load();
      }
    };
    const input = e => {
      const t = e.target;
      if (t.dataset.qParam !== undefined) Q.form.params[t.dataset.qParam] = t.value;
      else if (t.dataset.qTemplate !== undefined) { Q.form.template = t.value; Q.form.params = {}; const f = el.querySelector('#' + Q.uid + '-add'); if (f) f.innerHTML = addForm(Q); }
      else if (t.dataset.qFormWhen !== undefined) Q.form.when = t.value;
      else if (t.dataset.qFormNb !== undefined) Q.form.notBefore = t.value;
    };
    el.addEventListener('click', click); el.addEventListener('input', input); el.addEventListener('change', input);
    el.innerHTML = `<div><h1>Queue</h1><p class="muted" style="margin:0">Work that runs later or when the machine can spare it, from the deployment's job templates. Times are New York time.</p></div>
      <div id="${Q.uid}-add"></div><div id="${Q.uid}-list"></div>`;
    panels.add(Q); load();
    return { dispose() { panels.delete(Q); clearInterval(logTimer); el.removeEventListener('click', click); el.removeEventListener('input', input); el.removeEventListener('change', input); } };
  },
});
