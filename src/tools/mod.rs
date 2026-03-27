pub mod bash;
pub mod file;
pub mod ripgrep_native;
pub mod web_fetch;

use crate::types::Tool;
use serde_json::Value;
use std::collections::HashMap;

pub type ToolFn = Box<dyn Fn(Value) -> Result<String, String> + Send + Sync>;

pub struct Registry {
    tools: HashMap<String, ToolFn>,
    defs: Vec<Tool>,
}

impl Registry {
    pub fn new() -> Self {
        let mut reg = Self {
            tools: HashMap::new(),
            defs: Vec::new(),
        };
        file::register(&mut reg);
        bash::register(&mut reg);
        ripgrep_native::register(&mut reg);
        web_fetch::register(&mut reg);
        reg
    }

    pub fn register_tool(&mut self, name: &str, description: &str, params: Value, func: ToolFn) {
        self.tools.insert(name.to_string(), func);
        self.defs.push(Tool {
            tool_type: "function".to_string(),
            function: crate::types::FunctionDef {
                name: name.to_string(),
                description: description.to_string(),
                parameters: params,
            },
        });
    }

    pub fn definitions(&self) -> &[Tool] {
        &self.defs
    }

    pub fn execute(&self, name: &str, args_json: &str) -> Result<String, String> {
        let func = self
            .tools
            .get(name)
            .ok_or_else(|| format!("Unknown tool: {name}"))?;
        let args: Value =
            serde_json::from_str(args_json).map_err(|e| format!("Invalid JSON args: {e}"))?;
        match func(args) {
            Ok(result) => Ok(result),
            Err(e) => Ok(format!("Error: {e}")),
        }
    }
}
