// Companion dashboard client. Served same-origin as /s/<token>/app.js so the
// page needs no inline <script> — which lets the shell CSP use `script-src
// 'self'` (no 'unsafe-inline'). This is the only JS that runs in the
// token-bearing origin; agent card JS runs isolated in a sandboxed iframe (see
// the `card` handler below).
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

// The card is raw, agent-authored HTML by design (the `show` tool), and it may
// be interactive — its own inline scripts run. To let that happen without
// handing card JS the run of this token-bearing page, each card is rendered
// inside a sandboxed <iframe> loaded from a `data:` URL. The `data:` document
// gets an opaque origin (so its scripts execute — the shell CSP does not apply
// to it), while `sandbox="allow-scripts"` (no `allow-same-origin`, no
// `allow-top-navigation`) walls it off: card JS cannot read this page's URL
// (the session token), the dashboard DOM, or the SSE stream, and cannot redirect
// the top page. Latest-only: one iframe, its `src` swapped on each new card.
let frame = null;

// Appended to every card document so the parent can size the frame to its
// content (iframes don't auto-grow). It only ever posts a plain number back.
const RESIZE_REPORTER =
  "<scr" + "ipt>(function(){function r(){parent.postMessage(" +
  "{zoidCardHeight:document.documentElement.scrollHeight},'*');}" +
  "new ResizeObserver(r).observe(document.documentElement);" +
  "addEventListener('load',r);r();})()</scr" + "ipt>";

// Wrap the agent HTML in a minimal document: inherits the shell's typography,
// resets body margin, and carries the resize reporter.
const cardDoc = (html) =>
  "<!doctype html><meta charset=utf-8>" +
  "<style>html{font-family:system-ui,sans-serif;color:#191d24;font-size:15px}" +
  "body{margin:0}</style>" +
  html +
  RESIZE_REPORTER;

es.addEventListener("card", (e) => {
  const html = JSON.parse(e.data);
  if (!frame) {
    frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("title", "agent card");
    frame.style.width = "100%";
    frame.style.border = "0";
    frame.style.height = "120px";
    card.appendChild(frame);
  }
  frame.src =
    "data:text/html;charset=utf-8," + encodeURIComponent(cardDoc(html));
  // Bring the freshest card into view without yanking the whole page around.
  card.scrollIntoView({ behavior: "smooth", block: "nearest" });
});

// Size the card iframe from its self-reported content height. We trust only a
// finite number, only from our own frame, and clamp it — a hostile card cannot
// do more than pick its own (bounded) height.
window.addEventListener("message", (e) => {
  if (!frame || e.source !== frame.contentWindow) return;
  const h = e.data && e.data.zoidCardHeight;
  if (typeof h === "number" && isFinite(h)) {
    frame.style.height = Math.min(Math.max(h, 40), 20000) + "px";
  }
});
