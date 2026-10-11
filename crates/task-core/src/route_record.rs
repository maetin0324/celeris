//! Deterministic, database-independent routing records (ADR 2026-10-11 D1-D4).
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const TASK_FEATURES_SCHEMA: &str = "celeris.task-features/1";
pub const ROUTE_OPTIONS_SCHEMA: &str = "celeris.route-options/1";
pub const ROUTE_DECISION_SCHEMA: &str = "celeris.route-decision/1";
pub const ROUTE_OUTCOME_SCHEMA: &str = "celeris.route-outcome/1";
pub const TASK_BRIEF_TEMPLATE: &str = "task-brief/1";
pub const CANDIDATES_ONE_SHOT_BUILDER: &str = "candidates-one-shot/1";
pub const OBJECTIVE_MAX_CHARS: usize = 4_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskFeatures {
    pub schema: String,
    pub template: String,
    pub brief: String,
    pub aux: FeaturesAux,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FeaturesAux {
    pub genre: Option<String>,
    pub kind: Option<String>,
    pub harness: Option<String>,
    pub lane: Option<String>,
    pub depth: Option<u64>,
    pub phase_index: Option<u64>,
    pub deps_n: Option<u64>,
    pub acceptance_n: Option<u64>,
    pub check_command_n: Option<u64>,
    pub check_artifact_n: Option<u64>,
    pub check_reviewer_n: Option<u64>,
    pub check_human_n: Option<u64>,
    pub objective_chars: Option<usize>,
    pub objective_truncated: Option<bool>,
    pub context_paths_n: Option<usize>,
    pub crates_n: Option<usize>,
    pub repos_n: Option<usize>,
    pub prior_runs_n: Option<u64>,
    pub prior_failed_n: Option<u64>,
    pub retries_n: Option<u64>,
    pub repairs_n: Option<u64>,
    pub budget_turns: Option<u64>,
    pub budget_wall_s: Option<u64>,
    pub task_features_v69: Option<serde_json::Value>,
    pub missing: Vec<String>,
    pub truncated: Vec<Truncation>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Truncation {
    pub field: String,
    pub original_chars: usize,
}

/// Raw values needed to make D2's fixed brief and D3(a)'s auxiliary fields. Missing data stays None.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FeaturesInput {
    pub title: Option<String>,
    pub objective: Option<String>,
    pub acceptance: Option<Vec<String>>,
    pub checks: Option<Vec<CheckInput>>,
    pub done_when: Option<Vec<String>>,
    pub outputs: Option<Vec<String>>,
    pub paths: Option<Vec<String>>,
    pub repos: Option<Vec<String>>,
    pub write_set: Option<Vec<String>>,
    pub genre: Option<String>,
    pub kind: Option<String>,
    pub harness: Option<String>,
    pub lane: Option<String>,
    pub depth: Option<u64>,
    pub phase_index: Option<u64>,
    pub deps_n: Option<u64>,
    pub acceptance_n: Option<u64>,
    pub check_command_n: Option<u64>,
    pub check_artifact_n: Option<u64>,
    pub check_reviewer_n: Option<u64>,
    pub check_human_n: Option<u64>,
    pub context_paths_n: Option<usize>,
    pub crates_n: Option<usize>,
    pub repos_n: Option<usize>,
    pub prior_runs_n: Option<u64>,
    pub prior_failed_n: Option<u64>,
    pub retries_n: Option<u64>,
    pub repairs_n: Option<u64>,
    pub budget_turns: Option<u64>,
    pub budget_wall_s: Option<u64>,
    pub task_features_v69: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckInput {
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteOptions {
    pub schema: String,
    pub builder: String,
    pub question: String,
    pub options: Vec<RouteOption>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteOption {
    pub option_id: String,
    pub text: String,
    pub binds: Option<RouteOptionBinds>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteOptionBinds {
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteDecision {
    pub schema: String,
    pub decision_id: String,
    pub subject: DecisionSubject,
    pub run_id: Option<String>,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub judge: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RouteOutcome {
    pub schema: String,
    pub labels: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecisionSubject {
    pub kind: String,
    pub id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
    pub label: String,
}

pub fn build_task_features(input: &FeaturesInput) -> TaskFeatures {
    let mut aux = FeaturesAux {
        genre: input.genre.clone(),
        kind: input.kind.clone(),
        harness: input.harness.clone(),
        lane: input.lane.clone(),
        depth: input.depth,
        phase_index: input.phase_index,
        deps_n: input.deps_n,
        acceptance_n: input.acceptance_n,
        check_command_n: input.check_command_n,
        check_artifact_n: input.check_artifact_n,
        check_reviewer_n: input.check_reviewer_n,
        check_human_n: input.check_human_n,
        objective_chars: input.objective.as_deref().map(|s| s.chars().count()),
        objective_truncated: None,
        context_paths_n: input.context_paths_n,
        crates_n: input.crates_n,
        repos_n: input.repos_n,
        prior_runs_n: input.prior_runs_n,
        prior_failed_n: input.prior_failed_n,
        retries_n: input.retries_n,
        repairs_n: input.repairs_n,
        budget_turns: input.budget_turns,
        budget_wall_s: input.budget_wall_s,
        task_features_v69: input.task_features_v69.clone(),
        missing: Vec::new(),
        truncated: Vec::new(),
    };
    let title = match input.title.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(s) => bounded_secret(s, "title", 1000).0,
        None => "（不明）".into(),
    };
    let objective = match input.objective.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(s) => {
            let (v, n) = bounded_secret(s, "objective", OBJECTIVE_MAX_CHARS);
            aux.objective_truncated = Some(n.is_some());
            if let Some(n) = n {
                aux.truncated.push(n);
            }
            v
        }
        None => {
            aux.missing.push("objective".into());
            "（目的の記述なし）".into()
        }
    };
    let accepts = list(
        &mut aux,
        "acceptance",
        input.acceptance.as_deref(),
        "決定的な検査なし",
    );
    let check_text = match input.checks.as_deref() {
        Some(v) if !v.is_empty() => v
            .iter()
            .map(|c| {
                format!(
                    "- {}: {}",
                    c.kind,
                    bounded_secret(&c.description, "check", 1000).0
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(_) => "決定的な検査なし".into(),
        None => {
            aux.missing.push("checks".into());
            "決定的な検査なし".into()
        }
    };
    let done = list(&mut aux, "done_when", input.done_when.as_deref(), "なし");
    let outputs = list(&mut aux, "outputs", input.outputs.as_deref(), "なし");
    let paths = list(&mut aux, "paths", input.paths.as_deref(), "範囲の記述なし");
    let repos = list(&mut aux, "repos", input.repos.as_deref(), "範囲の記述なし");
    let writes = list(
        &mut aux,
        "write_set",
        input.write_set.as_deref(),
        "範囲の記述なし",
    );
    let position = [
        ("genre", input.genre.as_deref()),
        ("kind", input.kind.as_deref()),
        ("harness", input.harness.as_deref()),
        ("lane", input.lane.as_deref()),
    ]
    .iter()
    .map(|(k, v)| format!("- {k}: {}", v.unwrap_or("不明")))
    .collect::<Vec<_>>()
    .join("\n");
    let prior = match input.prior_runs_n {
        Some(0) => "初回".into(),
        Some(n) => format!(
            "前の run 数: {n}; failed: {}",
            input
                .prior_failed_n
                .map_or("不明".into(), |x| x.to_string())
        ),
        None => {
            aux.missing.push("prior_runs_n".into());
            "過去の試行: 不明".into()
        }
    };
    let brief = format!(
        "# 要求\nタイトル: {title}\n目的: {objective}\n\n# 期待する成果物\n受け入れ条件:\n{accepts}\nchecks:\n{check_text}\ndone_when:\n{done}\noutputs:\n{outputs}\n\n# 触る範囲\n予定 path:\n{paths}\nrepo:\n{repos}\n書き込み集合:\n{writes}\n\n# 置かれた位置\n{position}\n- depth: {}\n- phase_index: {}\n- deps_n: {}\n\n# 過去の試行\n{prior}\n\n# 性質（既存の判定）\n{}",
        input.depth.map_or("不明".into(), |n| n.to_string()),
        input.phase_index.map_or("不明".into(), |n| n.to_string()),
        input.deps_n.map_or("不明".into(), |n| n.to_string()),
        input
            .task_features_v69
            .as_ref()
            .map_or("不明".into(), |v| format!("判断の重さ: {}", v))
    );
    for (name, present) in [
        ("genre", input.genre.is_some()),
        ("kind", input.kind.is_some()),
        ("harness", input.harness.is_some()),
        ("lane", input.lane.is_some()),
        ("depth", input.depth.is_some()),
        ("phase_index", input.phase_index.is_some()),
        ("deps_n", input.deps_n.is_some()),
        ("acceptance_n", input.acceptance_n.is_some()),
        ("check_command_n", input.check_command_n.is_some()),
        ("check_artifact_n", input.check_artifact_n.is_some()),
        ("check_reviewer_n", input.check_reviewer_n.is_some()),
        ("check_human_n", input.check_human_n.is_some()),
        ("context_paths_n", input.context_paths_n.is_some()),
        ("crates_n", input.crates_n.is_some()),
        ("repos_n", input.repos_n.is_some()),
        ("prior_runs_n", input.prior_runs_n.is_some()),
        ("prior_failed_n", input.prior_failed_n.is_some()),
        ("retries_n", input.retries_n.is_some()),
        ("repairs_n", input.repairs_n.is_some()),
        ("budget_turns", input.budget_turns.is_some()),
        ("budget_wall_s", input.budget_wall_s.is_some()),
        ("task_features_v69", input.task_features_v69.is_some()),
    ] {
        if !present && !aux.missing.iter().any(|v| v == name) {
            aux.missing.push(name.into());
        }
    }
    let body = FeatureBody {
        schema: TASK_FEATURES_SCHEMA,
        template: TASK_BRIEF_TEMPLATE,
        brief: &brief,
        aux: &aux,
    };
    let hash = format!(
        "sha256:{}",
        digest(&serde_json::to_vec(&body).expect("features serialize"))
    );
    TaskFeatures {
        schema: TASK_FEATURES_SCHEMA.into(),
        template: TASK_BRIEF_TEMPLATE.into(),
        brief,
        aux,
        hash,
    }
}

fn list(aux: &mut FeaturesAux, name: &str, value: Option<&[String]>, empty: &str) -> String {
    match value {
        Some(v) if !v.is_empty() => v
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (s, n) = bounded_secret(s, &format!("{name}[{i}]"), 1000);
                if let Some(n) = n {
                    aux.truncated.push(n);
                }
                format!("- {s}")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(_) => empty.into(),
        None => {
            aux.missing.push(name.into());
            empty.into()
        }
    }
}
fn bounded_secret(s: &str, field: &str, max: usize) -> (String, Option<Truncation>) {
    let cleaned = redact_secrets(s);
    let count = cleaned.chars().count();
    if count <= max {
        return (cleaned, None);
    }
    let keep = max.saturating_sub(100) / 2;
    let start: String = cleaned.chars().take(keep).collect();
    let end: String = cleaned.chars().skip(count - keep).collect();
    let omitted = count - 2 * keep;
    let marker = format!(
        "…（{omitted} 文字省略、sha256={}）",
        digest(cleaned.as_bytes())
    );
    (
        format!("{start}{marker}{end}"),
        Some(Truncation {
            field: field.into(),
            original_chars: count,
        }),
    )
}
/// Replace common credential-shaped literals before text enters a feature record.
pub fn redact_secrets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        if let Some(n) = rest.strip_prefix("Bearer ").map(|v| {
            v.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .count()
        }) && n >= 8
        {
            out.push_str("Bearer <redacted>");
            i += 7 + n;
            continue;
        }
        let prefixes = [
            "sk-",
            "sk_live_",
            "ghp_",
            "gho_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
        ];
        if let Some(prefix) = prefixes.iter().find(|p| rest.starts_with(**p)) {
            let n = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .count();
            if n >= prefix.len() + 8 {
                out.push_str("<redacted>");
                i += n;
                continue;
            }
        }
        if rest.starts_with("api_key=")
            || rest.starts_with("token=")
            || rest.starts_with("secret=")
            || rest.starts_with("password=")
        {
            let n = rest
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(c, '&' | ';' | ',' | '}'))
                .count();
            if n > rest.find('=').unwrap_or(0) + 1 {
                let k = rest.find('=').unwrap() + 1;
                out.extend(chars[i..i + k].iter());
                out.push_str("<redacted>");
                i += n;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
#[derive(Serialize)]
struct FeatureBody<'a> {
    schema: &'static str,
    template: &'static str,
    brief: &'a str,
    aux: &'a FeaturesAux,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn candidates_one_shot(question: &str, candidates: &[Candidate]) -> RouteOptions {
    RouteOptions {
        schema: ROUTE_OPTIONS_SCHEMA.into(),
        builder: CANDIDATES_ONE_SHOT_BUILDER.into(),
        question: redact_secrets(question),
        options: candidates
            .iter()
            .map(|c| {
                let text = redact_secrets(&c.label);
                let id = digest(text.as_bytes());
                RouteOption {
                    option_id: id[..16].into(),
                    text,
                    binds: Some(RouteOptionBinds {
                        source_id: c.source_id.clone(),
                        model_id: c.model_id.clone(),
                        harness_id: c.harness_id.clone(),
                    }),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
