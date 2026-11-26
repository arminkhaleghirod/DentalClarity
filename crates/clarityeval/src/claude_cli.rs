//! The subscription route (docs/DECISIONS.md E20): Claude models through
//! Claude Code's print mode, signed in with the owner's subscription.
//!
//! Each request starts one `claude -p` process in an empty directory with
//! tools, settings sources, MCP servers and slash commands turned off, the
//! request's system prompt in place of Claude Code's own, and the JSON schema
//! passed as `--json-schema`. Claude Code still adds about 900 tokens of
//! environment context, and it offers no temperature or output-limit flags,
//! so those requests run at its defaults. The reported cost is the CLI's
//! list-price estimate; the subscription is not billed per call.

use async_trait::async_trait;
use dc_ai::{AiError, ReasoningEffort, Role, TextModel, TextRequest, TextResponse, Usage};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Semaphore;

/// Model ids on this route start with this prefix: `claude-cli/claude-opus-5-5`.
pub const PREFIX: &str = "claude-cli/";
/// Value of [`TextResponse::route`] for calls on this route.
pub const ROUTE: &str = "claude-subscription";
/// Processes running at once, across every model on this route.
pub const MAX_PROCESSES: usize = 6;
/// One call may take this long before the process is killed.
const CALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long to keep waiting out subscription usage limits, in total.
const LIMIT_PATIENCE: Duration = Duration::from_secs(6 * 60 * 60);
const LIMIT_PAUSE: Duration = Duration::from_secs(5 * 60);

pub fn is_cli_model(model: &str) -> bool {
    model.starts_with(PREFIX)
}

/// `claude-cli/claude-opus-5-5` is shown as `anthropic/claude-opus-5.5`.
pub fn display_name(model: &str) -> Option<String> {
    let name = model.strip_prefix(PREFIX)?;
    // The last hyphen between two digits is the version's decimal point.
    let bytes = name.as_bytes();
    let dot = (1..bytes.len().saturating_sub(1)).rev().find(|&i| {
        bytes[i] == b'-' && bytes[i - 1].is_ascii_digit() && bytes[i + 1].is_ascii_digit()
    });
    let mut shown = name.to_owned();
    if let Some(i) = dot {
        shown.replace_range(i..=i, ".");
    }
    Some(format!("anthropic/{shown}"))
}

pub struct ClaudeCli {
    exe: PathBuf,
    workdir: PathBuf,
    slots: Arc<Semaphore>,
}

impl ClaudeCli {
    /// `slots` is shared by every model on this route.
    pub fn new(slots: Arc<Semaphore>) -> anyhow::Result<Self> {
        let workdir = std::env::temp_dir().join(format!("clarityeval-cli-{}", std::process::id()));
        std::fs::create_dir_all(&workdir)?;
        Ok(ClaudeCli {
            exe: find_exe(),
            workdir,
            slots,
        })
    }

    pub fn shared_slots() -> Arc<Semaphore> {
        Arc::new(Semaphore::new(MAX_PROCESSES))
    }
}

