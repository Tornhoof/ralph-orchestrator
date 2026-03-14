//! Copilot stream event types for parsing `--output-format json` output.
//!
//! Copilot emits JSONL in prompt mode. This module provides lightweight parsing
//! helpers for extracting assistant text from those events.

use std::collections::HashSet;

use serde_json::Value;

/// Assistant message payload emitted by Copilot.
#[derive(Debug, Clone, PartialEq)]
pub struct CopilotAssistantMessage {
    pub message_id: Option<String>,
    pub content: Value,
}

/// Assistant message delta payload emitted by Copilot while streaming a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct CopilotAssistantMessageDelta {
    pub message_id: Option<String>,
    pub delta_content: String,
}

/// Events emitted by Copilot's `--output-format json`.
#[derive(Debug, Clone, PartialEq)]
pub enum CopilotStreamEvent {
    /// Assistant message content.
    AssistantMessage { data: CopilotAssistantMessage },
    /// Incremental assistant message content.
    AssistantMessageDelta { data: CopilotAssistantMessageDelta },
    /// Any other event type that Ralph currently ignores.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopilotLiveChunk {
    pub text: String,
    pub append_newline: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct CopilotStreamState {
    streamed_message_ids: HashSet<String>,
}

impl CopilotStreamState {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// Parses JSONL lines from Copilot's prompt-mode output.
pub struct CopilotStreamParser;

impl CopilotStreamParser {
    /// Parse a single line of JSONL output.
    ///
    /// Returns `None` for empty lines or malformed JSON.
    pub fn parse_line(line: &str) -> Option<CopilotStreamEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return None;
        }

        let value = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(e) => {
                tracing::debug!(
                    "Skipping malformed JSON line: {} (error: {})",
                    truncate(trimmed, 100),
                    e
                );
                return None;
            }
        };

        match value.get("type").and_then(Value::as_str) {
            Some("assistant.message") => Some(CopilotStreamEvent::AssistantMessage {
                data: CopilotAssistantMessage {
                    message_id: value
                        .get("data")
                        .and_then(|data| data.get("messageId"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    content: value
                        .get("data")
                        .and_then(|data| data.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null),
                },
            }),
            Some("assistant.message_delta") => Some(CopilotStreamEvent::AssistantMessageDelta {
                data: CopilotAssistantMessageDelta {
                    message_id: value
                        .get("data")
                        .and_then(|data| data.get("messageId"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    delta_content: value
                        .get("data")
                        .and_then(|data| data.get("deltaContent"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                        .unwrap_or_default(),
                },
            }),
            Some(_) => Some(CopilotStreamEvent::Other),
            None => None,
        }
    }

    /// Extract assistant text from a single Copilot JSONL line.
    pub fn extract_text(line: &str) -> Option<String> {
        match Self::parse_line(line)? {
            CopilotStreamEvent::AssistantMessage { data } => extract_content_text(&data.content),
            CopilotStreamEvent::AssistantMessageDelta { .. } | CopilotStreamEvent::Other => None,
        }
    }

    /// Extract text for live rendering, using deltas when available and
    /// suppressing duplicate full-message replays for message IDs already
    /// streamed incrementally.
    pub(crate) fn extract_live_chunk(
        line: &str,
        state: &mut CopilotStreamState,
    ) -> Option<CopilotLiveChunk> {
        match Self::parse_line(line)? {
            CopilotStreamEvent::AssistantMessageDelta { data } => {
                if let Some(message_id) = data.message_id {
                    state.streamed_message_ids.insert(message_id);
                }

                if data.delta_content.is_empty() {
                    None
                } else {
                    Some(CopilotLiveChunk {
                        text: data.delta_content,
                        append_newline: false,
                    })
                }
            }
            CopilotStreamEvent::AssistantMessage { data } => {
                if data
                    .message_id
                    .as_deref()
                    .is_some_and(|message_id| state.streamed_message_ids.contains(message_id))
                {
                    return Some(CopilotLiveChunk {
                        text: String::new(),
                        append_newline: true,
                    });
                }

                extract_content_text(&data.content).map(|text| CopilotLiveChunk {
                    text,
                    append_newline: true,
                })
            }
            CopilotStreamEvent::Other => None,
        }
    }

    /// Extract assistant text from a full Copilot JSONL payload.
    pub fn extract_all_text(raw_output: &str) -> String {
        let mut extracted = String::new();

        for line in raw_output.lines() {
            let Some(text) = Self::extract_text(line) else {
                continue;
            };
            Self::append_text_chunk(&mut extracted, &text);
        }

        extracted
    }

    /// Appends text while preserving message boundaries for downstream parsing.
    pub fn append_text_chunk(output: &mut String, chunk: &str) {
        output.push_str(chunk);
        if !chunk.ends_with('\n') {
            output.push('\n');
        }
    }
}

fn extract_content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let mut combined = String::new();
            for item in items {
                let text = match item {
                    Value::String(text) => Some(text.clone()),
                    Value::Object(map) => map
                        .get("text")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned),
                    _ => None,
                };
                if let Some(text) = text {
                    combined.push_str(&text);
                }
            }

            if combined.is_empty() {
                None
            } else {
                Some(combined)
            }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i < max_len)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &s[..boundary])
    }
}

