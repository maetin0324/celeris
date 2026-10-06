//! CoS chat adapter contract. Declared harness abilities are separate from abilities confirmed
//! for a particular run; only the latter may be passed in `CosChatContext`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::CosChatDelivery;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Continuation {
    ClaudeSessionResume,
    CodexExecResume,
    AcpSessionLoad,
}

/// Adapter abilities confirmed for a run. `for_adapter` gives the contract to implement; an
/// adapter must only publish this value after confirming its CLI/protocol supports the abilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessCapabilities {
    pub continuation: Continuation,
    pub native_image_input: bool,
    pub image_read_tool: bool,
    pub shell: bool,
    pub filesystem: bool,
    pub mcp: bool,
}

impl HarnessCapabilities {
    /// The D2/D4 target contract. `acp` is OpenCode; its image/resource and tools require
    /// protocol negotiation, so their unconfirmed defaults are false.
    pub const fn for_adapter(adapter: &str) -> Option<Self> {
        match adapter.as_bytes() {
            b"claude-code" => Some(Self {
                continuation: Continuation::ClaudeSessionResume,
                native_image_input: true,
                image_read_tool: true,
                shell: true,
                filesystem: true,
                mcp: true,
            }),
            b"codex" => Some(Self {
                continuation: Continuation::CodexExecResume,
                native_image_input: true,
                image_read_tool: true,
                shell: true,
                filesystem: true,
                mcp: true,
            }),
            b"acp" => Some(Self {
                continuation: Continuation::AcpSessionLoad,
                native_image_input: false,
                image_read_tool: false,
                shell: false,
                filesystem: false,
                mcp: false,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDelivery {
    Native,
    PathAndTool,
    Unsupported,
    FilePath,
}

impl ImageDelivery {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::PathAndTool => "path+tool",
            Self::Unsupported => "unsupported",
            Self::FilePath => "path",
        }
    }
}

/// Choose the actual route from the manifest and *confirmed* run abilities. No capabilities
/// means unsupported for an image, rather than assuming a tool exists.
pub const fn image_delivery(
    manifest: CosChatDelivery,
    confirmed: Option<&HarnessCapabilities>,
) -> ImageDelivery {
    if matches!(manifest, CosChatDelivery::File) {
        return ImageDelivery::FilePath;
    }
    match confirmed {
        Some(c) if c.native_image_input => ImageDelivery::Native,
        Some(c) if c.image_read_tool => ImageDelivery::PathAndTool,
        _ => ImageDelivery::Unsupported,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingCapability {
    Continuation,
    Shell,
    Filesystem,
    Mcp,
    Image,
}

/// Reason/status text for an unmet CoS ability. Callers may prefix a run id, but must not claim
/// that a tool ran or an image was inspected on this path.
pub const fn capability_reason(missing: MissingCapability) -> &'static str {
    match missing {
        MissingCapability::Continuation => {
            "CoS session resume unavailable; starting fresh from saved chat history"
        }
        MissingCapability::Shell => "CoS shell tool unavailable; no command was run",
        MissingCapability::Filesystem => {
            "CoS filesystem tool unavailable; no file was read or changed"
        }
        MissingCapability::Mcp => "CoS MCP tool unavailable; no MCP operation was performed",
        MissingCapability::Image => "CoS image input unsupported; image was not inspected",
    }
}

pub const fn image_delivery_reason(delivery: ImageDelivery) -> Option<&'static str> {
    match delivery {
        ImageDelivery::Unsupported => Some(capability_reason(MissingCapability::Image)),
        ImageDelivery::PathAndTool => {
            Some("Image staged at path; use the confirmed image-reading tool before describing it")
        }
        ImageDelivery::Native | ImageDelivery::FilePath => None,
    }
}
