//! Native OpenAI-compatible provider harness.
//!
//! OpenRouter, Ollama, LM Studio and a custom OpenAI-compatible endpoint share
//! one model catalog, streaming path and NekoUro-owned local tool loop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt as _, stream::BoxStream};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, ToolCall, UserInputQuestion,
};

use crate::{Harness, HarnessError, RunControls};

const MAX_TOOL_ROUNDS: usize = 16;
const TOOL_OUTPUT_LIMIT: usize = 48 * 1024;
const SYSTEM_PROMPT: &str = "You are NekoUro, a local coding agent. Work directly on the user's workspace using the tools provided. Inspect before editing, keep changes focused, run verification when command execution is available, and report what changed. Never claim a tool action succeeded unless its tool result says it did.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibleProvider {
    OpenRouter,
    Ollama,
    LmStudio,
    Custom,
}

impl CompatibleProvider {
    fn id(self) -> HarnessId {
        match self {
            Self::OpenRouter => HarnessId::OpenRouter,
            Self::Ollama => HarnessId::Ollama,
            Self::LmStudio => HarnessId::LmStudio,
            Self::Custom => HarnessId::OpenAiCompatible,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::Custom => "OpenAI Compatible",
        }
    }

    fn base_url(self) -> Option<String> {
        match self {
            Self::OpenRouter => Some("https://openrouter.ai/api/v1".into()),
            Self::Ollama => Some(
                std::env::var("NEKOURO_OLLAMA_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".into())
                    .trim_end_matches('/')
                    .to_owned(),
            ),
            Self::LmStudio => Some(
                std::env::var("NEKOURO_LM_STUDIO_BASE_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:1234/v1".into())
                    .trim_end_matches('/')
                    .to_owned(),
            ),
            Self::Custom => crate::provider_config::custom_base_url(),
        }
    }

    fn requires_key(self) -> bool {
        self == Self::OpenRouter
    }
}

pub struct OpenAiCompatibleHarness {
    provider: CompatibleProvider,
    client: reqwest::Client,
    model_capabilities: std::sync::RwLock<HashMap<String, ModelCapabilities>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ModelCapabilities {
    tools: bool,
    reasoning: bool,
}

impl OpenAiCompatibleHarness {
    pub fn openrouter() -> Self { Self::new(CompatibleProvider::OpenRouter) }
    pub fn ollama() -> Self { Self::new(CompatibleProvider::Ollama) }
    pub fn lm_studio() -> Self { Self::new(CompatibleProvider::LmStudio) }
    pub fn custom() -> Self { Self::new(CompatibleProvider::Custom) }

    fn new(provider: CompatibleProvider) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(format!("NekoUro/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            provider,
            client,
            model_capabilities: std::sync::RwLock::new(HashMap::new()),
        }
    }

