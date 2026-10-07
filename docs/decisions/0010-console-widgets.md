# 0010: Console widgets as components

- **Status:** Accepted
- **Date:** 2026-10-06
- **Amends:** [0008](0008-console-workspaces.md) (what a widget is)

## Context

The console's widgets began as pages and were fitted into Dockview panels
([0008](0008-console-workspaces.md)). A widget is spread over five places: a
name in `VIEWS`, a branch of `viewHtml`, a `STEADY` set that stops it being
redrawn, special cases in `renderPanel`, and `state.tab` checks in `render`.
Every panel's HTML is replaced each second, so a widget holding a form or
input has to opt out. Widgets find their elements by document-wide ids and
their events through some twenty listeners on the page, so no widget can be
open twice and nothing is cleaned up when a panel closes. The page is one
2,300-line file. The old full-page mode survives beside the workspaces.

## Decision

- **A widget is one module with one contract**, registered by name:

  ```js
  defineWidget('news', {
    title: 'News',
    mount(el) {
      // build into el; listen on el only
      return { update() { … }, dispose() { … } };
    },
  });
  ```

  - `mount` gets the panel's element. It returns an instance whose
    `update()` runs after each poll of the fleet and whose `dispose()` runs
    when the panel closes or the workspace changes. Settings, when they come,
    are a second argument.
  - `redrawing(draw)` builds the mount for a widget that only rebuilds its
    HTML.
  - A widget decides how to redraw. A simple one rebuilds its HTML; one
    holding a form or input updates in place. There is no opt-out list.
  - A widget keeps its own state in its instance and uses no element ids, so
    two of the same widget can be open at once. Settings and linked channels
    (0008) build on this.
  - Shared data comes from `lib/fleet.js` (the topology, the latest snapshot
    and the status helpers); formatting from `lib/format.js`.
- **Files, with no build step:** `web/index.html` is the shell (header, bell,
  switcher, palette, the Dockview host); `web/widgets/<name>.js` one module per
  widget; `web/lib/*.js` shared code; `web/vendor/` third-party libraries. The
  page's script is an ES module that imports them.
- **The console serves `web/` as built.** Every file under `web/` is built
  into the binary. With `CONSOLE_PAGE` set, the page and every file are read
  from that file's directory instead, so the page can change without a
  rebuild. Either way, one source; neither falls back to the other.
- **Workspaces are the only host.** The full-page mode goes; the logo opens
  workspace 1. The docs pages keep their own host. The composite Market view
  goes too: the Market workspace shows the same widgets side by side.

## Plan

Widgets move one at a time, simplest first: Schedule and News, then the
read-only ones (Prices, Services, Calendar, Accounts, System graph), then
those with state (Data, Storage, Shell, Reminders, Jobs). The page works after
every step. When the last one moves, `VIEWS`, `viewHtml`, `STEADY` and the
page mode are deleted.

## Consequences

- A new widget is one file and one registration.
- About the same amount of code, rearranged; the special cases go.
- The console image changes when `web/` changes (it is built in), as before.
