//! ADR-0056 D2: `resources/list` / `resources/read`（読むだけの客のため）。
//!
//! `celeris://knowledge/<path>`、`celeris://tasks/<id>`、`celeris://projects/<id>`、
//! `celeris://org/<node_id>`、`celeris://skills/<name>`。中身は対応する `*_get` と同じ
//! （実装は同じ道具（`tools::find`）に委譲する。スコープの判定も揃う）。
//!
//! `resources/list` は**列挙できるもの**（知識の索引・組織・skills）だけを出す。タスク・案件は
//! 件数が大きく汎用の列挙に意味が無いため、`tasks_list` / `projects_list` で id を知ってから
//! `resources/read` で読む（Phase 78 の簡略化。`docs/guides/mcp.md` に明記）。
//!
//! ADR 2026-11-10-taskstore-consumer-ports: 組織の列挙の本体（`org_resources`）は `&dyn OrgDirectory`
//! だけを受ける（試験は `tools/fake_ports.rs` の fake を渡す）。知識ベースの列挙は store を使わない。

use std::sync::Arc;

use serde::Serialize;
use task_core::{McpScope, OrgDirectory};

use crate::auth::AuthedClient;
use crate::state::{McpState, run_blocking};
use crate::tools::{self, ToolError, ToolOutput};

#[derive(Debug, Clone, Serialize)]
pub struct ResourceDescriptor {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: &'static str,
}

pub async fn list(
    state: &Arc<McpState>,
    client: &AuthedClient,
) -> Result<Vec<ResourceDescriptor>, ToolError> {
    let mut out = Vec::new();
    if client.has_scope(McpScope::KnowledgeRead)
        && let Some(root) = state.knowledge_root.clone()
    {
        let items = run_blocking(move || task_ops::knowledge::ensure_index(&root).items).await;
        for item in items {
            out.push(ResourceDescriptor {
                uri: format!("celeris://knowledge/{}", item.path),
                name: item.title,
                description: item.scope,
                mime_type: "text/markdown",
            });
        }
    }
    if client.has_scope(McpScope::OrgRead) {
        let nodes = state
            .blocking_on(&state.org_directory, org_resources)
            .await?;
        out.extend(nodes);
    }
    if client.has_scope(McpScope::SkillsRead)
        && let Some(root) = state.knowledge_root.clone()
    {
        let skills = run_blocking(move || task_ops::knowledge::skills_list(&root)).await;
        for skill in skills {
            out.push(ResourceDescriptor {
                uri: format!("celeris://skills/{}", skill.name),
                name: skill.name,
                description: Some(skill.description),
                mime_type: "text/markdown",
            });
        }
    }
    Ok(out)
}

/// 組織のノード 1 つにつき 1 件（`celeris://org/<node_id>`、名簿の並び）。引数は `McpState` の
/// `org_directory`（`Arc<dyn OrgDirectory>`）と同じ型にし、`blocking_on` へ関数のまま渡す。
pub(crate) fn org_resources(
    org: &(dyn OrgDirectory + 'static),
) -> Result<Vec<ResourceDescriptor>, ToolError> {
    Ok(org
        .org_list()
        .map_err(|e| ToolError::internal(e.to_string()))?
        .into_iter()
        .map(|node| ResourceDescriptor {
            uri: format!("celeris://org/{}", node.id),
            name: node.name,
            description: None,
            mime_type: "application/json",
        })
        .collect())
}

/// `celeris://<kind>/<id>` を `(tool 名, args)` に写す。
fn resolve(uri: &str) -> Result<(&'static str, serde_json::Value), ToolError> {
    let Some(rest) = uri.strip_prefix("celeris://") else {
        return Err(ToolError::invalid_params(format!(
            "{uri:?} is not a celeris:// resource"
        )));
    };
    let Some((kind, id)) = rest.split_once('/') else {
        return Err(ToolError::invalid_params(format!(
            "{uri:?} is missing an id"
        )));
    };
    match kind {
        "knowledge" => Ok(("knowledge_get", serde_json::json!({"path": id}))),
        "tasks" => Ok(("tasks_get", serde_json::json!({"id": id}))),
        "projects" => Ok(("projects_get", serde_json::json!({"id": id}))),
        "org" => Ok(("org_get", serde_json::json!({"node_id": id}))),
        "skills" => Ok(("skills_get", serde_json::json!({"name": id}))),
        other => Err(ToolError::invalid_params(format!(
            "unknown resource kind {other:?}"
        ))),
    }
}

pub async fn read(
    state: &Arc<McpState>,
    client: &AuthedClient,
    uri: &str,
) -> Result<ToolOutput, ToolError> {
    let (tool_name, args) = resolve(uri)?;
    let def =
        tools::find(tool_name).ok_or_else(|| ToolError::internal("unregistered resource tool"))?;
    if !client.has_scope(def.scope) {
        return Err(ToolError::not_found(format!("{uri:?} was not found")));
    }
    (def.call)(state, client, args).await
}
