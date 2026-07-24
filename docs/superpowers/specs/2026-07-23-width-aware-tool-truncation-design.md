# Width-Aware Tool Call & Result Truncation

## Problem

Tool call arguments and result previews are truncated at hardcoded limits regardless
of terminal width:

- **Tool call args:** `scalar()` truncates each argument value to **30 chars** via
  `truncate(s, 30)`. The args are formatted as `key: value` pairs, each independently
  truncated. So `shell(command: cd /home/gomanjoe/source/zoid…)` cuts the command at
  30 chars even on a 111-column terminal where 80+ chars are available.

- **Result preview:** `first_line()` truncates the first line of output to **40 chars**
  via `truncate(s.lines().next(), 40)`. So `✓ shell →    Compiling zoid-core v0.5.0
  (/home/go…` cuts at 40 chars even when 95+ chars are available.

On a typical 160-column terminal with the rail (text width ~111), these limits waste
most of the available space. The user sees truncated paths and output that are too
short to be useful — they can't tell which directory a `cd` targets, or which crate
is compiling.

## Design

Replace the hardcoded per-value truncation with **width-aware truncation of the whole
args/result string as a unit**, capped at `min(available_width, 120)`.

### §1 `scalar` — remove per-value truncation

`scalar` currently truncates each value to 30 chars. Change it to return the full
string — truncation moves up to `arg_summary` where the width budget is known:

```rust
// BEFORE:
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate(s, 30),
        other => truncate(&other.to_string(), 30),
    }
}

// AFTER:
fn scalar(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
```

### §2 `arg_summary` — truncate the joined string to a width budget

Add a `max_width: usize` parameter. Build the full `key: value` pairs string (using
the untruncated `scalar`), then truncate the whole thing as a unit:

```rust
// BEFORE:
fn arg_summary(args_json: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    }
}

// AFTER:
fn arg_summary(args_json: &str, max_width: usize) -> String {
    let v: serde_json::Value = serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    let inner = match v {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, val)| format!("{k}: {}", scalar(val)))
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar(&other),
    };
    truncate(&inner, max_width)
}
```

This means single-arg tools (the common case: `shell`, `read`, `update_tasks`) get the
full budget for their one argument. Multi-arg tools (`edit`, `write`) get the first
arg(s) in full and the later ones truncated — the first arg (typically the path or
command) is the most important, and `peek` / Detail zoom are available for full details.

### §3 `first_line` — truncate to a width budget

Add a `max_width: usize` parameter, replacing the hardcoded 40:

```rust
// BEFORE:
fn first_line(s: &str) -> String {
    truncate(s.lines().next().unwrap_or(""), 40)
}

// AFTER:
fn first_line(s: &str, max_width: usize) -> String {
    truncate(s.lines().next().unwrap_or(""), max_width)
}
```

### §4 Width budget computation in `build_conversation`

The render context carries `ctx.width` (the conversation text width, already
padding-adjusted). Compute the budget for each line by subtracting the fixed
overhead:

**Tool-call line:** `  ● {name}({args}) ⏎ peek`
- Fixed overhead: `  ● ` (4 display cols) + `name` (display width of the tool name) +
  `(` (1) + `) ⏎ peek` (10) = 15 + name_width.
- Args budget: `(ctx.width - 15 - name_width).min(120)`, saturated at 0.

**Result line:** `  ✓ {name} → {first_line}`
- Fixed overhead: `  ✓ ` (4) + `name` (display width) + ` → ` (3) = 7 + name_width.
- If the result is compacted, add the `compacted` prefix width: `{glyph} compacted `
  (display width varies by glyph, approximately 12).
- Result budget: `(ctx.width - 7 - name_width - compacted_overhead).min(120)`,
  saturated at 0.

Implementation:

```rust
// Tool call line:
let name_w = display_width(&tc.name);
let args_budget = ctx.width.saturating_sub(15 + name_w).min(120);
// → Span::styled(format!("({})", arg_summary(&tc.args, args_budget)), ...)

// Result line (non-diff path):
let name_w = display_width(name);
let mut overhead = 7 + name_w;
if *compacted {
    overhead += 12; // approximate width of "{glyph} compacted "
}
let result_budget = ctx.width.saturating_sub(overhead).min(120);
// → Span::styled(format!(" → {}", first_line(output, result_budget)), ...)
```

