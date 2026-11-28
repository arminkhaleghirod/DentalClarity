//! Amazon Bedrock text adapter for clinical mode.
//!
//! Calls go through the Converse API, which takes the same request shape for
//! every model on Bedrock. Structured output uses tool use (DECISIONS D13):
//! the response schema becomes the input schema of one tool named
//! [`OUTPUT_TOOL`], and the tool call's input is returned as the reply's JSON
//! text. The tool choice is `auto`, because the newer Claude models reject a
//! forced tool choice; a model that answers with plain JSON text instead is
//! accepted too, and the pipeline's strict `serde` parse decides either way.
//!
//! `temperature` is not sent by default: the current Claude models on
//! Bedrock accept only their default sampling and answer 400 otherwise.
//! [`Bedrock::new`] refuses to build the adapter outside clinical mode.

use crate::{
    AiError, ProviderPolicy, Result, Role, TextModel, TextRequest, TextResponse, Usage,
    openrouter::OUTPUT_LIMIT_NO_ANSWER,
};
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse::{ConverseError, ConverseOutput};
use aws_sdk_bedrockruntime::types::{
    AutoToolChoice, ContentBlock, ConversationRole, ConverseOutput as OutputType,
    InferenceConfiguration, Message, StopReason, SystemContentBlock, Tool, ToolChoice,
    ToolConfiguration, ToolInputSchema, ToolSpecification,
};
use aws_smithy_types::{Document, Number};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Name of the tool that carries structured output.
pub const OUTPUT_TOOL: &str = "output";
/// Per-call timeout. The default Claude models think before answering even
/// when no reasoning effort is requested, so every call gets the reasoning
/// timeout of D12.
pub const TIMEOUT: Duration = crate::openrouter::REASONING_TIMEOUT;

pub struct Bedrock {
    client: Client,
    send_temperature: bool,
    timeout: Duration,
}

impl std::fmt::Debug for Bedrock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bedrock")
            .field("send_temperature", &self.send_temperature)
            .finish()
    }
}

impl Bedrock {
    pub fn new(policy: ProviderPolicy, config: &aws_config::SdkConfig) -> Result<Self> {
        policy.check_aws()?;
        Ok(Self {
            client: Client::new(config),
            send_temperature: false,
            timeout: TIMEOUT,
        })
    }

    /// Sends the request's temperature, for models that accept it.
    pub fn with_temperature(mut self, send: bool) -> Self {
        self.send_temperature = send;
        self
    }

    /// Replaces the per-call timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The parts of a Converse request built from a [`TextRequest`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConverseParts {
    pub system: Vec<SystemContentBlock>,
    pub messages: Vec<Message>,
    pub tool_config: Option<ToolConfiguration>,
    pub inference: InferenceConfiguration,
}