    fn base_url(&self) -> Result<String, HarnessError> {
        let raw = self.provider.base_url().ok_or_else(|| {
            HarnessError::NotInstalled(
                "configure an OpenAI-compatible base URL in Settings → Providers".into(),
            )
        })?;
        let parsed = reqwest::Url::parse(&raw)
            .map_err(|_| HarnessError::Protocol("provider base URL is invalid".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(HarnessError::Protocol("provider base URL must use http or https".into()));
        }
        Ok(raw.trim_end_matches('/').to_owned())
    }

    fn key(&self) -> Result<Option<String>, HarnessError> {
        let key = crate::provider_config::api_key(self.id());
        if self.provider.requires_key() && key.is_none() {
            return Err(HarnessError::NotInstalled(
                "add an OpenRouter API key in Settings → Providers".into(),
            ));
        }
        Ok(key)
    }

    fn request(&self, method: reqwest::Method, url: String) -> Result<reqwest::RequestBuilder, HarnessError> {
        let mut request = self.client.request(method, url).header("Accept", "application/json");
        if let Some(key) = self.key()? {
            request = request.bearer_auth(key);
        }
        if self.provider == CompatibleProvider::OpenRouter {
            request = request.header("X-Title", "NekoUro");
        }
        Ok(request)
    }

    async fn models_live(&self) -> Result<Vec<Model>, HarnessError> {
        let url = format!("{}/models", self.base_url()?);
        let response = send_with_retry(|| {
            self.request(reqwest::Method::GET, url.clone())
                .map(|request| request.timeout(Duration::from_secs(25)))
        }).await?;
        let status = response.status();
        let body = response.bytes().await
            .map_err(|error| HarnessError::Protocol(format!("model catalog response failed: {error}")))?;
        if !status.is_success() {
            return Err(http_error("model catalog", status, &body));
        }
        let response: ModelsResponse = serde_json::from_slice(&body)
            .map_err(|error| HarnessError::Protocol(format!("invalid model catalog: {error}")))?;
        let mut capabilities = HashMap::new();
        let mut models = Vec::new();
        for row in response.data {
            if row.id.trim().is_empty() { continue; }
            let parameters = row.supported_parameters.unwrap_or_default();
            let caps = ModelCapabilities {
                tools: self.provider != CompatibleProvider::OpenRouter
                    || parameters.iter().any(|parameter| parameter == "tools"),
                reasoning: parameters.iter().any(|parameter| parameter == "reasoning"),
            };
            capabilities.insert(row.id.clone(), caps);
            let reasoning_levels = if caps.reasoning {
                vec![
                    ReasoningLevel::Minimal,
                    ReasoningLevel::Low,
                    ReasoningLevel::Medium,
                    ReasoningLevel::High,
                    ReasoningLevel::XHigh,
                ]
            } else {
                Vec::new()
            };
            models.push(Model {
                id: row.id.clone(),
                label: row.name.filter(|value| !value.trim().is_empty()).unwrap_or(row.id),
                description: row.description.map(|value| compact_description(&value)).filter(|value| !value.is_empty()),
                reasoning_levels,
                options: Vec::new(),
            });
        }
        models.sort_by(|a, b| a.label.to_ascii_lowercase().cmp(&b.label.to_ascii_lowercase()));
        *self.model_capabilities.write().unwrap_or_else(std::sync::PoisonError::into_inner) = capabilities;
        Ok(models)
    }

    fn model_capabilities(&self, model: &str) -> ModelCapabilities {
        self.model_capabilities
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(model)
            .copied()
            .unwrap_or(ModelCapabilities {
                tools: true,
                reasoning: self.provider == CompatibleProvider::OpenRouter,
            })
    }
}

#[async_trait]
impl Harness for OpenAiCompatibleHarness {
    fn id(&self) -> HarnessId { self.provider.id() }
    fn display_name(&self) -> &str { self.provider.name() }
    fn supports_steering(&self) -> bool { false }
    fn steering_mode(&self) -> SteeringMode { SteeringMode::TurnBoundary }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        if self.provider == CompatibleProvider::OpenRouter {
            &[
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
            ]
        } else {
            &[]
        }
    }

    fn installed(&self) -> bool {
        if !crate::provider_config::initialized() { return false; }
        match self.provider {
            CompatibleProvider::OpenRouter => crate::provider_config::has_api_key(self.id()),
            CompatibleProvider::Custom => crate::provider_config::custom_base_url().is_some(),
            CompatibleProvider::Ollama | CompatibleProvider::LmStudio => self
                .provider
                .base_url()
                .as_deref()
                .is_some_and(local_endpoint_available),
        }
    }

    fn deterministic_turn_end(&self) -> bool { true }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.models_live().await
    }

    async fn skills(&self, _cwd: &Path) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        Ok(None)
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let model = request.model.clone()
            .filter(|model| !model.trim().is_empty())
            .ok_or_else(|| HarnessError::Protocol("select a model before starting this provider".into()))?;
        let base_url = self.base_url()?;
        let key = self.key()?;
        let provider = self.provider;
        let caps = self.model_capabilities(&model);
        let client = self.client.clone();
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            run_session(provider, client, base_url, key, caps, model, request, controls, tx).await;
        });
        Ok(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        }).boxed())
    }
}

fn local_endpoint_available(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else { return false };
    let Some(host) = url.host_str() else { return false };
    let local = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1";
    if !local {
        // Explicit remote Ollama/LM Studio endpoints are configuration, not
        // local process detection; let model discovery report reachability.
        return true;
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let ip = if host == "::1" {
        std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    };
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::new(ip, port),
        Duration::from_millis(75),
    )
    .is_ok()
}

#[derive(Deserialize)]
struct ModelsResponse { data: Vec<ModelRow> }

