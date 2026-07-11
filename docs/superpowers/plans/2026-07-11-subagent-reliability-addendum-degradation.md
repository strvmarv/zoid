# Addendum: Remove layout degradation path (hard 160×40 minimum)

> Appended to the subagent-reliability branch. Eliminates the narrow-terminal
> collapse/fill allocator and replaces it with a hard minimum + simple
> allocation. Reduces test count (~15 degradation tests deleted) and eliminates
> the 28-snapshot cascade on every layout change.

## What we're removing

The entire "graceful degradation" subsystem in `layout.rs`:
- `MIN_DRAWER_BODY_ROWS`, `drawer_fit_priority`, the 3-step collapse/fill
  algorithm in `allocate_drawer_bodies`.
- `RAIL_MIN_TOTAL` conditional rail hiding (`show_rail` based on width).
- The palette "appears at narrow width" tests.
- All narrow-width layout tests (100×24).

## What replaces it

- **Hard minimum: 160×40.** If the terminal is smaller, render a full-screen
  "Terminal too small — resize to at least 160×40" message. No partial layout.
- **Simple allocation:** every open drawer gets exactly its `drawer_body_rows`.
  No collapse ordering, no surplus fill, no priority ranking.
- **Rail always visible** (160 cols is well above `RAIL_WIDTH + stream minimum`).

## Tasks

### Task A1: Simplify `allocate_drawer_bodies` to full-height allocation

Delete `MIN_DRAWER_BODY_ROWS`, `drawer_fit_priority`, and the 3-step algorithm.
Replace `allocate_drawer_bodies` with: each open drawer gets its full
`drawer_body_rows`; closed drawers get 0. No collapse, no fill.

### Task A2: Add the hard-minimum check in `compute`

At the top of `compute`: if `area.width < 160 || area.height < 40`, return a
`ShellLayout` with only `title` and `body` set (the renderer draws the
"too small" message into `body`). Everything else is `None`/empty.

### Task A3: Render the "too small" message

In `render_shell`: if `layout.rail.is_none() && area < 160×40`, render the
message instead of the normal shell.

### Task A4: Delete degradation tests, standardize remaining at 160×40

Delete all tests that verify collapse/fill behavior. Standardize the remaining
layout and snapshot tests at 160×40. Re-accept snapshots.
