//! Dev harness for eyeballing/verifying the companion server in a real browser.
//! Starts the actual server (real CSP header, real routes) with a fixed token,
//! publishes one dashboard snapshot and one *interactive* card, then parks.
//!
//! Run: `cargo run -p zoid-companion --example serve`
//! then open the printed URL. The card probes whether its sandboxed script can
//! reach the parent (it must NOT) and increments a counter (it must).

use std::time::Duration;

use zoid_companion::{start, CompanionHub, DashboardSnapshot, TierRow};

fn main() {
    let hub = CompanionHub::new();
    hub.set_enabled(true);

    let token = "devtoken1234".to_string();
    let server = start(hub.clone(), 0, token).expect("bind companion server");
    println!("\n  companion up → {}\n", server.url);

    hub.publish_snapshot(DashboardSnapshot {
        session_name: "dev-harness".into(),
        model: "glm-5.2:cloud".into(),
        provider: "ollama".into(),
        cwd: "/home/x/zoid".into(),
        ctx_used: 312_000,
        ctx_ceiling: 384_000,
        session_tokens: 1200,
        cached_tokens: 200,
        cache_supported: true,
        tasks_len: 2,
        busy: false,
        tiers: vec![
            TierRow {
                label: "system".into(),
                tokens: 1200,
                heat: 2,
                cold: false,
                pinned: true,
            },
            TierRow {
                label: "older turns".into(),
                tokens: 4200,
                heat: 0,
                cold: true,
                pinned: false,
            },
        ],
        churn: vec![10, 40, 25, 60, 30, 80],
        updated_ms: 1_700_000_000_000,
    });

    // An interactive card that ALSO probes the sandbox boundary: reading
    // `parent.location` must throw (opaque origin), proving card JS cannot see
    // the token in the shell's URL. The counter proves inline JS runs at all.
    let card = r##"
<h2 style="font:600 18px/1.3 system-ui;margin:.2em 0">Interactive card ✅</h2>
<button id="b" style="font-size:15px;padding:.45em .9em;border-radius:8px;border:1px solid #0f7d72;background:#0f7d72;color:#fff;cursor:pointer">clicked 0×</button>
<pre id="probe" style="background:#f4f1ea;padding:.7em;border-radius:8px;white-space:pre-wrap;font:13px ui-monospace,monospace;margin-top:.8em"></pre>
<script>
  let n = 0;
  const b = document.getElementById('b');
  b.onclick = () => { b.textContent = 'clicked ' + (++n) + '×'; };

  const out = [];
  out.push('inline script ran: YES');
  out.push('my origin (should be opaque): ' + document.location.protocol);
  try {
    out.push('parent.location.href: ' + parent.location.href + '  <-- LEAK!');
  } catch (err) {
    out.push('parent.location read: BLOCKED (' + err.name + ')  <-- good');
  }
  try {
    const t = parent.document.getElementById('dashboard');
    out.push('parent dashboard DOM: ' + (t ? 'READABLE <-- LEAK!' : 'null'));
  } catch (err) {
    out.push('parent DOM read: BLOCKED (' + err.name + ')  <-- good');
  }
  document.getElementById('probe').textContent = out.join('\n');
</script>
"##;
    hub.publish_card(card.to_string());

    println!("  (snapshot + interactive card published; Ctrl-C to stop)\n");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