#[derive(Deserialize)]
struct ModelRow {
    id: String,
    name: Option<String>,
    description: Option<String>,
    supported_parameters: Option<Vec<String>>,
}

fn compact_description(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(180).collect()
}

#[derive(Default, Clone)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct Completion {
    content: String,
    reasoning: String,
    tool_calls: Vec<PendingToolCall>,
}

async fn run_session(
    provider: CompatibleProvider,
    client: reqwest::Client,
    base_url: String,
    key: Option<String>,
    caps: ModelCapabilities,
    model: String,
    request: RunRequest,
    mut controls: RunControls,
    tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
) {
    let _execution_lease = controls.execution_lease.take();
    let session_id = request.resume.clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut assistant_message_id = uuid::Uuid::new_v4().to_string();
    let mut messages = request.resume.as_deref()
        .and_then(|id| crate::provider_config::session_load(provider.id(), id))
        .unwrap_or_else(|| vec![json!({"role":"system","content":SYSTEM_PROMPT})]);
    messages.push(json!({"role":"user","content":request.prompt}));

    let tool_names = if caps.tools {
        ["read_file","write_file","edit_file","search","list_files","git_status","git_diff","run_command"]
            .into_iter().map(str::to_owned).collect()
    } else {
        Vec::new()
    };
    if send_event(&tx, AgentEvent::SessionStarted {
        harness: provider.id(),
        model: model.clone(),
        tools: tool_names,
        cwd: request.cwd.clone(),
        session_id: session_id.clone(),
        assistant_message_id: assistant_message_id.clone(),
    }).await.is_err() {
        return;
    }

    let mut last_text = String::new();
    for _round in 0..MAX_TOOL_ROUNDS {
        if controls.interrupt.is_cancelled() {
            let _ = send_done(&tx, DoneStatus::Interrupted, None, None, Some(session_id)).await;
            return;
        }
        let completion = match stream_completion(
            provider, &client, &base_url, key.as_deref(), &model, &messages, caps,
            request.reasoning, &controls.interrupt, &tx,
        ).await {
            Ok(value) => value,
            Err(error) => {
                let interrupted = controls.interrupt.is_cancelled();
                let message = error.to_string();
                if !interrupted {
                    let _ = send_event(&tx, AgentEvent::Error { message: message.clone() }).await;
                }
                let _ = send_done(
                    &tx,
                    if interrupted { DoneStatus::Interrupted } else { DoneStatus::Errored },
                    None,
                    (!interrupted).then_some(message),
                    Some(session_id),
                ).await;
                return;
            }
        };

        last_text.push_str(&completion.content);
        let tool_calls_json: Vec<Value> = completion.tool_calls.iter().map(|call| json!({
            "id": call.id,
            "type": "function",
            "function": {"name": call.name, "arguments": call.arguments}
        })).collect();
        let mut assistant = json!({"role":"assistant","content":completion.content});
        if !tool_calls_json.is_empty() {
            assistant["tool_calls"] = Value::Array(tool_calls_json);
        }
        if !completion.reasoning.is_empty() {
            assistant["reasoning"] = Value::String(completion.reasoning);
        }
        messages.push(assistant);

        if completion.tool_calls.is_empty() {
            let _ = crate::provider_config::session_save(provider.id(), &session_id, &messages);
            let completed_id = std::mem::replace(
                &mut assistant_message_id,
                uuid::Uuid::new_v4().to_string(),
            );
            let _ = send_event(&tx, AgentEvent::AssistantMessageCompleted {
                assistant_message_id: completed_id,
            }).await;
            let _ = send_done(&tx, DoneStatus::Completed, Some(last_text), None, Some(session_id)).await;
            return;
        }

        for call in completion.tool_calls {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
            if send_event(&tx, AgentEvent::ToolCall {
                id: call.id.clone(),
                call: normalized_tool_call(&call.name, &args),
            }).await.is_err() {
                return;
            }
            let (is_error, output) = match execute_tool(
                &call, &args, &request.cwd, request.sandbox, request.auto_approve, &controls,
            ).await {
                Ok(output) => (false, output),
                Err(error) => (true, error),
            };
            let output = truncate_output(output);
            let _ = send_event(&tx, AgentEvent::ToolResult {
                id: call.id.clone(),
                is_error,
                output: Some(output.clone()),
                diff: None,
            }).await;
            messages.push(json!({"role":"tool","tool_call_id":call.id,"content":output}));
        }
        let _ = crate::provider_config::session_save(provider.id(), &session_id, &messages);
        let completed_id = std::mem::replace(
            &mut assistant_message_id,
            uuid::Uuid::new_v4().to_string(),
        );
        let _ = send_event(&tx, AgentEvent::AssistantMessageCompleted {
            assistant_message_id: completed_id,
        }).await;
    }

    let message = format!("native provider stopped after {MAX_TOOL_ROUNDS} tool rounds");
    let _ = send_event(&tx, AgentEvent::Error { message: message.clone() }).await;
    let _ = send_done(&tx, DoneStatus::Errored, None, Some(message), Some(session_id)).await;
}

