use std::sync::Arc;
use tokio::sync::mpsc;
use ulid::Ulid;
use zoid::agent::{run_agent_turn, AgentUpdate};
use zoid_core::assembler::ContextPolicy;
use zoid_core::economy::token_ledger;
use zoid_core::event::{Event, EventKind};
use zoid_core::session::SessionHandle;
use zoid_provider::{FakeProvider, ProviderEvent, Usage};

fn now() -> i64 {
    0
}

#[tokio::test]
async fn turn_usage_lands_in_ledger() {
    // Fake provider: a little text, then a usage report, then done.
    let provider = Arc::new(FakeProvider::new(vec![
        ProviderEvent::TextDelta("hello".into()),
        ProviderEvent::Usage(Usage {
            input_tokens: 120,
            output_tokens: 18,
            cached: 0,
            thinking_tokens: 0,
        }),
        ProviderEvent::Done,
    ]));
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let session = SessionHandle::spawn(tmp.path().to_str().unwrap()).unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage { text: "hi".into() },
    )];
    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    run_agent_turn(
        zoid::agent::chat_turn_config(),
        provider,
        Arc::new(zoid_tools::registry()),
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "m".into(),
        tx,
        Ulid::new(),
        zoid_companion::CompanionHub::new(),
        now,
    )
    .await
    .unwrap();
    while rx.recv().await.is_some() {}

    let events = session.snapshot().await.unwrap();
    let ledger = token_ledger(&events);
    assert_eq!(ledger.input, 120);
    assert_eq!(ledger.output, 18);
    assert_eq!(ledger.total, 138);
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::Usage)));
}

#[tokio::test]
async fn oversized_tool_result_is_compacted_when_over_threshold() {
    // A shell command whose stdout is large/multi-line enough that its
    // ToolResult blows past a tiny compaction threshold. `shell` (unlike
    // `read_file`) has no path key, so its output stays an `ItemKind::ToolResult`
    // (compactable) rather than folding into an `ItemKind::File` item.
    let command =
        "for i in $(seq 1 2000); do echo \"line $i: filler text to pad out tokens\"; done";

    // Provider script: one tool call to a shell-like tool, then a final message.
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("shell", serde_json::json!({ "command": command })),
        zoid_testkit::text("done"),
        ProviderEvent::Done,
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage {
            text: "run the big command".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let mut config = zoid::agent::chat_turn_config();
    config.policy = ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: false,
        compact_threshold: Some(50), // tiny: the tool-result alone dwarfs this
    };

    let events = run_agent_turn(
        config,
        provider,
        Arc::new(zoid_tools::registry()),
        Arc::new(zoid_tools::AllowAll),
        session.clone(),
        zoid::eventlog::EventLog::from_vec(seed),
        "fake".into(),
        tx,
        Ulid::new(),
        zoid_companion::CompanionHub::new(),
        now,
    )
    .await
    .unwrap();
    let _ = drain.await;

    let compacted = events
        .iter()
        .any(|e| matches!(e.kind, EventKind::ToolResultCompacted { .. }));
    assert!(
        compacted,
        "a large tool-result over threshold must be compacted"
    );
}

#[tokio::test]
async fn compaction_emits_started_and_complete_updates() {
    let command =
        "for i in $(seq 1 2000); do echo \"line $i: filler text to pad out tokens\"; done";

    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("shell", serde_json::json!({ "command": command })),
        zoid_testkit::text("done"),
        ProviderEvent::Done,
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage {
            text: "run the big command".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    let mut config = zoid::agent::chat_turn_config();
    config.policy = ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: false,
        compact_threshold: Some(50),
    };

    let handle = tokio::spawn(async move {
        run_agent_turn(
            config,
            provider,
            Arc::new(zoid_tools::registry()),
            Arc::new(zoid_tools::AllowAll),
            session,
            zoid::eventlog::EventLog::from_vec(seed),
            "fake".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            now,
        )
        .await
        .unwrap();
    });

    let mut saw_started = false;
    let mut saw_complete = false;
    while let Some(update) = rx.recv().await {
        match update {
            AgentUpdate::CompactionStarted => saw_started = true,
            AgentUpdate::CompactionComplete => saw_complete = true,
            _ => {}
        }
    }
    let _ = handle.await;

    assert!(saw_started, "CompactionStarted must be emitted");
    assert!(saw_complete, "CompactionComplete must be emitted");
}

#[tokio::test]
async fn compaction_does_not_emit_updates_when_nothing_compacted() {
    // A turn with a tiny tool-result well below the compaction threshold —
    // plan_compactions returns empty, so no CompactionStarted/Complete.
    let provider = zoid_testkit::script(vec![
        zoid_testkit::tool_call("shell", serde_json::json!({ "command": "echo hi" })),
        zoid_testkit::text("done"),
        ProviderEvent::Done,
    ]);

    let session = SessionHandle::spawn(":memory:").unwrap();
    let seed = vec![Event::new(
        Ulid::new(),
        None,
        0,
        EventKind::UserMessage {
            text: "run the tiny command".into(),
        },
    )];
    session.append(seed[0].clone()).await.unwrap();

    let (tx, mut rx) = mpsc::channel::<AgentUpdate>(64);

    let mut config = zoid::agent::chat_turn_config();
    config.policy = ContextPolicy {
        token_ceiling: None,
        auto_evict_cold: false,
        compact_threshold: Some(50),
    };

    let handle = tokio::spawn(async move {
        run_agent_turn(
            config,
            provider,
            Arc::new(zoid_tools::registry()),
            Arc::new(zoid_tools::AllowAll),
            session,
            zoid::eventlog::EventLog::from_vec(seed),
            "fake".into(),
            tx,
            Ulid::new(),
            zoid_companion::CompanionHub::new(),
            now,
        )
        .await
        .unwrap();
    });

    let mut saw_any_compaction = false;
    while let Some(update) = rx.recv().await {
        match update {
            AgentUpdate::CompactionStarted | AgentUpdate::CompactionComplete => {
                saw_any_compaction = true;
            }
            _ => {}
        }
    }
    let _ = handle.await;

    assert!(
        !saw_any_compaction,
        "no CompactionStarted/Complete when nothing was compacted"
    );
}
