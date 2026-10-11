//! Deterministic, database-independent routing records (ADR 2026-10-11 D1-D4).
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::execution_plan::WorkUnitCheck;
use crate::model::{Check, Criterion};
use crate::model_policy::{Level, TaskFeatures as LaneFeatures};

pub const TASK_FEATURES_SCHEMA: &str = "celeris.task-features/1";
pub const ROUTE_OPTIONS_SCHEMA: &str = "celeris.route-options/1";
pub const ROUTE_DECISION_SCHEMA: &str = "celeris.route-decision/1";
pub const ROUTE_OUTCOME_SCHEMA: &str = "celeris.route-outcome/1";
pub const TASK_BRIEF_TEMPLATE: &str = "task-brief/2";
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
    /// D2「触る範囲」の書き込み集合の予測: 前の試行の `run_write_sets`（WU は
    /// `work_unit_write_sets` も）の path を `<repo>:<path>` で正規化・重複除去・整列したもの。
    pub write_set: Option<Vec<String>>,
    /// 書き込み集合の元の行のうち status が `incomplete`／`unavailable` のもの。
    pub write_set_gaps: Vec<WriteSetGap>,
    pub prior_runs_n: Option<u64>,
    pub prior_failed_n: Option<u64>,
    pub retries_n: Option<u64>,
    pub repairs_n: Option<u64>,
    pub budget_turns: Option<u64>,
    pub budget_wall_s: Option<u64>,
    pub task_features_v69: Option<LaneFeatures>,
    pub missing: Vec<String>,
    pub truncated: Vec<Truncation>,
}

/// 書き込み集合の不完全な元の行 1 件（`source` は `run_write_sets`／`work_unit_write_sets`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct WriteSetGap {
    pub source: String,
    pub owner_id: String,
    pub repo: String,
    pub status: String,
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
    pub acceptance: Option<Vec<AcceptanceInput>>,
    pub checks: Option<Vec<CheckInput>>,
    pub done_when: Option<Vec<String>>,
    pub outputs: Option<Vec<String>>,
    pub paths: Option<Vec<String>>,
    pub repos: Option<Vec<String>>,
    pub write_set: Option<Vec<String>>,
    pub write_set_gaps: Vec<WriteSetGap>,
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
    /// D2「過去の試行」: 前の run ごとの要約（古い順）。
    pub prior_runs: Option<Vec<PriorRunInput>>,
    pub retries_n: Option<u64>,
    pub repairs_n: Option<u64>,
    pub budget_turns: Option<u64>,
    pub budget_wall_s: Option<u64>,
    pub task_features_v69: Option<LaneFeatures>,
}

/// D2「期待する成果物」: 受け入れ条件 1 件（`Criterion` の文と check の種類）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceInput {
    pub text: String,
    pub check: CheckInput,
}

impl From<&Criterion> for AcceptanceInput {
    fn from(c: &Criterion) -> Self {
        Self {
            text: c.text.clone(),
            check: CheckInput::from(&c.check),
        }
    }
}

/// `Criterion.check` の種類、または WU の `WorkUnitCheck`（常に command）。
/// command の `cmd` は元の文字列を持ち、brief には実行ファイル名と subcommand だけを出す。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckInput {
    Command { cmd: String },
    ArtifactExists,
    KnowledgePage,
    Reviewer,
    Human,
}

impl CheckInput {
    fn render(&self) -> String {
        match self {
            CheckInput::Command { cmd } => format!("command `{}`", command_name(cmd)),
            CheckInput::ArtifactExists => "artifact_exists".into(),
            CheckInput::KnowledgePage => "knowledge_page".into(),
            CheckInput::Reviewer => "reviewer".into(),
            CheckInput::Human => "human".into(),
        }
    }
}

impl From<&Check> for CheckInput {
    fn from(c: &Check) -> Self {
        match c {
            Check::Command { cmd, .. } => CheckInput::Command { cmd: cmd.clone() },
            Check::ArtifactExists { .. } => CheckInput::ArtifactExists,
            Check::KnowledgePage { .. } => CheckInput::KnowledgePage,
            Check::Reviewer => CheckInput::Reviewer,
            Check::Human => CheckInput::Human,
        }
    }
}

impl From<&WorkUnitCheck> for CheckInput {
    fn from(c: &WorkUnitCheck) -> Self {
        CheckInput::Command { cmd: c.cmd.clone() }
    }
}

