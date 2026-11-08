use rillwright_engine::chat::Message;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolState {
    Pending,
    Processing,
    WaitingForApproval,
    Completed,
    Failed,
    NotExecuted,
    OutcomeUnknown,
    ResultAvailable,
}

impl ToolState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Processing => "Processing",
            Self::WaitingForApproval => "Waiting for approval",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::NotExecuted => "Not executed",
            Self::OutcomeUnknown => "Outcome unknown",
            Self::ResultAvailable => "Result available",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolCallView {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
    pub result: Option<String>,
    pub state: ToolState,
    pub is_error: Option<bool>,
    pub truncated: Option<bool>,
    pub approval_request: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum ConversationItem {
    Text {
        id: String,
        role: String,
        body: String,
    },
    Reasoning {
        id: String,
        body: String,
    },
    Tool(ToolCallView),
    Notice {
        id: String,
        title: String,
        body: String,
    },
}

impl ConversationItem {
    pub fn tool(&self) -> Option<&ToolCallView> {
        if let Self::Tool(tool) = self {
            Some(tool)
        } else {
            None
        }
    }
    pub fn tool_mut(&mut self) -> Option<&mut ToolCallView> {
        if let Self::Tool(tool) = self {
            Some(tool)
        } else {
            None
        }
    }
}

pub fn project_messages(messages: &[Message], show_reasoning: bool) -> Vec<ConversationItem> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let message = &messages[i];
        if message.role == "system" {
            i += 1;
            continue;
        }
        let id = format!("message:{i}");
        match message.role.as_str() {
            "assistant" => {
                if show_reasoning
                    && let Some(reasoning) = &message.reasoning_content
                    && !reasoning.is_empty()
                {
                    out.push(ConversationItem::Reasoning {
                        id: format!("{id}:reasoning"),
                        body: reasoning.clone(),
                    });
                }
                if !message.content.is_empty() {
                    out.push(ConversationItem::Text {
                        id: format!("{id}:text"),
                        role: "Dory".into(),
                        body: message.content.clone(),
                    });
                }
                for (ordinal, call) in message.tool_calls.iter().enumerate() {
                    out.push(ConversationItem::Tool(ToolCallView {
                        id: format!("{id}:call:{ordinal}"),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        result: None,
                        state: ToolState::Processing,
                        is_error: None,
                        truncated: None,
                        approval_request: None,
                    }));
                }
                let call_count = message.tool_calls.len();
                let mut paired = 0;
                let mut j = i + 1;
                while j < messages.len() && messages[j].role == "tool" {
                    if paired < call_count {
                        let result = messages[j].content.clone();
                        if let Some(item) = out.iter_mut().rev().find(|item| {
                            item.tool()
                                .is_some_and(|t| t.id == format!("{id}:call:{paired}"))
                        }) {
                            let tool = item.tool_mut().unwrap();
                            tool.state = if result == rillwright_agent::session::UNKNOWN_OUTCOME {
                                ToolState::OutcomeUnknown
                            } else {
                                ToolState::ResultAvailable
                            };
                            tool.result = Some(result);
                        }
                        paired += 1;
                    } else {
                        out.push(ConversationItem::Notice {
                            id: format!("message:{j}:orphan"),
                            title: "Unmatched tool result".into(),
                            body: messages[j].content.clone(),
                        });
                    }
                    j += 1;
                }
                i = j;
            }
            "user" => {
                out.push(ConversationItem::Text {
                    id: format!("{id}:text"),
                    role: "You".into(),
                    body: message.content.clone(),
                });
                i += 1;
            }
            "tool" => {
                out.push(ConversationItem::Notice {
                    id: format!("{id}:orphan"),
                    title: "Unmatched tool result".into(),
                    body: message.content.clone(),
                });
                i += 1;
            }
            _ => {
                out.push(ConversationItem::Notice {
                    id: format!("{id}:notice"),
                    title: message.role.clone(),
                    body: message.content.clone(),
                });
                i += 1;
            }
        }
    }
    out
}

pub fn summary(tool: &ToolCallView) -> String {
    let get = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| tool.arguments.get(*k).and_then(Value::as_str))
    };
    let value = match tool.name.as_str() {
        "read_file" | "write_file" | "list_dir" => get(&["path", "file_path"]),
        "run_command" => {
            let command = get(&["command"]).unwrap_or("command");
            let cwd = get(&["cwd"]);
            return match cwd {
                Some(cwd) => format!("{} · cwd {cwd}", bounded(command, 72)),
                None => bounded(command, 100),
            };
        }
        "web_search" => get(&["query"]),
        _ => get(&["path", "command", "query", "url"]),
    };
    value
        .map(|s| bounded(s, 100))
        .unwrap_or_else(|| "Tool call".into())
}

pub fn bounded(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rillwright_engine::chat::ToolCall;

    #[test]
    fn call_only_message_projects_distinct_cards_with_ordered_results() {
        let arguments = serde_json::json!({"path":"same.txt"})
            .as_object()
            .unwrap()
            .clone();
        let messages = vec![
            Message {
                role: "assistant".into(),
                content: String::new(),
                reasoning_content: None,
                tool_calls: vec![
                    ToolCall {
                        name: "read_file".into(),
                        arguments: arguments.clone(),
                    },
                    ToolCall {
                        name: "read_file".into(),
                        arguments,
                    },
                ],
            },
            Message::new("tool", "first result"),
            Message::new("tool", "second result"),
        ];
        let items = project_messages(&messages, true);
        assert_eq!(items.len(), 2);
        let calls: Vec<_> = items.iter().filter_map(ConversationItem::tool).collect();
        assert_ne!(calls[0].id, calls[1].id);
        assert_eq!(calls[0].result.as_deref(), Some("first result"));
        assert_eq!(calls[1].result.as_deref(), Some("second result"));
        assert_eq!(calls[0].state, ToolState::ResultAvailable);
    }

    #[test]
    fn unknown_result_and_orphan_are_preserved() {
        let messages = vec![
            Message {
                role: "assistant".into(),
                content: "checking".into(),
                reasoning_content: None,
                tool_calls: vec![ToolCall {
                    name: "run_command".into(),
                    arguments: Map::new(),
                }],
            },
            Message::new("tool", rillwright_agent::session::UNKNOWN_OUTCOME),
            Message::new("tool", "orphan output"),
        ];
        let items = project_messages(&messages, false);
        assert_eq!(items.len(), 3);
        assert_eq!(items[1].tool().unwrap().state, ToolState::OutcomeUnknown);
        assert!(
            matches!(&items[2], ConversationItem::Notice { body, .. } if body == "orphan output")
        );
    }

    #[test]
    fn preview_boundary_keeps_unicode_valid() {
        assert_eq!(bounded("ééé", 3), "é…");
    }
}
