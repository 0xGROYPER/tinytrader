# 0008: The console as workspaces of widgets

- **Status:** Accepted
- **Date:** 2026-10-06
- **Amends:** [0003](0003-front-ends.md) (the console's shape)

## Context

The console began as pages in a sidebar. It became workspaces: tiles, each
showing one view, arranged by the operator, switched with keys and saved per
browser. The tiling was written by hand (a binary split tree) and the views
are plain functions that render HTML. That works, but it has no tabs within a
tile, no dragging a tile to dock it elsewhere, no floating windows, and
workspaces are a flat list that won't scale.

## Decision

- **A docking library manages the windows: Dockview** (`dockview-core`, MIT,
  no dependencies, plain JavaScript). Groups of tabbed panels split in any
  direction, panels drag to dock or float, a group can be maximised, and a
  layout is a JSON document. The library is vendored in the console's image
  and served from `/vendor/` with its version in the name; nothing is loaded
  from a CDN.
- **A widget is a definition:** a name, a title, and how it renders into an
  element it's given; later, settings (which account, symbol or schema), the
  data it subscribes to, and a channel linking it to others (pick a symbol
  in one, the linked ones follow). Until settings exist, a widget appears in
  one panel at a time.
- **Workspaces are named Dockview layouts in groups**, switched from the
  header tabs (the current group's, with a breadcrumb), a switcher popup (a
  tree: groups expand and collapse, workspaces open, rename, duplicate, move
  and delete) and keys (⌥↑/⌥↓ within a group, ⌥←/⌥→ between groups, ⌥1–⌥9,
  ⌘K). They are kept per browser until the console has a login, then by the
  console.
- **Only the open workspace runs.** Its panels render and refresh; others are
  saved layouts, which is all the switcher's previews read.

## Consequences

- The hand-written tiling goes; saved tile trees convert to Dockview layouts
  once.
- The page loads one more script (about 540 KB, cached).
- Settings, channels and live streams per widget come with the symbol
  workspace, not now.