/// D2「過去の試行」の 1 run。`failed_criteria` は落ちた検査の criterion 文と exit code
/// （review の stdout は入れない）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PriorRunInput {
    pub status: String,
    pub model: Option<String>,
    pub wall_s: Option<u64>,
    pub failed_criteria: Vec<FailedCriterionInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailedCriterionInput {
    pub text: String,
    pub exit: Option<i32>,
}

/// 実行ファイル名（basename）と subcommand まで。先頭の `VAR=value`、option、path 形の引数は落とす。
fn command_name(cmd: &str) -> String {
    let mut words = cmd
        .split_whitespace()
        .skip_while(|w| w.contains('=') && !w.starts_with('-'));
    let Some(exe) = words.next() else {
        return String::new();
    };
    let exe = exe.rsplit('/').next().unwrap_or(exe);
    match words.next() {
        Some(sub)
            if !sub.starts_with('-')
                && !sub.contains(['/', '=', '.', '$', '"', '\''])
                && !sub.is_empty() =>
        {
            format!("{exe} {sub}")
        }
        _ => exe.to_owned(),
    }
}

/// ADR-0069 の 9 軸の日本語名（brief「性質」節の行の順）。
fn lane_feature_lines(f: &LaneFeatures) -> String {
    [
        ("判断の重さ", f.judgment),
        ("要求の曖昧さ", f.ambiguity),
        ("決定的な検証のしやすさ", f.verifiability),
        ("やり直しやすさ", f.reversibility),
        ("失敗したときの損失", f.consequence),
        ("読むべき文脈の大きさ", f.context_size),
        ("道具を使う度合い", f.tool_intensity),
        ("想定される長さ", f.expected_length),
        ("領域をまたぐ度合い", f.cross_cutting),
    ]
    .iter()
    .map(|(name, level)| {
        let level = match level {
            Level::Low => "低",
            Level::Medium => "中",
            Level::High => "高",
        };
        format!("- {name}: {level}")
    })
    .collect::<Vec<_>>()
    .join("\n")
}