`display_width` uses `UnicodeWidthStr::width` (already used by `truncate` in
`text.rs`). For the compacted overhead, use the actual formatted prefix length rather
than a hardcoded 12 if convenient — but a constant approximation is fine since the
budget is a soft cap, not an exact column fit (ratatui handles line rendering).

The `min(120)` cap means very wide terminals (200+ cols, text width 150+) still cap
tool-call args and result previews at 120 chars — preventing absurdly long lines while
giving 3-4× more content than the current 30/40 limits on normal terminals.

### §5 Visual example

Before (111-col text width, current hardcoded limits):

```
  ● shell(command: cd /home/gomanjoe/source/…) ⏎ peek
  ✓ shell →    Compiling zoid-core v0.5.0 (/home/go…
  ● update_tasks(tasks: [{"status":"done","text":"T1-…) ⏎ peek
  ✓ update_tasks → 4 tasks · 0 active
```

After (111-col text width, width-aware with cap 120):

```
  ● shell(command: cd /home/gomanjoe/source/zoid && cargo build --spike 2>&1 | head -20) ⏎ peek
  ✓ shell →    Compiling zoid-core v0.5.0 (/home/gomanjoe/source/zoid)
  ● update_tasks(tasks: [{"status":"done","text":"T1-verify spike compiles"},{"status":"done","text":"T2-fix… ⏎ peek
  ✓ update_tasks → 4 tasks · 0 active
```

The `shell` command is now fully visible. The `update_tasks` JSON shows the first two
task entries before truncating. The result line shows the full crate name and path.

### §6 What is not touched

- **`truncate` / `truncate_start`** (`text.rs`) — unchanged. They handle the ellipsis
  and display-width logic.
- **The `⏎ peek` hint** — left as-is. (It is a non-functional visual hint — there is no
  action or keybinding behind it — but removing it is out of scope for this change.)
- **The diff preview path** (edit/write results showing `+N −N` instead of
  `first_line`) — unchanged. Only the `first_line` fallback path gets the width budget.
- **Detail/Summary/Overview zoom** — no tool-call/result line rendering at those zooms.
- **`push_message` and assistant text rendering** — no change.
- **The projection layer** — unchanged.

### §7 Testing

**Unit tests in `chat.rs`:**

- `scalar` returns full string (no truncation) — verify with a 100-char string returns
  all 100 chars.
- `arg_summary` with a short single-arg JSON and large budget → full string, no
  truncation.
- `arg_summary` with a long single-arg JSON and budget 60 → truncated to 60 with
  ellipsis.
- `arg_summary` with multi-arg JSON → all args joined, then truncated as a unit to
  budget (first args visible, later ones cut).
- `arg_summary` with budget 0 → empty string (saturates).
- `first_line` with a short output and large budget → full first line.
- `first_line` with a long output and budget 80 → truncated to 80 with ellipsis.
- `first_line` with multi-line output → only the first line is used (existing behavior
  preserved).
- `first_line` with empty output → empty string.
- Tool-call line at width 111 with a short command → no truncation (fits in budget).
- Tool-call line at width 111 with a very long command → truncated to
  `min(111 - 15 - name_w, 120)`.
- Result line at width 200 → budget capped at 120 (not the full ~193 available).
- Budget saturates at 0 for very narrow terminals (name longer than width) → empty
  args string, no panic.
- Existing tests that called `arg_summary` / `first_line` without a width parameter
  are updated to pass the new argument.

### §8 Edge cases

- **Tool name longer than text width:** `ctx.width.saturating_sub(15 + name_w)` = 0 →
  `arg_summary` gets budget 0 → `truncate` returns empty string. The line still shows
  `  ● longtoolname() ⏎ peek`. No crash.
- **Empty args JSON (`{}` or `null`):** `arg_summary` returns `""` or `"null"` — short
  enough to fit in any budget. No truncation.
- **Result output with no newlines:** `first_line` takes the whole string, truncates
  to budget. Same behavior, wider budget.
- **Result output that's empty:** `first_line` returns `""` → result line shows
  `  ✓ name → `. Same as today.
- **Unicode in args/results:** `truncate` uses `UnicodeWidthStr` — handles wide glyphs
  correctly. The `name_w` computation also uses `UnicodeWidthStr::width`. Already
  tested in `text.rs`.
- **Compacted result prefix:** The `compacted` glyph + " compacted " prefix adds
  overhead to the result line. The budget accounts for this so the `first_line`
  preview doesn't overflow.