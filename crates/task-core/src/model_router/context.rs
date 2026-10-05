use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingContext {
    pub version: String,
    pub origin: String,
    pub task_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub run_id: Option<String>,
    pub role: Option<String>,
    pub harness: Option<String>,
    pub task_kind: Option<String>,
    pub required_tools: bool,
    pub required_structured_output: bool,
    pub required_vision: bool,
    pub required_streaming: bool,
    pub input_tokens: Option<u64>,
    pub output_reserve: Option<u64>,
    pub safety_margin: u64,
    pub provenance: String,
}