/// Builds the Converse request for `request`. System messages go to the
/// `system` field; consecutive messages of one role are joined into one
/// message, since Converse expects the roles to alternate.
pub fn converse_parts(request: &TextRequest, send_temperature: bool) -> Result<ConverseParts> {
    let mut system = Vec::new();
    let mut messages: Vec<(ConversationRole, Vec<ContentBlock>)> = Vec::new();
    for message in &request.messages {
        let role = match message.role {
            Role::System => {
                system.push(SystemContentBlock::Text(message.content.clone()));
                continue;
            }
            Role::User => ConversationRole::User,
            Role::Assistant => ConversationRole::Assistant,
        };
        let block = ContentBlock::Text(message.content.clone());
        match messages.last_mut() {
            Some((last, blocks)) if *last == role => blocks.push(block),
            _ => messages.push((role, vec![block])),
        }
    }
    if messages.first().map(|(r, _)| r) != Some(&ConversationRole::User) {
        return Err(AiError::Request(
            "a Bedrock conversation must start with a user message".into(),
        ));
    }
    let messages = messages
        .into_iter()
        .map(|(role, content)| {
            Message::builder()
                .role(role)
                .set_content(Some(content))
                .build()
                .map_err(|e| AiError::Request(e.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    let tool_config = match &request.response_schema {
        None => None,
        Some(schema) => {
            system.push(SystemContentBlock::Text(format!(
                "Give your answer by calling the `{OUTPUT_TOOL}` tool exactly once, with \
                 input that follows its schema. Do not write the answer as text."
            )));
            let spec = ToolSpecification::builder()
                .name(OUTPUT_TOOL)
                .description("Returns the answer in the required JSON format.")
                .input_schema(ToolInputSchema::Json(json_to_document(schema)))
                .build()
                .map_err(|e| AiError::Request(e.to_string()))?;
            Some(
                ToolConfiguration::builder()
                    .tools(Tool::ToolSpec(spec))
                    .tool_choice(ToolChoice::Auto(AutoToolChoice::builder().build()))
                    .build()
                    .map_err(|e| AiError::Request(e.to_string()))?,
            )
        }
    };

    let max_tokens = i32::try_from(request.max_tokens).unwrap_or(i32::MAX);
    let mut inference = InferenceConfiguration::builder().max_tokens(max_tokens);
    if send_temperature {
        inference = inference.temperature(request.temperature);
    }
    Ok(ConverseParts {
        system,
        messages,
        tool_config,
        inference: inference.build(),
    })
}

/// Reads a Converse reply: the [`OUTPUT_TOOL`] call's input as JSON text when
/// there is one, otherwise the text blocks joined. Reasoning blocks are
/// ignored.
pub fn parse_output(
    output: &ConverseOutput,
    requested: &str,
    latency_ms: u64,
) -> Result<TextResponse> {
    let stop = output.stop_reason();
    if matches!(
        stop,
        StopReason::GuardrailIntervened | StopReason::ContentFiltered
    ) {
        return Err(AiError::BadResponse(format!(
            "the reply was filtered ({})",
            stop.as_str()
        )));
    }
    let blocks: &[ContentBlock] = match output.output() {
        Some(OutputType::Message(message)) => message.content(),
        _ => &[],
    };
    let tool_input = blocks.iter().find_map(|b| match b {
        ContentBlock::ToolUse(call) if call.name() == OUTPUT_TOOL => Some(call.input()),
        _ => None,
    });
    let content = match tool_input {
        Some(input) => document_to_json(input).to_string(),
        None => blocks
            .iter()
            .filter_map(|b| b.as_text().ok().map(String::as_str))
            .collect::<Vec<_>>()
            .join(""),
    };
    if content.trim().is_empty() {
        return Err(AiError::BadResponse(if *stop == StopReason::MaxTokens {
            OUTPUT_LIMIT_NO_ANSWER.into()
        } else {
            "no message content".into()
        }));
    }
    let usage = output.usage().map_or_else(Usage::default, |u| Usage {
        input_tokens: u32::try_from(u.input_tokens()).unwrap_or(0),
        output_tokens: u32::try_from(u.output_tokens()).unwrap_or(0),
        // Converse reports tokens, not a price.
        cost_usd: None,
    });
    Ok(TextResponse {
        model: requested.to_owned(),
        route: Some("bedrock".into()),
        content,
        usage,
        latency_ms,
    })
}

/// Maps a Converse failure onto the error kinds the callers retry on.
fn map_error(err: SdkError<ConverseError>) -> AiError {
    let service = match err {
        SdkError::ServiceError(e) => e.into_err(),
        SdkError::TimeoutError(_) => {
            return AiError::BadResponse("the Bedrock call timed out".into());
        }
        SdkError::ResponseError(_) => {
            return AiError::BadResponse("Bedrock sent a reply that could not be read".into());
        }
        other => return AiError::Request(format!("Bedrock request failed: {other}")),
    };
    let name = service.meta().code().unwrap_or("unknown").to_owned();
    match service {
        ConverseError::ThrottlingException(_) => AiError::Limited(name),
        ConverseError::ModelTimeoutException(_) => {
            AiError::BadResponse("the model timed out on Bedrock".into())
        }
        // Access, validation and missing-model errors are configuration
        // problems; the message is kept because it names what is wrong.
        other => AiError::Request(format!("{name}: {}", other.meta().message().unwrap_or(""))),
    }
}

#[async_trait]
impl TextModel for Bedrock {
    fn provider(&self) -> &'static str {
        "bedrock"
    }

    async fn generate(&self, request: TextRequest) -> Result<TextResponse> {
        let parts = converse_parts(&request, self.send_temperature)?;
        let started = Instant::now();
        let call = self
            .client
            .converse()
            .model_id(&request.model)
            .set_system(Some(parts.system))
            .set_messages(Some(parts.messages))
            .set_tool_config(parts.tool_config)
            .inference_config(parts.inference)
            .send();
        // A call that timed out may still be billed, so it is reported as an
        // unusable reply, as the OpenRouter adapter does (D12).
        let output = tokio::time::timeout(self.timeout, call)
            .await
            .map_err(|_| {
                AiError::BadResponse(format!("no reply within {} s", self.timeout.as_secs()))
            })?
            .map_err(map_error)?;
        parse_output(
            &output,
            &request.model,
            started.elapsed().as_millis() as u64,
        )
    }
}

/// Converts JSON to the SDK's document type.
pub fn json_to_document(value: &serde_json::Value) -> Document {
    use serde_json::Value;
    match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => Document::Number(if let Some(u) = n.as_u64() {
            Number::PosInt(u)
        } else if let Some(i) = n.as_i64() {
            Number::NegInt(i)
        } else {
            Number::Float(n.as_f64().unwrap_or(0.0))
        }),
        Value::String(s) => Document::String(s.clone()),
        Value::Array(items) => Document::Array(items.iter().map(json_to_document).collect()),
        Value::Object(map) => Document::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect::<HashMap<_, _>>(),
        ),
    }
}

/// Converts the SDK's document type to JSON.
pub fn document_to_json(doc: &Document) -> serde_json::Value {
    use serde_json::Value;
    match doc {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(Number::PosInt(u)) => Value::from(*u),
        Document::Number(Number::NegInt(i)) => Value::from(*i),
        Document::Number(Number::Float(f)) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Document::String(s) => Value::String(s.clone()),
        Document::Array(items) => Value::Array(items.iter().map(document_to_json).collect()),
        Document::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), document_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message as Msg;
    use aws_sdk_bedrockruntime::types::{ReasoningContentBlock, TokenUsage, ToolUseBlock};
    use dc_core::Mode;
    use serde_json::json;

    fn request(schema: Option<serde_json::Value>) -> TextRequest {
        TextRequest {
            model: "anthropic.claude-haiku-5-5".into(),
            messages: vec![
                Msg {
                    role: Role::System,
                    content: "rules".into(),
                },
                Msg {
                    role: Role::User,
                    content: "facts".into(),
                },
                Msg {
                    role: Role::User,
                    content: "task".into(),
                },
            ],
            response_schema: schema,
            max_tokens: 2000,
            temperature: 0.2,
            reasoning: None,
        }
    }

    fn reply(blocks: Vec<ContentBlock>, stop: StopReason) -> ConverseOutput {
        let message = Message::builder()
            .role(ConversationRole::Assistant)
            .set_content(Some(blocks))
            .build()
            .unwrap();
        ConverseOutput::builder()
            .output(OutputType::Message(message))
            .stop_reason(stop)
            .usage(
                TokenUsage::builder()
                    .input_tokens(120)
                    .output_tokens(45)
                    .total_tokens(165)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap()
    }

    #[test]
    fn builds_system_messages_and_joins_roles() {
        let parts = converse_parts(&request(None), false).unwrap();
        assert_eq!(parts.system, vec![SystemContentBlock::Text("rules".into())]);
        assert_eq!(parts.messages.len(), 1);
        assert_eq!(parts.messages[0].role(), &ConversationRole::User);
        assert_eq!(parts.messages[0].content().len(), 2);
        assert!(parts.tool_config.is_none());
        assert_eq!(parts.inference.max_tokens(), Some(2000));
        assert_eq!(parts.inference.temperature(), None);
        let parts = converse_parts(&request(None), true).unwrap();
        assert_eq!(parts.inference.temperature(), Some(0.2));
    }

    #[test]
    fn a_schema_becomes_the_output_tool() {
        let schema = json!({
            "type": "object",
            "properties": {"text": {"type": "string"}, "n": {"type": "integer", "minimum": -1}},
            "required": ["text"],
            "additionalProperties": false,
        });
        let parts = converse_parts(&request(Some(schema.clone())), false).unwrap();
        let config = parts.tool_config.unwrap();
        assert!(matches!(config.tool_choice(), Some(ToolChoice::Auto(_))));
        let Tool::ToolSpec(spec) = &config.tools()[0] else {
            panic!("expected a tool spec");
        };
        assert_eq!(spec.name(), OUTPUT_TOOL);
        let Some(ToolInputSchema::Json(doc)) = spec.input_schema() else {
            panic!("expected a JSON schema");
        };
        assert_eq!(document_to_json(doc), schema);
        assert_eq!(parts.system.len(), 2);
    }

    #[test]
    fn a_conversation_must_start_with_the_user() {
        let mut r = request(None);
        r.messages.retain(|m| m.role != Role::User);
        assert!(converse_parts(&r, false).is_err());
    }

    #[test]
    fn reads_the_tool_input_as_json() {
        let call = ToolUseBlock::builder()
            .tool_use_id("t1")
            .name(OUTPUT_TOOL)
            .input(json_to_document(&json!({"text": "hello", "n": 3})))
            .build()
            .unwrap();
        let out = reply(
            vec![
                ContentBlock::ReasoningContent(ReasoningContentBlock::RedactedContent(
                    aws_smithy_types::Blob::new(vec![1, 2]),
                )),
                ContentBlock::Text("Here you go.".into()),
                ContentBlock::ToolUse(call),
            ],
            StopReason::ToolUse,
        );
        let parsed = parse_output(&out, "m", 7).unwrap();
        let value: serde_json::Value = serde_json::from_str(&parsed.content).unwrap();
        assert_eq!(value, json!({"text": "hello", "n": 3}));
        assert_eq!(parsed.usage.input_tokens, 120);
        assert_eq!(parsed.usage.output_tokens, 45);
        assert_eq!(parsed.usage.cost_usd, None);
        assert_eq!(parsed.model, "m");
        assert_eq!(parsed.latency_ms, 7);
    }

    #[test]
    fn falls_back_to_text_and_names_empty_replies() {
        let out = reply(
            vec![
                ContentBlock::Text("{\"text\":".into()),
                ContentBlock::Text("\"x\"}".into()),
            ],
            StopReason::EndTurn,
        );
        assert_eq!(
            parse_output(&out, "m", 0).unwrap().content,
            "{\"text\":\"x\"}"
        );
        let cut = reply(vec![], StopReason::MaxTokens);
        let err = parse_output(&cut, "m", 0).unwrap_err();
        assert!(err.to_string().contains(OUTPUT_LIMIT_NO_ANSWER));
        let empty = reply(vec![ContentBlock::Text(" ".into())], StopReason::EndTurn);
        assert!(
            parse_output(&empty, "m", 0)
                .unwrap_err()
                .to_string()
                .contains("no message content")
        );
        let filtered = reply(
            vec![ContentBlock::Text("x".into())],
            StopReason::ContentFiltered,
        );
        assert!(matches!(
            parse_output(&filtered, "m", 0),
            Err(AiError::BadResponse(_))
        ));
    }

    #[test]
    fn documents_round_trip() {
        let value = json!({"a": [1, -2, 2.5, true, null, "s"], "b": {"c": []}});
        assert_eq!(document_to_json(&json_to_document(&value)), value);
    }

    #[tokio::test]
    async fn cannot_be_built_in_demo_mode() {
        let config = aws_config::SdkConfig::builder()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .build();
        assert!(matches!(
            Bedrock::new(ProviderPolicy::for_mode(Mode::Demo), &config),
            Err(AiError::NotAllowed { .. })
        ));
        assert!(Bedrock::new(ProviderPolicy::for_mode(Mode::Clinical), &config).is_ok());
    }
}
