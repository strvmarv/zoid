use super::types::{AnthropicRequest, CacheControl, CacheKind, ContentBlock, MessageContent};

/// Place ephemeral (1h) cache breakpoints on the system block and on the last
/// message's last block. Interior messages stay plain. Mirrors the rolling-
/// breakpoint behavior of the legacy `request_body` (anthropic.rs:42-58): the
/// previous turn's breakpoint becomes an interior read on the next turn, and
/// the new breakpoint extends the cached prefix.
pub fn place_breakpoints(req: &mut AnthropicRequest) {
    if let Some(sys) = req.system.as_mut() {
        for block in sys.iter_mut() {
            block.cache_control = Some(CacheControl {
                kind: CacheKind::Ephemeral1h,
            });
        }
    }
    if let Some(last_msg) = req.messages.last_mut() {
        if let MessageContent::Blocks(blocks) = &mut last_msg.content {
            if let Some(ContentBlock::Text { cache_control, .. }) = blocks.last_mut() {
                *cache_control = Some(CacheControl {
                    kind: CacheKind::Ephemeral1h,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::place_breakpoints;
    use crate::anthropic::types::{
        AnthropicMessage, AnthropicRequest, AnthropicRole, CacheControl, CacheKind, ContentBlock,
        MessageContent, SystemBlock,
    };

    fn user_text(s: &str) -> AnthropicMessage {
        AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Text(s.into()),
        }
    }

    fn user_blocks(blocks: Vec<ContentBlock>) -> AnthropicMessage {
        AnthropicMessage {
            role: AnthropicRole::User,
            content: MessageContent::Blocks(blocks),
        }
    }

    #[test]
    fn places_breakpoint_on_system_when_present() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_text("x")],
            system: Some(vec![SystemBlock {
                text: "be terse".into(),
                cache_control: None,
            }]),
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        let sys = req.system.as_ref().unwrap();
        assert_eq!(
            sys[0].cache_control,
            Some(CacheControl {
                kind: CacheKind::Ephemeral1h
            })
        );
    }

    #[test]
    fn no_system_no_system_breakpoint() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_text("x")],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        assert!(req.system.is_none());
    }

    #[test]
    fn places_breakpoint_on_last_message_last_block_only() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![
                user_text("a"),
                AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: MessageContent::Text("b".into()),
                },
                user_blocks(vec![ContentBlock::Text {
                    text: "c".into(),
                    cache_control: None,
                }]),
            ],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        // interior messages unchanged
        assert!(matches!(req.messages[0].content, MessageContent::Text(_)));
        assert!(matches!(req.messages[1].content, MessageContent::Text(_)));
        // last message's last block gets the breakpoint
        match &req.messages[2].content {
            MessageContent::Blocks(blocks) => match &blocks[0] {
                ContentBlock::Text { cache_control, .. } => assert_eq!(
                    *cache_control,
                    Some(CacheControl {
                        kind: CacheKind::Ephemeral1h
                    })
                ),
                other => panic!("got {other:?}"),
            },
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn interior_blocks_stay_plain() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![user_blocks(vec![
                ContentBlock::Text {
                    text: "first".into(),
                    cache_control: None,
                },
                ContentBlock::Text {
                    text: "second".into(),
                    cache_control: None,
                },
            ])],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req);
        match &req.messages[0].content {
            MessageContent::Blocks(blocks) => {
                // first block stays plain
                assert!(matches!(
                    &blocks[0],
                    ContentBlock::Text {
                        cache_control: None,
                        ..
                    }
                ));
                // last block gets the breakpoint
                assert!(matches!(
                    &blocks[1],
                    ContentBlock::Text {
                        cache_control: Some(_),
                        ..
                    }
                ));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn empty_messages_no_panic() {
        let mut req = AnthropicRequest {
            model: "m".into(),
            max_tokens: 8,
            stream: true,
            messages: vec![],
            system: None,
            tools: vec![],
            thinking: None,
        };
        place_breakpoints(&mut req); // must not panic
    }
}
