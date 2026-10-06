// Document collections (the console's /api/collections): labels shared by
// the docs pages and the Docs widget.
// Folder names are capitalized, except the platform's own lowercase name.
export const folderLabel = n => n === 'tinytrader' ? n : n.charAt(0).toUpperCase() + n.slice(1);
export const groupLabel = g => g.split('/').map(p => folderLabel(p)).join(' › ');
let loading = null;
// The collections, fetched once per page load.
export function collections() {
  loading ||= fetch('/api/collections', { cache: 'no-store' }).then(r => r.ok ? r.json() : []).catch(() => []);
  return loading;
}