#[cfg(test)]
mod tests {
    use super::{CopilotLiveChunk, CopilotStreamEvent, CopilotStreamParser, CopilotStreamState};
    use serde_json::Value;

    #[test]
    fn test_parse_assistant_message_content() {
        let line = r#"{"type":"assistant.message","data":{"messageId":"msg-1","content":"hello world","toolRequests":[]}}"#;
        let event = CopilotStreamParser::parse_line(line).unwrap();

        match event {
            CopilotStreamEvent::AssistantMessage { data } => {
                assert_eq!(data.message_id.as_deref(), Some("msg-1"));
                assert_eq!(data.content, Value::String("hello world".to_string()));
            }
            _ => panic!("Expected AssistantMessage event"),
        }
    }

    #[test]
    fn test_parse_assistant_message_delta() {
        let line = r#"{"type":"assistant.message_delta","data":{"messageId":"msg-1","deltaContent":"hello"}}"#;
        let event = CopilotStreamParser::parse_line(line).unwrap();

        match event {
            CopilotStreamEvent::AssistantMessageDelta { data } => {
                assert_eq!(data.message_id.as_deref(), Some("msg-1"));
                assert_eq!(data.delta_content, "hello");
            }
            _ => panic!("Expected AssistantMessageDelta event"),
        }
    }

    #[test]
    fn test_extract_text_ignores_non_assistant_lines() {
        let line = r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#;
        assert_eq!(CopilotStreamParser::extract_text(line), None);
    }

    #[test]
    fn test_extract_live_chunk_streams_deltas_without_duplication() {
        let mut state = CopilotStreamState::new();
        let delta = r#"{"type":"assistant.message_delta","data":{"messageId":"msg-1","deltaContent":"Hello"}}"#;
        let message =
            r#"{"type":"assistant.message","data":{"messageId":"msg-1","content":"Hello"}}"#;

        assert_eq!(
            CopilotStreamParser::extract_live_chunk(delta, &mut state),
            Some(CopilotLiveChunk {
                text: "Hello".to_string(),
                append_newline: false,
            })
        );
        assert_eq!(
            CopilotStreamParser::extract_live_chunk(message, &mut state),
            Some(CopilotLiveChunk {
                text: String::new(),
                append_newline: true,
            })
        );
    }

    #[test]
    fn test_extract_all_text_aggregates_text_from_jsonl() {
        let raw = concat!(
            "{\"type\":\"assistant.turn_start\",\"data\":{\"turnId\":\"0\"}}\n",
            "{\"type\":\"assistant.message_delta\",\"data\":{\"messageId\":\"msg-1\",\"deltaContent\":\"ignored\"}}\n",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":\"First line\"}}\n",
            "{\"type\":\"assistant.message\",\"data\":{\"content\":\"LOOP_COMPLETE\"}}\n",
            "{\"type\":\"result\",\"exitCode\":0}\n"
        );

        assert_eq!(
            CopilotStreamParser::extract_all_text(raw),
            "First line\nLOOP_COMPLETE\n"
        );
    }
}
