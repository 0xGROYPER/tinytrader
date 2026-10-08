// The console's own APIs that widgets call.
export async function jobsApi(path, body) {
  const r = await fetch('/api/jobs/' + path, body === undefined ? { cache: 'no-store' }
    : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No jobs service is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
// The job queue (decision 0012), through the console's /api/queue proxy.
export async function queueApi(path, body, method) {
  const r = await fetch('/api/queue/' + path, body === undefined && !method ? { cache: 'no-store' }
    : { method: method || 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body ?? {}) });
  let v = null; try { v = await r.json(); } catch (_) {}
  if (!r.ok) throw new Error(v?.error || (r.status === 404 ? 'No job queue is configured.' : `${r.status} ${r.statusText}`));
  return v;
}
