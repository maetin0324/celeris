//! DAG（`docs/gui/api.md` §3.16 / §6.2）。`depends_on` を辺にし、親子は `parent_id` で表す。

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{Status, Task, TaskId, TaskKind, TaskStore};

use crate::error::OpsError;

/// ノード数の上限。超えたら `OpsError::Validation`（`root` の指定を促す）。
pub const MAX_GRAPH_NODES: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GraphNode {
    pub id: TaskId,
    pub title: String,
    pub status: Status,
    pub kind: TaskKind,
    pub parent_id: Option<TaskId>,
    /// ADR-0016 D1 の `Task.role`（GUI-R2: DAG のノードに役割のラベルを出すため）。
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct GraphEdge {
    /// 先行タスク。
    pub from: TaskId,
    /// 後続タスク。
    pub to: TaskId,
    /// 常に `"depends_on"`。
    pub kind: String,
}

/// `depends_on` と `parent_id` を両方向にたどる隣接リストを組み立てる。
fn adjacency(all_tasks: &[Task]) -> HashMap<TaskId, Vec<TaskId>> {
    let mut adj: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for t in all_tasks {
        for dep in &t.depends_on {
            adj.entry(t.id).or_default().push(*dep);
            adj.entry(*dep).or_default().push(t.id);
        }
        if let Some(pid) = t.parent_id {
            adj.entry(t.id).or_default().push(pid);
            adj.entry(pid).or_default().push(t.id);
        }
    }
    adj
}

/// `root` から `depth` ホップ以内（`None` なら無制限）に到達できるタスク（`root` 自身を含む）。
fn reachable(all_tasks: &[Task], root: TaskId, depth: Option<u32>) -> HashSet<TaskId> {
    let adj = adjacency(all_tasks);
    let mut visited: HashSet<TaskId> = HashSet::new();
    visited.insert(root);
    let mut frontier: Vec<TaskId> = vec![root];
    let mut hops = 0u32;

    loop {
        if let Some(max_depth) = depth
            && hops >= max_depth
        {
            break;
        }
        let mut next_frontier = Vec::new();
        for id in &frontier {
            if let Some(neighbors) = adj.get(id) {
                for n in neighbors {
                    if visited.insert(*n) {
                        next_frontier.push(*n);
                    }
                }
            }
        }
        if next_frontier.is_empty() {
            break;
        }
        frontier = next_frontier;
        hops += 1;
    }

    visited
}

/// `root` を指定するとその祖先・子孫（`depends_on` と `parent_id` を両方向に `depth` ホップまで）だけ。
pub fn graph(
    store: &dyn TaskStore,
    root: Option<TaskId>,
    depth: Option<u32>,
    include_terminal: bool,
) -> Result<Graph, OpsError> {
    let all_tasks = store.list(None)?;
    let by_id: HashMap<TaskId, &Task> = all_tasks.iter().map(|t| (t.id, t)).collect();

    let included: HashSet<TaskId> = match root {
        Some(root_id) => {
            if !by_id.contains_key(&root_id) {
                return Err(OpsError::NotFound(root_id));
            }
            reachable(&all_tasks, root_id, depth)
        }
        None => all_tasks.iter().map(|t| t.id).collect(),
    };

    let mut node_ids: Vec<TaskId> = included
        .into_iter()
        .filter(|id| {
            include_terminal
                || by_id
                    .get(id)
                    .map(|t| !t.status.is_terminal())
                    .unwrap_or(false)
        })
        .collect();

    if node_ids.len() > MAX_GRAPH_NODES {
        return Err(OpsError::Validation(format!(
            "graph has {} nodes, which exceeds the {MAX_GRAPH_NODES} node limit; narrow the result with `root`",
            node_ids.len()
        )));
    }

    node_ids.sort();
    let node_set: HashSet<TaskId> = node_ids.iter().copied().collect();

    let nodes: Vec<GraphNode> = node_ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .map(|t| GraphNode {
            id: t.id,
            title: t.title.clone(),
            status: t.status,
            kind: t.kind,
            parent_id: t.parent_id,
            role: t.role.clone(),
        })
        .collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    for t in &all_tasks {
        if !node_set.contains(&t.id) {
            continue;
        }
        for dep in &t.depends_on {
            if node_set.contains(dep) {
                edges.push(GraphEdge {
                    from: *dep,
                    to: t.id,
                    kind: "depends_on".to_string(),
                });
            }
        }
    }
    edges.sort_by_key(|e| (e.from, e.to));

    Ok(Graph { nodes, edges })
}

#[cfg(test)]
mod tests;