async fn stream_completion(
    provider: CompatibleProvider,
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
    model: &str,
    messages: &[Value],
    caps: ModelCapabilities,
    reasoning: Option<ReasoningLevel>,
    interrupt: &tokio_util::sync::CancellationToken,
    tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
) -> Result<Completion, HarnessError> {
    let mut body = json!({"model":model,"messages":messages,"stream":true});
    if caps.tools {
        body["tools"] = tool_definitions();
        body["tool_choice"] = Value::String("auto".into());
    }
    if provider == CompatibleProvider::OpenRouter {
        body["include_reasoning"] = Value::Bool(true);
        if caps.reasoning && let Some(level) = reasoning {
            body["reasoning"] = json!({"effort": reasoning_name(level)});
        }
    }

    let url = format!("{base_url}/chat/completions");
    let response = tokio::select! {
        _ = interrupt.cancelled() => {
            return Err(HarnessError::Protocol("interrupted".into()));
        }
        response = send_with_retry(|| {
            let mut request = client.post(&url).header("Accept", "text/event-stream").json(&body);
            if let Some(key) = key { request = request.bearer_auth(key); }
            if provider == CompatibleProvider::OpenRouter { request = request.header("X-Title", "NekoUro"); }
            Ok(request)
        }) => response?,
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.bytes().await.unwrap_or_default();
        return Err(http_error("chat completion", status, &body));
    }

    let mut chunks = response.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    let mut content = String::new();
    let mut reasoning_text = String::new();
    let mut calls: Vec<PendingToolCall> = Vec::new();
    let mut done = false;
    while !done {
        let next = tokio::select! {
            _ = interrupt.cancelled() => return Err(HarnessError::Protocol("interrupted".into())),
            item = chunks.next() => item
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|error| HarnessError::Protocol(format!("stream read failed: {error}")))?;
        buffer.extend_from_slice(&chunk);
        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = buffer.drain(..=newline).collect::<Vec<_>>();
            while line.last().is_some_and(|byte| *byte == b'\n' || *byte == b'\r') {
                line.pop();
            }
            let Some(mut line) = line.strip_prefix(b"data:") else { continue };
            while line.first() == Some(&b' ') { line = &line[1..]; }
            if line == b"[DONE]" {
                done = true;
                break;
            }
            if line.is_empty() { continue; }
            let event: Value = serde_json::from_slice(line)
                .map_err(|error| HarnessError::Protocol(format!("invalid streaming event: {error}")))?;
            if let Some(usage) = event.get("usage") {
                let input = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
                let output = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
                if input != 0 || output != 0 {
                    let _ = send_event(tx, AgentEvent::Usage { input_tokens: input, output_tokens: output }).await;
                }
            }
            let Some(delta) = event.get("choices").and_then(Value::as_array)
                .and_then(|choices| choices.first()).and_then(|choice| choice.get("delta"))
            else { continue };
            if let Some(text) = delta.get("content").and_then(Value::as_str)
                && !text.is_empty()
            {
                content.push_str(text);
                send_event(tx, AgentEvent::TextDelta { text: text.to_owned() }).await?;
            }
            if let Some(text) = delta.get("reasoning").and_then(Value::as_str)
                && !text.is_empty()
            {
                reasoning_text.push_str(text);
                send_event(tx, AgentEvent::ReasoningDelta { text: text.to_owned() }).await?;
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for value in tool_calls {
                    let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    while calls.len() <= index { calls.push(PendingToolCall::default()); }
                    let call = &mut calls[index];
                    if let Some(id) = value.get("id").and_then(Value::as_str) { call.id.push_str(id); }
                    if let Some(function) = value.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str) { call.name.push_str(name); }
                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) { call.arguments.push_str(arguments); }
                    }
                }
            }
        }
    }
    for call in &mut calls {
        if call.id.is_empty() { call.id = format!("call_{}", uuid::Uuid::new_v4()); }
    }
    calls.retain(|call| !call.name.is_empty());
    Ok(Completion { content, reasoning: reasoning_text, tool_calls: calls })
}

