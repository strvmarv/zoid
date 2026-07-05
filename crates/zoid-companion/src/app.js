// Companion dashboard client. Served same-origin as /s/<token>/app.js so the
// page needs no inline <script> — which lets the CSP use `script-src 'self'`
// (no 'unsafe-inline'), keeping any scripts in agent-authored cards inert.
const dash = document.getElementById("dashboard");
const card = document.getElementById("card");

// Escape interpolated *metric* text before it touches innerHTML. These fields
// (provider/model/session name, tier labels) are data, not markup.
const esc = (s) =>
  String(s).replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c])
  );

const es = new EventSource("events");

es.addEventListener("dashboard", (e) => {
  const d = JSON.parse(e.data);
  const pct = d.ctx_ceiling
    ? Math.min(100, Math.round((d.ctx_used / d.ctx_ceiling) * 100))
    : 0;
  const tiers = (d.tiers || [])
    .map(
      (t) =>
        `<div><span class="k">${esc(t.label)}</span> <span class="v">${t.tokens}</span>${
          t.cold ? " · cold" : ""
        }</div>`
    )
    .join("");
  dash.innerHTML =
    `<div class="k">${esc(d.provider)} · ${esc(d.model)} · ${esc(d.session_name)}</div>` +
    `<div class="v">${d.ctx_used} / ${d.ctx_ceiling} (${pct}%)</div>` +
    `<div class="bar"><i style="width:${pct}%"></i></div>` +
    `<div class="k">tiers</div>${tiers}` +
    `<div class="k">tasks: ${d.tasks_len}${d.busy ? " · busy" : ""}</div>`;
});

// The card is raw, agent-authored HTML by design (the `show` tool) — rendered
// as innerHTML for rich content (tables, SVG, layout). Blast radius is
// contained: <script> tags inserted via innerHTML never execute (a DOM rule,
// not CSP), and `script-src 'self'` additionally neutralizes inline event-
// handler attributes and `javascript:` URIs; `connect-src`/`form-action` 'self'
// block script- and form-driven egress. (Top-level navigation is a residual —
// see the CSP note in server.rs.)
es.addEventListener("card", (e) => {
  card.innerHTML = JSON.parse(e.data);
});
