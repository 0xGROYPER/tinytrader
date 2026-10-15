// What the order gateway's answers mean, in words, for the order tickets
// (Spread builder, Accounts). A refusal's detail is an object; it's said
// plainly for the reasons that have one, else listed as it came.

const words = d => Object.entries(d || {}).map(([k, v]) => `${k.replace(/_/g, ' ')}: ${typeof v === 'object' ? JSON.stringify(v) : v}`).join(', ');

/** Why an order wasn't sent (`not_sent`), as a sentence. @param {string} reason @param {any} d */
export function notSentText(reason, d) {
  if (d == null || typeof d !== 'object') return `${reason}${d ? `: ${d}` : ''}`;
  switch (reason) {
    case 'cap': return `over the account's ${String(d.cap || 'limit').replace(/_/g, ' ')} cap${Object.keys(d).length > 1 ? ` (${words(Object.fromEntries(Object.entries(d).filter(([k]) => k !== 'cap')))})` : ''}`;
    case 'killed': return 'the account\'s kill switch is on: nothing opens until it\'s lifted';
    case 'not_ready': return `the gateway isn't ready${d.state ? ` (${d.state})` : ''}: try again in a moment`;
    case 'throttled': return 'the gateway\'s request budget is used up for the minute: try again shortly';
    case 'validation': return `the order doesn't validate${d.path ? ` at ${d.path}` : ''}${d.message ? `: ${d.message}` : ''}`;
    default: return `${reason.replace(/_/g, ' ')}${Object.keys(d).length ? ` (${words(d)})` : ''}`;
  }
}