async fn send_with_retry(
    mut build: impl FnMut() -> Result<reqwest::RequestBuilder, HarnessError>,
) -> Result<reqwest::Response, HarnessError> {
    let waits = [0_u64, 350, 900, 1800];
    let mut last = None;
    for (attempt, wait) in waits.into_iter().enumerate() {
        if wait != 0 { tokio::time::sleep(Duration::from_millis(wait)).await; }
        match build()?.send().await {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) if matches!(response.status().as_u16(), 408 | 409 | 429 | 500 | 502 | 503 | 504)
                && attempt + 1 < waits.len() =>
            {
                last = Some(format!("HTTP {}", response.status()));
            }
            Ok(response) => return Ok(response),
            Err(error) if attempt + 1 < waits.len() => last = Some(error.to_string()),
            Err(error) => return Err(HarnessError::Protocol(format!("provider request failed: {error}"))),
        }
    }
    Err(HarnessError::Protocol(format!(
        "provider request failed after retries: {}",
        last.unwrap_or_else(|| "unknown error".into())
    )))
}

fn http_error(context: &str, status: reqwest::StatusCode, body: &[u8]) -> HarnessError {
    let compact = String::from_utf8_lossy(body).split_whitespace().collect::<Vec<_>>().join(" ");
    let compact: String = compact.chars().take(600).collect();
    HarnessError::Protocol(format!("{context} failed ({status}): {compact}"))
}

fn reasoning_name(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Minimal => "minimal",
        ReasoningLevel::Low => "low",
        ReasoningLevel::Medium => "medium",
        ReasoningLevel::High => "high",
        ReasoningLevel::XHigh | ReasoningLevel::Max | ReasoningLevel::Ultra
        | ReasoningLevel::Ultracode | ReasoningLevel::Ultrathink => "xhigh",
    }
}

