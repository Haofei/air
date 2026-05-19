use crate::ConfigTools;
use air_runtime::{ApprovalDecision, RuntimeError, ToolProvider};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone)]
pub struct EchoTools;

impl ToolProvider for EchoTools {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        Ok(json!({
            "tool": name,
            "input": input
        }))
    }
}

#[derive(Clone)]
pub enum ToolProviderChoice {
    Echo(EchoTools),
    Config(Box<ConfigTools>),
}

impl ToolProviderChoice {
    pub fn from_config(tool_config: Option<PathBuf>, example_tools: bool) -> Result<Self> {
        if let Some(tool_config) = tool_config {
            Ok(Self::Config(Box::new(ConfigTools::from_file(tool_config)?)))
        } else if example_tools {
            Ok(Self::Config(Box::new(ConfigTools::example())))
        } else {
            Ok(Self::Echo(EchoTools))
        }
    }
}

impl ToolProvider for ToolProviderChoice {
    fn call_tool(&mut self, name: &str, input: &Value) -> Result<Value, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => provider.call_tool(name, input),
            ToolProviderChoice::Config(provider) => provider.call_tool(name, input),
        }
    }

    fn call_tool_with_timeout(
        &mut self,
        name: &str,
        input: &Value,
        timeout: Duration,
    ) -> Result<Value, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => {
                provider.call_tool_with_timeout(name, input, timeout)
            }
            ToolProviderChoice::Config(provider) => {
                provider.call_tool_with_timeout(name, input, timeout)
            }
        }
    }

    fn tool_capability(&self, name: &str) -> Option<&str> {
        match self {
            ToolProviderChoice::Echo(provider) => provider.tool_capability(name),
            ToolProviderChoice::Config(provider) => provider.tool_capability(name),
        }
    }

    fn request_approval(
        &mut self,
        module: &str,
        approval_for: &[String],
        state: &Value,
    ) -> Result<ApprovalDecision, RuntimeError> {
        match self {
            ToolProviderChoice::Echo(provider) => {
                provider.request_approval(module, approval_for, state)
            }
            ToolProviderChoice::Config(provider) => {
                provider.request_approval(module, approval_for, state)
            }
        }
    }
}
