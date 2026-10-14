use crate::openai::types::{ChatCompletionRequest, FunctionCall, ToolCall};
use uuid::Uuid;

pub struct PromptTranslator;

impl PromptTranslator {
    pub fn build_gemini_prompt(req: &ChatCompletionRequest) -> String {
        let mut prompt = String::new();

        // 1. Tool definitions (if any)
        if let Some(tools) = &req.tools
            && !tools.is_empty()
        {
            prompt.push_str("=== AVAILABLE TOOLS ===\n");
            prompt.push_str("You have access to the following tools/functions. If you need to call a tool, you MUST respond ONLY with the tool call in the exact format:\n");
            prompt.push_str(
                "<tool_call>\n{\"name\": \"tool_name\", \"arguments\": { ... }}\n</tool_call>\n\n",
            );
            prompt.push_str("Tool specifications:\n");
            for tool in tools {
                if let Ok(json_str) = serde_json::to_string_pretty(&tool.function) {
                    prompt.push_str(&json_str);
                    prompt.push('\n');
                }
            }
            prompt.push_str("=== END TOOLS ===\n\n");
        }

        // 2. Format conversation history
        for msg in &req.messages {
            match msg.role.as_str() {
                "system" => {
                    prompt.push_str("[System Instruction]\n");
                    prompt.push_str(&msg.content_as_str());
                    prompt.push_str("\n\n");
                }
                "user" => {
                    prompt.push_str("[User]\n");
                    prompt.push_str(&msg.content_as_str());
                    prompt.push_str("\n\n");
                }
                "assistant" => {
                    prompt.push_str("[Assistant]\n");
                    if let Some(tool_calls) = &msg.tool_calls {
                        for call in tool_calls {
                            prompt.push_str(&format!(
                                "<tool_call>\n{{\"name\": \"{}\", \"arguments\": {}}}\n</tool_call>\n",
                                call.function.name, call.function.arguments
                            ));
                        }
                    }
                    let content = msg.content_as_str();
                    if !content.is_empty() {
                        prompt.push_str(&content);
                    }
                    prompt.push_str("\n\n");
                }
                "tool" => {
                    let tool_id = msg.tool_call_id.as_deref().unwrap_or("unknown");
                    let tool_name = msg.name.as_deref().unwrap_or("unknown_tool");
                    prompt.push_str(&format!(
                        "[Tool Result for {} (ID: {})]\n",
                        tool_name, tool_id
                    ));
                    prompt.push_str(&msg.content_as_str());
                    prompt.push_str("\n\n");
                }
                _ => {
                    prompt.push_str(&format!("[{}]\n{}\n\n", msg.role, msg.content_as_str()));
                }
            }
        }

        prompt.trim().to_string()
    }

    pub fn parse_response(content: &str) -> (Option<String>, Option<Vec<ToolCall>>) {
        let tool_call_tag_start = "<tool_call>";
        let tool_call_tag_end = "</tool_call>";

        if let Some(start_idx) = content.find(tool_call_tag_start)
            && let Some(end_idx) = content.find(tool_call_tag_end)
        {
            let json_slice = &content[start_idx + tool_call_tag_start.len()..end_idx].trim();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_slice) {
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let args = val
                    .get("arguments")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());

                let tool_call = ToolCall {
                    id: format!("call_{}", Uuid::new_v4().simple()),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: name.to_string(),
                        arguments: args,
                    },
                };

                let text_before = content[..start_idx].trim();
                let text_after = content[end_idx + tool_call_tag_end.len()..].trim();
                let remaining_text = format!("{} {}", text_before, text_after).trim().to_string();

                let text_opt = if remaining_text.is_empty() {
                    None
                } else {
                    Some(remaining_text)
                };

                return (text_opt, Some(vec![tool_call]));
            }
        }

        (Some(content.to_string()), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plain_text() {
        let input = "Hello, world! This is a simple test.";
        let (text, tool_calls) = PromptTranslator::parse_response(input);
        assert_eq!(text, Some(input.to_string()));
        assert!(tool_calls.is_none());
    }

    #[test]
    fn test_parse_tool_call() {
        let input = "I will check the file.\n<tool_call>{\"name\": \"read_file\", \"arguments\": {\"path\": \"/tmp/test.txt\"}}</tool_call>";
        let (text, tool_calls) = PromptTranslator::parse_response(input);
        assert_eq!(text, Some("I will check the file.".to_string()));
        assert!(tool_calls.is_some());
        let calls = tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read_file");
        assert!(calls[0].function.arguments.contains("/tmp/test.txt"));
    }

    #[test]
    fn test_parse_standalone_tool_call() {
        let input = "<tool_call>{\"name\": \"get_weather\", \"arguments\": {\"location\": \"Hanoi\"}}</tool_call>";
        let (text, tool_calls) = PromptTranslator::parse_response(input);
        assert!(text.is_none());
        assert!(tool_calls.is_some());
        let calls = tool_calls.unwrap();
        assert_eq!(calls[0].function.name, "get_weather");
    }
}
