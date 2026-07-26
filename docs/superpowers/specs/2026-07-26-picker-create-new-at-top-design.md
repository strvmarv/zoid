# Startup Picker: "Create new" at the top

## Problem

The startup session picker lists sessions (most-recent first) and ends with a
trailing "Create new session" row at the bottom. With a long session list the
"Create new" row sits below the fold; the scroll offset (added in `295ea25`)
keeps it reachable, but it still competes with the most-recent sessions for
visibility and requires scrolling down to reach when the list is long. As a
result the always-available "start fresh" action — the one thing a user can
always do regardless of session state — is the easiest to lose.

## Design

Move "Create new session" to the **top** of the list, directly under the
title/blank header, so it is always on screen no matter how many sessions
exist. The cursor still starts on the most-recent session (the common case —
resume recent — stays one Enter away); "Create new" is visible but unselected,
rendered in its existing dim style until the cursor moves onto it.

This is a layout + outcome-mapping change inside `pick_session`, plus a small
boundary flip in the pure `pick_choice` so it encodes the new layout directly.

### §1 New row order

Current `lines` Vec (top → bottom):

```
0          title
1          (blank)
2 .. 2+n-1   session rows (most-recent first)
2+n        "Create new session"
2+n+1      (blank)
2+n+2      hint
```

New `lines` Vec:

```
0          title
1          (blank)
2          "Create new session"
3 .. 3+n-1   session rows (most-recent first)
3+n        (blank)
3+n+1      hint
```

"Create new" is pinned at line 2 — below the fixed two-line header, above every
session row — so it is always within the first screen and can never be clipped.

### §2 Logical index remap (the conceptual change)

`pick_choice` models the picker as `n_sessions + 1` logical rows with linear
wrap. Today the boundary it uses to tell "session row" from "Create new" is
`cur < n_sessions` (indices `0..n-1` are sessions; index `n` is Create new).
The reorder flips that boundary so **index 0 is "Create new"** and
**indices `1..=n` are sessions**.

This requires a small, self-contained change to `pick_choice` itself — the
pure function should encode the actual layout, not have `pick_session`
translate indices around it. The wrap math (`total = n + 1`, Up from 0 → `n`,
Down from `n` → 0) is unchanged; only the session/Create-new boundary moves.

| Logical index | Today (`cur < n`)    | After (`cur == 0` is Create-new) |
|--------------|----------------------|----------------------------------|
| 0            | first session        | **"Create new"**                 |
| 1 .. n-1     | session rows         | session rows                      |
| n            | "Create new"         | last session                      |

New `pick_choice` rules:
- `selected` initializes to **1** (the first/most-recent session), not 0.
- **Enter:** `cur == 0` → `CreateNew`; `cur >= 1` → `Resume(cur)` (session row).
- **Delete:** `cur == 0` → `Pending(0)` (no-op, can't delete "Create new");
  `cur >= 1` → `DeleteConfirm(cur)`.
- **Up/Down:** unchanged wrap math.

`pick_session`'s `PickOutcome` handling remaps to match:
- `PickOutcome::Resume(idx)` → `sessions[idx - 1]` (session rows are offset by
  1 because index 0 is now "Create new").
- `PickOutcome::CreateNew` fires when `cur == 0`.
- `PickOutcome::DeleteConfirm(idx)` → applies to `sessions[idx - 1]`.

### §3 Render changes

In the `terminal.draw` closure:
- Emit the "Create new" line **before** the session-row loop, at line index 2.
- The session-row loop renders `sessions` as before; each row's highlight test
  changes from `i == selected` to `i + 1 == selected` (since session rows now
  occupy logical indices `1..=n`).
- The "Create new" highlight test changes from `selected == n` to
  `selected == 0`.
- The optional delete-confirm line is emitted in the same place (after the
  session rows, before the trailing blank/hint) — its position relative to the
  selected row is unchanged.

### §4 Scroll offset

`picker_scroll_offset` (added in `295ea25`) is unchanged. Its caller in the
render loop updates the `selected_line` calc to reflect the new layout:

- `selected == 0` → line 2 ("Create new"). Always within the first screen →
  offset 0; the row can never be clipped, which is the whole point of the
  move.
- `selected > 0` → `2 + selected` (header is 2 lines, "Create new" is line 2,
  and the selected session is the `selected`-th session row, at line
  `2 + selected`). The optional delete-confirm line renders *below* the
  session rows, so it never shifts the selected row's line — unlike the old
  layout where "Create new" sat below the delete-confirm line. The
  `+ pending_delete.is_some()` term from the old `selected_line` calc is
  therefore dropped.
- `visible_height = area.height - 2` (block borders) — unchanged.

With "Create new" pinned at line 2, the offset's job shrinks to keeping the
**selected session row** visible; it no longer has to rescue the "Create new"
row from clipping.

### §5 Initial cursor

`selected` is initialized to `1` (the first session) — per design decision:
the common case (resume the most-recent session) stays one Enter away, and
"Create new" is a visible-but-unselected alternative at the top.

### §6 What is not touched

- `PickKey`, `PickOutcome` enums.
- `BootPath`, `boot_decision`, CLI flags (`--new`, `--resume`).
- Session store, `SessionInfo`, liveness (`is_live`), heartbeat, takeover.
- The in-session `:resume` / `:new` overlays and their confirm cards.
- The delete flow (`pending_delete`, `DeleteConfirm`) — only its index-to-row
  mapping shifts by the same offset.

### §7 Testing

- **`pick_choice` tests that assert the session/Create-new boundary update** to
  the new convention (index 0 = Create-new):
  - `pick_choice_enter_on_create_new`: `pick_choice(3, 3, Enter)` →
    `CreateNew` becomes `pick_choice(3, 0, Enter)` → `CreateNew`.
  - `pick_choice_delete_on_create_new_is_noop`: `pick_choice(2, 2, Delete)` →
    `Pending(2)` becomes `pick_choice(2, 0, Delete)` → `Pending(0)`.
  - `pick_choice_enter_on_session_resumes`: `pick_choice(3, 1, Enter)` →
    `Resume(1)` stays valid (index 1 is still a session row).
  - `pick_choice_delete_on_session_row`: `pick_choice(2, 0, Delete)` and
    `(2, 1, Delete)` → `DeleteConfirm(0)` / `DeleteConfirm(1)` become
    `pick_choice(2, 1, Delete)` and `(2, 2, Delete)` → `DeleteConfirm(1)` /
    `DeleteConfirm(2)` (index 0 is no longer a session).
  - The wrap/clamp tests (`pick_choice_up_wraps`, `pick_choice_down_wraps`,
    `pick_choice_clamps_selection_to_total_rows`,
    `pick_choice_down_advances_selection`, `pick_choice_esc_aborts`) are
    layout-agnostic and stay valid as-is.
- **`picker_scroll_offset` tests stay valid** — the pure function is unchanged.
  The `selected_line` values its caller feeds in change, but the function's
  contract (keep the selected line in the visible window) does not.
- The render path remains thin (line layout, no wrapping logic); a successful
  boot still exercises it end-to-end.