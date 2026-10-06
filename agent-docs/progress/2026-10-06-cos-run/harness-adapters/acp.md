# CoS chat ACP adapter: session, permission, updates, images

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
---

- CoS run detection uses only `RunContext.cos_chat`. The legacy Secretary conversation retains its unconditional ACP permission Deny. CoS permission requests select an Allow option, including when the adapter's generic permission is Deny; the worker's existing sandbox and API authorization remain in force.
- On the first run the adapter sends `session/new`. A later run sends `session/load` only when `initialize.agentCapabilities.loadSession` confirms support. Missing support or a rejected load starts a new session in the same run, emits a single status reason, and uses saved chat history in the CoS prompt. Ordinary WU session/load behavior remains unchanged.
- ACP `agent_message_chunk`, `tool_call`, and completed/failed `tool_call_update` feed Text, ToolUse, and ToolResult. Unrecognized CoS session updates emit Status; ordinary runs retain their heartbeat-only handling.
- Negotiated `promptCapabilities.image` sends an ACP image block with base64 bytes. When only `promptCapabilities.resource` or `embeddedContext` is present, the prompt carries a resource link to the staged path. Without either capability, the adapter emits an unsupported status and sends no image block. The CoS prompt is rebuilt after initialize so its `actual` attachment route matches the negotiated response.
- The ACP client does not advertise filesystem or terminal callbacks; OpenCode uses its own read/edit/bash tools in the worker's `cwd` per ADR-0026. `mcpServers` remains the existing empty list; provider-owned MCP configuration is outside this adapter. The prompt capability record is conservative when the agent does not confirm tool support.
- Offline fake-agent tests use `crate::test_support::write_executable`. No real CLI or network is used.

Verification: `cargo test -p task-worker acp::tests --lib`; `cargo check -p task-worker --lib`; `cargo fmt --all -- --check`.
