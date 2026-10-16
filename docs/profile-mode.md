# Console profile mode

The console can run with everything on the page measured while you use it, so slowness can be found where it happens: in Safari on a laptop or phone, in your own workspaces, during real clicks and scrolls. Off by default, and free when off: nothing is wrapped or watched until it's turned on.

## Turning it on

- Open the console with `#profile` at the end of its address (for example `https://<console-host>/#profile`), or
- press ⌘K and choose **Profile mode: turn on**.

It stays on for that browser tab until **Turn off** in its bar (which reloads the page, so nothing stays wrapped).

## What it measures

- **Widgets:** every mount, update and dispose of every panel, timed; whether an update's redraw changed anything (`paint()`), and timer and animation-frame callbacks each widget scheduled.
- **Fetches:** every request's path (symbols collapsed to `:sym`), time, size, status and the widget that made it; and the time to parse each JSON response.
- **The state poll:** its size and parse time.
- **Stalls:** the main thread blocked for more than 50 ms — long tasks where the browser reports them (Chrome), gaps between animation frames otherwise (Safari) — and the frame rate.
- **Size:** the number of DOM elements every 5 seconds, and the JS heap where the browser reports it (Chrome only).
- **What you did:** clicks, keys and scrolls (which panel, and the button's text), `tt-*` events between widgets, and errors.

A bar along the bottom shows the last minute: frame rate, stalls, data fetched, DOM size, heap, the slowest widget and any errors. A small badge over each panel shows its last and average update time and how many redraws were skipped as unchanged.

## Recording

**Record** in the bar starts a recording (at most 20,000 events or about 5 MB); **Stop and save** uploads it to the console and offers it as a download. The console keeps the newest 30, in `profiles/` beside its reminders (`CONSOLE_PROFILES` to choose another directory):

- `GET /api/profiles` lists them; `GET /api/profiles/{id}` returns one.
- On the host, `services/console/scripts/profile-report` summarises the newest (or a named id, or a downloaded file; `--list` lists them): the slowest widgets, timers and parsing by widget, each stall with what happened in the second before it, fetch traffic by path, and the DOM and heap over time.