/// `CLAUDE_CLI` when set; on Windows the native binary behind npm's shim,
/// because a `.cmd` shim cannot take JSON arguments safely; else `claude`.
fn find_exe() -> PathBuf {
    if let Some(p) = std::env::var_os("CLAUDE_CLI") {
        return PathBuf::from(p);
    }
    if cfg!(windows)
        && let Some(appdata) = std::env::var_os("APPDATA")
    {
        let p = PathBuf::from(appdata)
            .join("npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("claude")
}

/// The conversation after the system prompt, as one prompt. A single user
/// turn is passed as is; a repair conversation is written out with labelled
/// turns, since print mode takes one prompt.
pub fn flatten(request: &TextRequest) -> String {
    let turns: Vec<_> = request
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .collect();
    if let [only] = turns.as_slice() {
        return only.content.clone();
    }
    let last = turns.len().saturating_sub(1);
    let mut out = String::new();
    for (i, m) in turns.iter().enumerate() {
        let label = match (m.role, i == last) {
            (_, true) => "<current_request>",
            (Role::Assistant, _) => "<your_earlier_reply>",
            _ => "<earlier_request>",
        };
        let close = label.replacen('<', "</", 1);
        out.push_str(&format!("{label}\n{}\n{close}\n\n", m.content));
    }
    out.trim_end().to_owned()
}

fn effort(r: ReasoningEffort) -> &'static str {
    match r {
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

/// Reads the CLI's `--output-format json` result.
pub fn parse_output(
    stdout: &[u8],
    requested: &str,
    latency_ms: u64,
) -> dc_ai::Result<TextResponse> {
    let v: Value = serde_json::from_slice(stdout)
        .map_err(|e| AiError::Request(format!("claude cli printed no JSON result: {e}")))?;
    if v.get("is_error").and_then(Value::as_bool) == Some(true) {
        let text = v
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let status = v.get("api_error_status").and_then(Value::as_u64);
        let lower = text.to_lowercase();
        if status == Some(429) || lower.contains("limit") || lower.contains("overloaded") {
            return Err(AiError::Limited(format!("claude cli: {text}")));
        }
        return Err(AiError::Request(format!("claude cli: {text}")));
    }
    let content = match v.get("structured_output") {
        Some(s) if !s.is_null() => s.to_string(),
        _ => v
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::BadResponse("claude cli returned no result".into()))?
            .to_owned(),
    };
    let u = v.get("usage").cloned().unwrap_or(Value::Null);
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let input = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
    let model = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|m| m.keys().next().cloned())
        .unwrap_or_else(|| requested.to_owned());
    Ok(TextResponse {
        model,
        route: Some(ROUTE.into()),
        content,
        usage: Usage {
            input_tokens: u32::try_from(input).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(n("output_tokens")).unwrap_or(u32::MAX),
            cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
        },
        latency_ms,
    })
}

impl ClaudeCli {
    async fn call_once(&self, request: &TextRequest) -> dc_ai::Result<TextResponse> {
        let model = request.model.strip_prefix(PREFIX).ok_or_else(|| {
            AiError::Request(format!("{} is not a claude-cli model", request.model))
        })?;
        let system = request
            .messages
            .iter()
            .find(|m| m.role == Role::System)
            .map_or("", |m| m.content.as_str());
        let prompt_file = self.workdir.join(format!("system-{}.txt", uuid_like()));
        std::fs::write(&prompt_file, system).map_err(|e| AiError::Request(e.to_string()))?;

        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.current_dir(&self.workdir)
            .arg("-p")
            .args(["--model", model])
            .arg("--system-prompt-file")
            .arg(&prompt_file)
            .args(["--tools", ""])
            .args(["--setting-sources", ""])
            .arg("--strict-mcp-config")
            .arg("--disable-slash-commands")
            .arg("--no-session-persistence")
            .args(["--output-format", "json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(schema) = &request.response_schema {
            cmd.arg("--json-schema").arg(schema.to_string());
        }
        if let Some(r) = request.reasoning {
            cmd.args(["--effort", effort(r)]);
        }

        let started = Instant::now();
        let result = async {
            let mut child = cmd.spawn().map_err(|e| {
                AiError::Request(format!("cannot start {}: {e}", self.exe.display()))
            })?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(flatten(request).as_bytes())
                    .await
                    .map_err(|e| AiError::Request(e.to_string()))?;
            }
            match tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output()).await {
                Err(_) => Err(AiError::BadResponse(format!(
                    "no reply within {} s",
                    CALL_TIMEOUT.as_secs()
                ))),
                Ok(Err(e)) => Err(AiError::Request(e.to_string())),
                Ok(Ok(out)) => parse_output(
                    &out.stdout,
                    &request.model,
                    started.elapsed().as_millis() as u64,
                ),
            }
        }
        .await;
        let _ = std::fs::remove_file(&prompt_file);
        result
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

#[async_trait]
impl TextModel for ClaudeCli {
    fn provider(&self) -> &'static str {
        "claude-cli"
    }

    /// Usage limits are waited out here, up to [`LIMIT_PATIENCE`], so a
    /// subscription window that runs out pauses the run instead of failing it.
    async fn generate(&self, request: TextRequest) -> dc_ai::Result<TextResponse> {
        let waiting_since = Instant::now();
        loop {
            let result = {
                let _slot = self.slots.acquire().await.expect("semaphore open");
                self.call_once(&request).await
            };
            match result {
                Err(AiError::Limited(why)) if waiting_since.elapsed() < LIMIT_PATIENCE => {
                    eprintln!(
                        "claude cli limited ({why}); waiting {} s",
                        LIMIT_PAUSE.as_secs()
                    );
                    tokio::time::sleep(LIMIT_PAUSE).await;
                }
                other => return other,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_ai::Message;

    #[test]
    fn shows_the_model_name() {
        assert_eq!(
            display_name("claude-cli/claude-opus-5-5").as_deref(),
            Some("anthropic/claude-opus-5.5")
        );
        assert_eq!(
            display_name("claude-cli/claude-haiku-5-5").as_deref(),
            Some("anthropic/claude-haiku-5.5")
        );
        assert_eq!(display_name("openai/gpt-6-luna"), None);
    }

    fn req(turns: &[(Role, &str)]) -> TextRequest {
        TextRequest {
            model: "claude-cli/claude-haiku-5-5".into(),
            messages: turns
                .iter()
                .map(|(r, c)| Message {
                    role: *r,
                    content: (*c).into(),
                })
                .collect(),
            response_schema: None,
            max_tokens: 10,
            temperature: 0.0,
            reasoning: None,
        }
    }

    #[test]
    fn flattens_repair_turns() {
        assert_eq!(
            flatten(&req(&[(Role::System, "s"), (Role::User, "u")])),
            "u"
        );
        let f = flatten(&req(&[
            (Role::System, "s"),
            (Role::User, "write"),
            (Role::Assistant, "draft"),
            (Role::User, "fix it"),
        ]));
        assert!(f.starts_with("<earlier_request>\nwrite\n</earlier_request>"));
        assert!(f.contains("<your_earlier_reply>\ndraft\n</your_earlier_reply>"));
        assert!(f.ends_with("<current_request>\nfix it\n</current_request>"));
    }

    #[test]
    fn reads_results_and_limits() {
        let ok = br#"{"is_error":false,"result":"{\"a\":1}","structured_output":{"a":1},
            "total_cost_usd":0.01,"usage":{"input_tokens":2,"cache_creation_input_tokens":900,
            "cache_read_input_tokens":100,"output_tokens":50},"modelUsage":{"claude-haiku-5-5":{}}}"#;
        let r = parse_output(ok, "claude-cli/claude-haiku-5-5", 5).unwrap();
        assert_eq!(r.content, r#"{"a":1}"#);
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (1002, 50));
        assert_eq!(r.model, "claude-haiku-5-5");
        assert_eq!(r.route.as_deref(), Some(ROUTE));
        let limited =
            br#"{"is_error":true,"result":"You've hit your usage limit","api_error_status":null}"#;
        assert!(matches!(
            parse_output(limited, "m", 1),
            Err(AiError::Limited(_))
        ));
        assert!(matches!(
            parse_output(b"oops", "m", 1),
            Err(AiError::Request(_))
        ));
    }
}