fn tool_definitions() -> Value {
    json!([
        {"type":"function","function":{"name":"read_file","description":"Read a UTF-8 text file from the workspace.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"write_file","description":"Write a complete UTF-8 file inside the workspace unless danger-full-access is active.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"edit_file","description":"Replace one exact string in a UTF-8 file.","parameters":{"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"}},"required":["path","old_string","new_string"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"search","description":"Search workspace text files for a literal string.","parameters":{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"list_files","description":"List files and directories under one workspace directory.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}}},
        {"type":"function","function":{"name":"git_status","description":"Run git status --short in the workspace.","parameters":{"type":"object","properties":{},"additionalProperties":false}}},
        {"type":"function","function":{"name":"git_diff","description":"Read the current git diff. Optional staged=true reads --cached.","parameters":{"type":"object","properties":{"staged":{"type":"boolean"}},"additionalProperties":false}}},
        {"type":"function","function":{"name":"run_command","description":"Run a shell command in the workspace. Requires danger-full-access and approval unless auto-approve is enabled.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}}
    ])
}

fn normalized_tool_call(name: &str, args: &Value) -> ToolCall {
    let string = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
    match name {
        "read_file" => ToolCall::ReadFile { path: string("path").unwrap_or_default() },
        "write_file" => ToolCall::WriteFile { path: string("path").unwrap_or_default(), content: None },
        "edit_file" => ToolCall::EditFile {
            path: string("path").unwrap_or_default(),
            old_string: string("old_string"),
            new_string: string("new_string"),
        },
        "search" => ToolCall::Search { pattern: string("pattern").unwrap_or_default(), path: string("path") },
        "list_files" => ToolCall::Glob { pattern: string("path").unwrap_or_else(|| ".".into()) },
        "git_status" => ToolCall::Exec { command: "git status --short".into() },
        "git_diff" => ToolCall::Exec {
            command: if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                "git diff --cached".into()
            } else {
                "git diff".into()
            },
        },
        "run_command" => ToolCall::Exec { command: string("command").unwrap_or_default() },
        other => ToolCall::Unknown { name: other.to_owned(), input: Some(args.clone()) },
    }
}

async fn execute_tool(
    call: &PendingToolCall,
    args: &Value,
    cwd: &str,
    sandbox: SandboxLevel,
    auto_approve: bool,
    controls: &RunControls,
) -> Result<String, String> {
    let cwd = PathBuf::from(cwd);
    match call.name.as_str() {
        "read_file" => {
            let path = workspace_path(&cwd, required_string(args, "path")?, sandbox, true)?;
            tokio::fs::read_to_string(&path).await
                .map_err(|error| format!("read {}: {error}", path.display()))
        }
        "write_file" => {
            if sandbox == SandboxLevel::ReadOnly { return Err("write_file is blocked by the read-only sandbox".into()); }
            approve_mutation(call, auto_approve, controls).await?;
            let path = workspace_path(&cwd, required_string(args, "path")?, sandbox, false)?;
            tokio::fs::write(&path, required_string(args, "content")?).await
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(format!("wrote {}", path.display()))
        }
        "edit_file" => {
            if sandbox == SandboxLevel::ReadOnly { return Err("edit_file is blocked by the read-only sandbox".into()); }
            approve_mutation(call, auto_approve, controls).await?;
            let path = workspace_path(&cwd, required_string(args, "path")?, sandbox, true)?;
            let old = required_string(args, "old_string")?;
            let new = required_string(args, "new_string")?;
            let text = tokio::fs::read_to_string(&path).await
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            let count = text.matches(old).count();
            if count != 1 { return Err(format!("edit_file expected exactly one match, found {count}")); }
            tokio::fs::write(&path, text.replacen(old, new, 1)).await
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            Ok(format!("edited {}", path.display()))
        }
        "list_files" => {
            let path = workspace_path(&cwd, args.get("path").and_then(Value::as_str).unwrap_or("."), sandbox, true)?;
            let mut entries = tokio::fs::read_dir(&path).await
                .map_err(|error| format!("list {}: {error}", path.display()))?;
            let mut names = Vec::new();
            while let Some(entry) = entries.next_entry().await.map_err(|error| error.to_string())? {
                let kind = entry.file_type().await.map_err(|error| error.to_string())?;
                names.push(format!("{}{}", entry.file_name().to_string_lossy(), if kind.is_dir() { "/" } else { "" }));
                if names.len() >= 500 { names.push("… truncated …".into()); break; }
            }
            names.sort();
            Ok(names.join("\n"))
        }
        "search" => {
            let pattern = required_string(args, "pattern")?.to_owned();
            let start = workspace_path(&cwd, args.get("path").and_then(Value::as_str).unwrap_or("."), sandbox, true)?;
            tokio::task::spawn_blocking(move || search_files(&start, &pattern)).await
                .map_err(|error| format!("search task failed: {error}"))?
        }
        "git_status" => run_readonly_command(&cwd, &["status", "--short"], &controls.interrupt).await,
        "git_diff" => {
            if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                run_readonly_command(&cwd, &["diff", "--cached"], &controls.interrupt).await
            } else {
                run_readonly_command(&cwd, &["diff"], &controls.interrupt).await
            }
        }
        "run_command" => {
            if sandbox != SandboxLevel::DangerFullAccess {
                return Err("run_command requires danger-full-access because NekoUro cannot OS-sandbox an arbitrary shell command portably".into());
            }
            approve_mutation(call, auto_approve, controls).await?;
            run_shell_command(&cwd, required_string(args, "command")?, &controls.interrupt).await
        }
        other => Err(format!("unknown tool {other:?}")),
    }
}

async fn approve_mutation(
    call: &PendingToolCall,
    auto_approve: bool,
    controls: &RunControls,
) -> Result<(), String> {
    if auto_approve { return Ok(()); }
    let question_id = format!("native-tool-{}", call.id);
    let rx = (controls.request_input)(vec![UserInputQuestion {
        id: question_id.clone(),
        header: "Approve tool".into(),
        question: format!("Allow NekoUro to run {}?", call.name),
        options: vec!["Allow".into(), "Deny".into()],
        multi_select: false,
        prefill: None,
        multiline: false,
    }]);
    let answers = rx.await.map_err(|_| "tool approval was cancelled".to_string())?;
    if answers.iter().any(|answer| {
        answer.question_id == question_id
            && answer.labels.iter().any(|label| label.eq_ignore_ascii_case("allow"))
    }) {
        Ok(())
    } else {
        Err("tool call denied by user".into())
    }
}

fn required_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn workspace_path(cwd: &Path, raw: &str, sandbox: SandboxLevel, must_exist: bool) -> Result<PathBuf, String> {
    let root = cwd.canonicalize().map_err(|error| format!("workspace {}: {error}", cwd.display()))?;
    let raw = Path::new(raw);
    let joined = if raw.is_absolute() { raw.to_path_buf() } else { root.join(raw) };
    if sandbox == SandboxLevel::DangerFullAccess {
        return if must_exist {
            joined.canonicalize().map_err(|error| format!("{}: {error}", joined.display()))
        } else {
            Ok(joined)
        };
    }
    let checked = if must_exist {
        joined.canonicalize().map_err(|error| format!("{}: {error}", joined.display()))?
    } else {
        let parent = joined.parent().ok_or_else(|| "path has no parent".to_string())?;
        let parent = parent.canonicalize().map_err(|error| format!("{}: {error}", parent.display()))?;
        parent.join(joined.file_name().ok_or_else(|| "path has no file name".to_string())?)
    };
    if !checked.starts_with(&root) { return Err("path escapes the workspace sandbox".into()); }
    Ok(checked)
}

fn search_files(root: &Path, pattern: &str) -> Result<String, String> {
    let mut stack = vec![root.to_path_buf()];
    let mut out = Vec::new();
    let mut visited = 0usize;
    while let Some(path) = stack.pop() {
        let metadata = match std::fs::metadata(&path) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if matches!(name, ".git" | "node_modules" | "target" | ".next" | "dist") && path != root { continue; }
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() { stack.push(entry.path()); }
            }
            continue;
        }
        visited += 1;
        if visited > 4000 { out.push("… search truncated after 4000 files …".into()); break; }
        if metadata.len() > 2 * 1024 * 1024 { continue; }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        for (line, value) in text.lines().enumerate() {
            if value.contains(pattern) {
                out.push(format!("{}:{}: {}", path.display(), line + 1, value.trim()));
                if out.len() >= 500 {
                    out.push("… results truncated …".into());
                    return Ok(out.join("\n"));
                }
            }
        }
    }
    Ok(out.join("\n"))
}

async fn run_readonly_command(
    cwd: &Path,
    args: &[&str],
    interrupt: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let mut command = tokio::process::Command::new("git");
    command.args(args).current_dir(cwd).kill_on_drop(true);
    run_output(command, interrupt).await
}

async fn run_shell_command(
    cwd: &Path,
    script: &str,
    interrupt: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", script]);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.args(["-lc", script]);
        command
    };
    command.current_dir(cwd).kill_on_drop(true);
    run_output(command, interrupt).await
}

async fn run_output(
    mut command: tokio::process::Command,
    interrupt: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let output = tokio::select! {
        _ = interrupt.cancelled() => return Err("command interrupted".into()),
        result = command.output() => result.map_err(|error| error.to_string())?,
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() { text.push('\n'); }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if output.status.success() { Ok(text) } else { Err(format!("command exited with {}\n{}", output.status, text)) }
}

fn truncate_output(mut output: String) -> String {
    if output.len() <= TOOL_OUTPUT_LIMIT { return output; }
    let mut cut = TOOL_OUTPUT_LIMIT;
    while !output.is_char_boundary(cut) { cut -= 1; }
    output.truncate(cut);
    output.push_str("\n… output truncated by NekoUro …");
    output
}

async fn send_event(
    tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    event: AgentEvent,
) -> Result<(), HarnessError> {
    tx.send(Ok(event)).await.map_err(|_| HarnessError::Protocol("event consumer closed".into()))
}

async fn send_done(
    tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    status: DoneStatus,
    result: Option<String>,
    error: Option<String>,
    session_id: Option<String>,
) -> Result<(), HarnessError> {
    send_event(tx, AgentEvent::Done { status, result, error, session_id }).await
}
