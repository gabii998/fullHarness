use crate::domain::{Provider, TokenUsage};

#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub custom_tools_only: bool,
    pub subscription_auth: bool,
    pub resume: bool,
    pub interrupt: bool,
    pub reported_usage: bool,
}

#[derive(Clone, Debug)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    Usage {
        request_id: String,
        model: String,
        usage: TokenUsage,
    },
    Completed,
    Failed(String),
}

/// Transport only. Tool execution belongs to our Rust runtime.
/// Legacy interface sketch. The direct async transport is in codex.rs and its
/// host-owned tool loop in runtime.rs; future providers will share that contract.
pub trait ProviderAdapter: Send {
    fn provider(&self) -> Provider;
    fn capabilities(&self) -> Capabilities;
    fn send(&mut self, input: &str) -> Result<(), String>;
    fn submit_tool_result(&mut self, call_id: &str, result: &str) -> Result<(), String>;
    fn interrupt(&mut self) -> Result<(), String>;
}