fn opt_num<T: ToString>(v: Option<T>) -> String {
    v.map_or("不明".into(), |n| n.to_string())
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
        write_set: input.write_set.clone(),
        write_set_gaps: input.write_set_gaps.clone(),
        prior_runs_n: input.prior_runs_n,
        prior_failed_n: input.prior_failed_n,
        retries_n: input.retries_n,
        repairs_n: input.repairs_n,
        budget_turns: input.budget_turns,
        budget_wall_s: input.budget_wall_s,
        task_features_v69: input.task_features_v69,
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
    let accepts = match input.acceptance.as_deref() {
        Some(v) if !v.is_empty() => v
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let text = bounded(&mut aux, &a.text, &format!("acceptance[{i}]"));
                format!("- {text}（検査: {}）", bounded_plain(&a.check.render()))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(_) => "決定的な検査なし".into(),
        None => {
            aux.missing.push("acceptance".into());
            "決定的な検査なし".into()
        }
    };
    let check_text = match input.checks.as_deref() {
        Some(v) if !v.is_empty() => v
            .iter()
            .map(|c| format!("- {}", bounded_plain(&c.render())))
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
    let prior = prior_section(&mut aux, input);
    let nature = input.task_features_v69.as_ref().map_or(String::new(), |f| {
        format!("\n\n# 性質（既存の判定）\n{}", lane_feature_lines(f))
    });
    let brief = format!(
        "# 要求\nタイトル: {title}\n目的: {objective}\n\n# 期待する成果物\n受け入れ条件:\n{accepts}\nchecks:\n{check_text}\ndone_when:\n{done}\noutputs:\n{outputs}\n\n# 触る範囲\n予定 path:\n{paths}\nrepo:\n{repos}\n書き込み集合:\n{writes}\n\n# 置かれた位置\n{position}\n- depth: {}\n- phase_index: {}\n- deps_n: {}\n\n# 過去の試行\n{prior}{nature}",
        opt_num(input.depth),
        opt_num(input.phase_index),
        opt_num(input.deps_n),
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

/// D2「過去の試行」: 数の行、run ごとの 1 行（古い順）、repair 数の行。前の run が無ければ「初回」。
fn prior_section(aux: &mut FeaturesAux, input: &FeaturesInput) -> String {
    let runs = input.prior_runs.as_deref();
    let count = input.prior_runs_n.or_else(|| runs.map(|r| r.len() as u64));
    let mut lines = vec![match count {
        Some(0) => "初回".to_owned(),
        Some(n) => format!(
            "前の run: {n} 回（うち失敗 {}）",
            opt_num(input.prior_failed_n)
        ),
        None => "前の run: 不明".to_owned(),
    }];
    match runs {
        Some(runs) => {
            for (i, run) in runs.iter().enumerate() {
                let failed = if run.failed_criteria.is_empty() {
                    "なし".to_owned()
                } else {
                    run.failed_criteria
                        .iter()
                        .enumerate()
                        .map(|(j, c)| {
                            let text =
                                bounded(aux, &c.text, &format!("prior_runs[{i}].failed[{j}]"));
                            format!("「{text}」(exit {})", opt_num(c.exit))
                        })
                        .collect::<Vec<_>>()
                        .join("、")
                };
                lines.push(format!(
                    "- run {}: {}、model: {}、壁時計: {}、落ちた検査: {failed}",
                    i + 1,
                    bounded_plain(&run.status),
                    run.model.as_deref().map_or("不明".into(), bounded_plain),
                    run.wall_s.map_or("不明".into(), |s| format!("{s} 秒")),
                ));
            }
        }
        None => aux.missing.push("prior_runs".into()),
    }
    lines.push(
        input
            .repairs_n
            .map_or("repair: 不明".into(), |n| format!("repair: {n} 件")),
    );
    lines.join("\n")
}

fn list(aux: &mut FeaturesAux, name: &str, value: Option<&[String]>, empty: &str) -> String {
    match value {
        Some(v) if !v.is_empty() => v
            .iter()
            .enumerate()
            .map(|(i, s)| format!("- {}", bounded(aux, s, &format!("{name}[{i}]"))))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(_) => empty.into(),
        None => {
            aux.missing.push(name.into());
            empty.into()
        }
    }
}
/// 1,000 字上限の自由文。切ったら `aux.truncated` に残す。
fn bounded(aux: &mut FeaturesAux, s: &str, field: &str) -> String {
    let (s, n) = bounded_secret(s, field, 1000);
    if let Some(n) = n {
        aux.truncated.push(n);
    }
    s
}
/// 短い識別子（check の種類・command 名・status・model）。切断は記録しない。
fn bounded_plain(s: &str) -> String {
    bounded_secret(s, "", 1000).0
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

const TOKEN_PREFIXES: [&str; 7] = [
    "sk-",
    "sk_live_",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
];
const KEY_PREFIXES: [&str; 4] = ["api_key=", "token=", "secret=", "password="];

/// Replace common credential-shaped literals before text enters a feature record.
/// One pass over the bytes: every pattern starts with ASCII, so a match can only begin on a
/// char boundary, and unmatched spans are copied as slices of `text` (no per-position copies).
pub fn redact_secrets(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.starts_with(b"Bearer ") {
            let n = ascii_run(&rest[7..], |b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')
            });
            if n >= 8 {
                out.push_str(&text[copied..i]);
                out.push_str("Bearer <redacted>");
                i += 7 + n;
                copied = i;
                continue;
            }
        }
        if let Some(prefix) = TOKEN_PREFIXES
            .iter()
            .find(|p| rest.starts_with(p.as_bytes()))
        {
            let n = ascii_run(rest, |b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')
            });
            if n >= prefix.len() + 8 {
                out.push_str(&text[copied..i]);
                out.push_str("<redacted>");
                i += n;
                copied = i;
                continue;
            }
        }
        if let Some(prefix) = KEY_PREFIXES.iter().find(|p| rest.starts_with(p.as_bytes())) {
            let k = prefix.len();
            let n = text[i + k..]
                .char_indices()
                .find(|(_, c)| c.is_whitespace() || matches!(c, '&' | ';' | ',' | '}'))
                .map_or(text.len() - i - k, |(j, _)| j);
            if n > 0 {
                out.push_str(&text[copied..i + k]);
                out.push_str("<redacted>");
                i += k + n;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}
/// Length of the leading run of ASCII bytes that satisfy `keep`.
fn ascii_run(bytes: &[u8], keep: impl Fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|b| keep(**b)).count()
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
