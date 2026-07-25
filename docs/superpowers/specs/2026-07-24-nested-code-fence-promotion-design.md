# Nested Code Fence Promotion — Design

> **Status:** DESIGN (brainstorming, 2026-07-24). Ready for `writing-plans`.
>
> **Parent:** Markdown rendering fix for nested fenced code blocks.

---

## 1. Goal & scope

When the model's output contains a fenced code block that itself contains
``` fences, `pulldown-cmark` (correctly per CommonMark) treats the first
inner ``` as the closing fence of the outer block. This causes the
rendering to "flip" between code and prose — the inner ``` closes the
outer block, the text after it is parsed as markdown, the next inner ```
opens a new block, and so on.

**Fix:** a preprocessing step in `render_body` that promotes ``` fences
to ```` when the block they open contains inner ``` lines. A ```` fence
only closes at the next ```` line, so inner ``` lines are consumed as
content. This is standard CommonMark — no parser changes needed.

**In scope:**
- A `promote_nested_fences(source: &str) -> String` function in
  `markdown.rs`.
- Called at the top of `render_body` before `Parser::new_ext`.
- Line-by-line scan: detect ``` openings, scan ahead for the close, count
  inner ``` lines, promote if needed.

**Out of scope:**
- Handling ```` fences that contain inner ```` fences (would need `````).
  This is recursive but vanishingly rare — the model rarely produces
  4-backtick fences. A simple iterative approach handles the common case;
  if ```` nesting ever occurs, the same approach extends recursively.
- Changing `pulldown-cmark` or the parser options.
- The `render_markdown` wrapper (it delegates to `render_body`).

---

## 2. Algorithm

```
fn promote_nested_fences(source: &str) -> String:
    lines = source.split('\n')
    output = []
    i = 0
    while i < len(lines):
        line = lines[i]
        # Is this a fence opening? ``` optionally followed by a language tag.
        # A fence opening line starts with ``` and the rest is whitespace or
        # a language tag (no other content).
        if is_fence_open(line, 3):  # exactly 3 backticks
            # Scan ahead to find the matching close (next line that is
            # exactly ``` or starts with ```).
            close_idx = find_close(lines, i + 1, 3)
            if close_idx is not None:
                # Count inner ``` lines between open and close.
                inner_count = count_inner_fences(lines, i + 1, close_idx, 3)
                if inner_count > 0:
                    # Promote: replace ``` with ```` on open and close.
                    output.append(promote(line, 3, 4))
                    for j in range(i + 1, close_idx):
                        output.append(lines[j])
                    output.append(promote(lines[close_idx], 3, 4))
                    i = close_idx + 1
                    continue
        output.append(line)
        i += 1
    return '\n'.join(output)
```

### 2.1 `is_fence_open(line, n) -> bool`

True if the line starts with exactly `n` backticks, optionally followed by
a language tag (alphanumeric + hyphens), and nothing else. Leading
whitespace is allowed (up to 3 spaces per CommonMark). The line must not
be a closing fence (closing fences have no language tag — but we detect
closes separately by scanning ahead, so this function just identifies
potential opens).

### 2.2 `find_close(lines, start, n) -> Option<usize>`

Scans from `start` for the first line that is exactly `n` backticks
(optionally with trailing whitespace), or starts with `n` backticks
followed by nothing but whitespace. Returns the index, or `None` if no
close is found (unterminated fence — leave it alone, `pulldown-cmark`
handles it).

### 2.3 `count_inner_fences(lines, start, end, n) -> usize`

Counts lines between `start` and `end` (exclusive) that are exactly `n`
backticks (with optional trailing whitespace). These are inner ``` lines
that `pulldown-cmark` would misinterpret as closing fences.

### 2.4 `promote(line, from_n, to_n) -> String`

Replaces the first `from_n` backticks in the line with `to_n` backticks.
For opening fences, the language tag is preserved. For closing fences,
there's no tag — just the backticks.

---

## 3. Integration

```rust
pub fn render_body(source: &str, content_w: usize) -> Vec<BodyLine> {
    let source = promote_nested_fences(source);
    // ... rest unchanged: Parser::new_ext(&source, ...) ...
}
```

The preprocessor runs on every `render_body` call. It's O(n) in the
number of lines, with no allocation when no nesting is detected (the
common case returns the input unchanged — or rather, returns a `String`
that is identical to the input).

**Performance:** the scan only allocates a `String` when promotion is
needed. For the common case (no nested fences), the scan is a simple
line iteration that returns `source.to_string()`. The overhead is
negligible — one pass over the lines, no regex, no parsing.

---

## 4. Edge cases

- **No inner fences:** ``` block with no inner ``` → no promotion, output
  unchanged.
- **Unterminated fence:** ``` with no closing ``` → `find_close` returns
  `None`, no promotion, `pulldown-cmark` handles it (renders as a code
  block to end of input).
- **Indented code blocks** (4-space indent): not affected — these are
  `CodeBlockKind::Indented`, not fenced. The preprocessor only looks for
  ``` fences.
- **```~ fences (tilde fences):** CommonMark also supports `~~~` fences.
  The preprocessor handles ``` only — `~~~` fences with inner `~~~` are
  even rarer and can be added later if needed.
- **Multiple nested levels:** ``` containing ``` containing ``` — the
  scan promotes the outermost ``` to ````. But the inner ``` containing
  ``` still has the same problem. A recursive approach handles this:
  after promoting the outer fence, re-scan the content. For v1, a single
  pass handles the common case (one level of nesting). A loop or
  recursive call can handle deeper nesting.
- **Language tag on the fence:** ```rust, ```json, etc. — the language
  tag is preserved during promotion (````rust, ````json).

---

## 5. Testing

- **No nesting:** ``` block with no inner ``` → output unchanged.
- **Single nesting:** ``` containing one inner ``` → outer promoted to
  ````.
- **Language tag preserved:** ```rust containing inner ``` → ````rust.
- **Multiple inner fences:** ``` containing 3 inner ``` lines → outer
  promoted to ````.
- **No false positives:** a ``` block with ``` in the *middle of a line*
  (not a full fence line) → no promotion (inner ``` must be on its own
  line to be a fence).
- **Unterminated fence:** ``` with no close → no promotion.
- **Indented code:** 4-space indented content → no promotion (not a
  fence).
- **Round-trip:** `promote_nested_fences` is idempotent — running it
  twice produces the same output.

---

## 6. Cross-crate impact

- **`markdown.rs` (zoid-tui)** — new `promote_nested_fences` function.
  Called at the top of `render_body`. No other file changes.
- `cargo build --workspace && cargo test --workspace` after the change.