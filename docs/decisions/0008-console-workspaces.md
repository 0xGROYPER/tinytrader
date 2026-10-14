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

**Amended 2026-10-12: a tree of folders and views, in a side pane, in place
of the numbers.** The console's layouts become a tree: *folders* are the
parents and *views* the leaves, each view one saved layout with its tabs (the
code still calls a layout a workspace; its `group` holds its folder's path,
`Market/Options`). Folders nest as deep as you like and are kept when empty.
Every view is in a folder: one found loose (saved before folders, or
ready-made) gets a folder of its own name, and a new folder opens on a first
view of its name. The tree is in a side pane on the left (☰ in the header,
or ⌥B), which slides over the page rather than taking room from it and goes
away when you pick something, press Esc or click outside it. You arrange it:
views and folders are dragged into a folder or beside a view (a view dropped
on the pane's title gets a folder of its own); nothing nests by itself.
Deleting a folder deletes the views in it, and a view's folder goes with
its last view (each asked first, in its row; at least one view stays).  The
ready-made views start filed together (Market, Oil & Iran and Earnings in
Market, the Watchlist view in Market › Watchlists; Operations and Data in
Operations), and the palette's "Reset views to
the defaults" starts a browser over. Keys move one level at a time, as in a
file manager (amended 2026-10-13, replacing ⌥↑/⌥↓ over every view under the
top-level folder): ⌥↑/⌥↓ step through what's in the current folder, its
views and then its folders; landing on a view opens it, landing on a folder
only picks it. ⌥→ goes into the picked folder, to the view last open there
(else its first), and Enter (⌥ held or not) opens its first view; ⌥← goes up a level with the folder it came out of picked.
A card in the middle of the page shows the level when a folder is picked or the level changes; stepping from view to view shows none (the header names the view). ⌥B moves
into the pane to pick a view anywhere (↑↓, → and ← to open and close, Enter
opens the view, or a folder's last view, and puts the pane away; N a folder
inside; Esc goes back); the pane opened with a click takes the keys too. The header's numbers
(1–9, 0 and ⌥1–⌥0) go: the header shows the folder and view
(`devops / deployment › Operations`), and clicking it opens the pane. The cap
of ten goes too (100 views now).

## Consequences

- The hand-written tiling goes; saved tile trees convert to Dockview layouts
  once.
- The page loads one more script (about 540 KB, cached).
- Settings, channels and live streams per widget come with the symbol
  workspace, not now.
