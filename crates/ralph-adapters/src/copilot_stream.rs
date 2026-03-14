//! Copilot stream event types for parsing `--output-format json` output.
//!
//! Copilot emits JSONL in prompt mode. This module provides lightweight parsing
//! helpers for extracting assistant text from those events.

use serde_json::Value;

/// Assistant message payload emitted by Copilot.
#[derive(Debug, Clone, PartialEq)]
pub struct CopilotAssistantMessage {
    pub content: Value,
}

/// Events emitted by Copilot's `--output-format json`.
#[derive(Debug, Clone, PartialEq)]
pub enum CopilotStreamEvent {
    /// Assistant message content.
    AssistantMessage { data: CopilotAssistantMessage },
    /// Any other event type that Ralph currently ignores.
    Other,
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
                    content: value
                        .get("data")
                        .and_then(|data| data.get("content"))
                        .cloned()
                        .unwrap_or(Value::Null),
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
    use super::{CopilotStreamEvent, CopilotStreamParser};
    use serde_json::Value;

    #[test]
    fn test_parse_assistant_message_content() {
        let line =
            r#"{"type":"assistant.message","data":{"content":"hello world","toolRequests":[]}}"#;
        let event = CopilotStreamParser::parse_line(line).unwrap();

        match event {
            CopilotStreamEvent::AssistantMessage { data } => {
                assert_eq!(data.content, Value::String("hello world".to_string()));
            }
            CopilotStreamEvent::Other => panic!("Expected AssistantMessage event"),
        }
    }

    #[test]
    fn test_extract_text_ignores_non_assistant_lines() {
        let line = r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#;
        assert_eq!(CopilotStreamParser::extract_text(line), None);
    }

    #[test]
    fn test_extract_all_text_aggregates_text_from_jsonl() {
        let raw = concat!(
            "{\"type\":\"assistant.turn_start\",\"data\":{\"turnId\":\"0\"}}\n",
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
