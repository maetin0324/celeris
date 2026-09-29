//! `claude-code` アダプタ（DESIGN §5.4, ADR-0003 D7, ADR-0006）。
//!
//! `claude` CLI は celeris 独自のワーカープロトコルを話さない。`--output-format stream-json` が吐く
//! Claude Code 自身のイベント（`system`/`assistant`/`user`/`result`）を読み、結果ファイル規約
//! （ADR-0006 D3: `artifacts/result.json`）と `result` メッセージ（D4）から `RunOutcome` を合成する。
//! 生存監視（wall-clock・無出力タイムアウト・SIGTERM→SIGKILL）は `subprocess.rs` の低レベル部分を再利用する。

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use task_core::{Check, RateLimitObservation, Task, TaskKind, Usage};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::delegate_file::{clear_delegate_file, forward_delegate_file};
use crate::progress;
use crate::protocol::{Answer, Evidence, ProviderFailure, RunContext, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, adopt_result_json_written_under_work_dir, kill_now,
    read_line_limited, read_tail, reap_after_terminal, write_result_json,
};

/// `[adapters.claude_code]`（config.toml, ADR-0006 D6）。
#[derive(Debug, Clone)]
pub struct ClaudeCodeConfig {
    /// 起動するコマンド名／パス。既定 `"claude"`。
    pub command: String,
    /// 末尾に追加する引数。
    pub extra_args: Vec<String>,
    /// `--permission-mode`。既定 `"bypassPermissions"`（ADR-0006 D6: celeris は許可プロンプトに応答できない）。
    pub permission_mode: String,
    /// `--model`（省略時は claude の既定モデル）。
    pub model: Option<String>,
    /// 追加の環境変数（例: `CLAUDE_CONFIG_DIR`）。
    pub env: Vec<(String, String)>,
    /// ADR-0075 G3-fix1: 子プロセスから外す環境変数（`with_env_removed`。`env` より先に `env_remove` する）。
    pub env_remove: Vec<String>,
    /// ADR-0043 D3（Phase 56）: `Some` なら `claude` をコンテナの中で起こす（`container::wrap`）。
    /// TOML には書かない（ディスパッチャが `with_container` で入れる）。
    pub container: Option<crate::container::SharedPlan>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            command: "claude".to_string(),
            extra_args: Vec::new(),
            permission_mode: "bypassPermissions".to_string(),
            model: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            container: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter {
    config: ClaudeCodeConfig,
}

impl ClaudeCodeAdapter {
    pub const ID: &'static str = "claude-code";

    pub fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for ClaudeCodeAdapter {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        run_claude_code(&self.config, &req, run_id, &limits, sink).await
    }

    /// ADR-0024 D2: `extra` を `config.env` の末尾に足した複製を返す。同名キーは後勝ち（`envs()` に渡す順で
    /// 最後に指定した値が使われる）ので、末尾に足すだけで `extra` が既存の同名キーに勝つ。
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.model = Some(model.to_owned());
        Some(Arc::new(Self::new(config)))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }
    fn with_env_removed(&self, keys: &[String]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        crate::adapter::remove_env_keys(&mut config.env, &mut config.env_remove, keys);
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }

    /// ADR-0043 D3（Phase 56）: コンテナの中で `claude` を起こす複製。
    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }

    /// ADR-0072 D14（Phase E4b 項目3）: `--permission-mode` を上書きした複製。planner run に
    /// `[execution.planner].permission_mode`（既定 `"bypassPermissions"`）を実際の CLI 引数へ反映するために使う
    /// （`with_model`/`with_env` と同じ形。ADR-0072「Phase E3 実装時の逸脱・明確化」で見送っていた
    /// フック）。
    fn with_permission_mode(&self, mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.permission_mode = mode.to_owned();
        Some(Arc::new(ClaudeCodeAdapter::new(config)))
    }
}

/// ADR-0006 Phase 115 D1（本番障害 01M3915FARENW8M0JM11XVF6W0 / 01M38T8N17MEWPTJQXGX1TNYJD）:
/// `work_dir`（実際の cwd）が `workspace` と異なる run（部署のリポジトリの git worktree で走るタスク）
/// だけ、プロンプトの先頭に「cwd と成果物ディレクトリは別」の注意を 2 行足す。`result_json_instructions`
/// は既に `artifacts_dir` の絶対パスで書く（`RunRequest::artifacts_rel` が `work_dir.is_some()` のとき
/// 絶対パスを返す）が、その絶対パスの指示を読み飛ばして相対 `artifacts/` を書いてしまう事故があったため、
/// 冒頭で明示的に注意する。`work_dir` が無い・`workspace` と同じなら何も足さない（既存の文面は
/// 1 バイトも変わらない）。純粋関数。
pub(crate) fn work_dir_note(
    work_dir: Option<&Path>,
    workspace: &Path,
    artifacts_dir: &Path,
) -> String {
    match work_dir {
        Some(wd) if wd != workspace => format!(
            "cwd は `{}`（リポジトリの worktree）。成果物ディレクトリは `{}`。\
             相対 `artifacts/` はリポジトリの中を指すので使わない。\n\n",
            wd.display(),
            artifacts_dir.display()
        ),
        _ => String::new(),
    }
}

/// タスクからワーカーへのプロンプトを組み立てる（ADR-0006 D2, ADR-0007 D7, 純粋関数）。`run_id` は
/// スキーマ変更を避けてプロンプト文面にのみ埋め込む（旧 P-11。ADR-0006 D2 参照）。`task.kind` で分岐する
/// （`Plan` はプランナー用、`Review` はレビュアー用、それ以外は Phase 4 のワーカー用プロンプト。ADR-0007 D7）。
/// `artifacts` は成果物ディレクトリの workspace 相対表記（`RunRequest::artifacts_rel`。ADR-0036 D3。
/// 単独タスクでは `artifacts` なので、文面は Phase 34 までと 1 バイトも変わらない）。
pub fn build_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut prompt = build_prompt_inner(task, context, run_id, artifacts);
    if let Some(browser) = &context.browser {
        prompt.push_str(&crate::browser::prompt(browser));
    }
    prompt
}

fn build_prompt_inner(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    // ADR-0072 D14（Phase E3）: task-local な planner run は `task.kind` に依らず（常に `Execute`）、
    // `context.execution_planner` の有無で選ぶ。
    if context.execution_planner.is_some() {
        return build_execution_plan_prompt(task, context, run_id, artifacts);
    }
    match task.kind {
        // ADR-0074 D3.3（Phase F4a (b)）: `MILESTONES_PLAN_LABEL` の印がある Plan タスクは、案件全体の
        // マイルストーン DAG を `project-plan.json` に書く専用のプロンプト（旧来の `plan.json` の
        // 分解は 1 バイトも変えない）。
        // ADR-0074 D3.4（Phase F4b (e)）: 承認済みの計画がある案件の replan は差分
        // （`celeris.project-plan-delta/1`）を書く専用のプロンプト。
        TaskKind::Plan if task_core::is_milestones_replan_task(task) => {
            build_project_replan_prompt(task, context, run_id, artifacts)
        }
        TaskKind::Plan if task_core::is_milestones_plan_task(task) => {
            build_project_plan_prompt(task, context, run_id, artifacts)
        }
        TaskKind::Plan => build_plan_prompt(task, context, run_id, artifacts),
        TaskKind::Review => build_review_prompt(task, context, run_id, artifacts),
        TaskKind::Execute | TaskKind::Approval => {
            build_execute_prompt(task, context, run_id, artifacts)
        }
    }
}

/// 冒頭の共通部分（タイトル・run_id/attempt・前置き・分野・目的）。前置き（役職と brief・永続の認可・記憶・
/// 直近のやり取り・役割の指示文）は `crate::preamble::render` が組む（ADR-0016 D1 / M3, ADR-0033 D4 / D6）。
/// `task.genre` があり、その分野が `context.available_genres` に載っていれば、続けて `## Genre: <id>` と
/// 説明を出す（ADR-0027 D1）。
fn prompt_header(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Task: {}\n\n", task.title));
    out.push_str(&format!(
        "(run {run_id}, attempt {} of {})\n\n",
        task.attempts + 1,
        task.budget.max_retries + 1
    ));
    // ADR-0033 D4 / D6（Phase 24）: 役職と brief → 永続の認可 → 記憶 → 直近のやり取り → 役割の指示文。
    // 前置きは `crate::preamble` が 1 か所で組む（`RunContext` が空なら 1 バイトも増えない）。
    out.push_str(&crate::preamble::render(context, artifacts));
    if let Some(genre_id) = &task.genre
        && let Some(genre) = context.available_genres.iter().find(|g| &g.id == genre_id)
    {
        out.push_str(&format!(
            "## Genre: {}\n{}\n\n",
            genre.id, genre.description
        ));
    }
    // ADR-0072 D9/D21（Phase E2）: 計画のある Task の WU の run では、Task 全体の objective では
    // なく WU の objective を出す（`context.work_unit` が無い run は E1 までと 1 バイトも変わらない）。
    match &context.work_unit {
        Some(wu) => {
            out.push_str(&format!("## Objective\n{}\n\n", wu.objective));
            out.push_str(&format!(
                "## このタスク全体の目的（参考）\n{}\n\n",
                wu.task_objective_excerpt
            ));
            if !wu.plan_overview.is_empty() {
                out.push_str("## 計画の中のこの WorkUnit\n");
                for line in &wu.plan_overview {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
            if !wu.dependency_summaries.is_empty() {
                out.push_str("## 依存する WorkUnit の完了状況\n");
                for line in &wu.dependency_summaries {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
            // ADR-0079 D7（Phase R3a）: この leaf が待っていた人の決定（固定の書式の行。無ければ出さない）。
            if !wu.human_decisions.is_empty() {
                out.push_str(crate::preamble::HUMAN_DECISIONS_HEADING);
                out.push('\n');
                for line in &wu.human_decisions {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push('\n');
            }
            // ADR-0074 D1.2（Phase F2b）: WU ごとの worktree で走る run だけ（無ければ空）。
            out.push_str(&crate::preamble::work_unit_branch_section(wu));
        }
        None => {
            out.push_str(&format!("## Objective\n{}\n\n", task.objective));
        }
    }
    // ADR-0079 D7（Phase R3a）: 木の節点の worker の run だけ（`false` ならプロンプトは変わらない）。
    if context.decision_requests {
        out.push_str(&crate::preamble::decision_requests_section(artifacts));
    }
    out
}

/// `context.available_genres` の一覧を「使える専門家」の箇条書きに描く（ADR-0027 D1, ADR-0028 D2）。
/// `できること` / `渡すもの…返るもの` の行は、それぞれの一覧が空なら出さない（ADR-0028 D1: 3 フィールドとも任意）。
/// 見出しと、末尾の使い方の説明（委譲 or plan.json）は呼び出し側が足す。
fn genre_list_lines(context: &RunContext) -> String {
    let mut out = String::new();
    for g in &context.available_genres {
        out.push_str(&format!("- {}: {}\n", g.id, g.description));
        if !g.capabilities.is_empty() {
            out.push_str(&format!("  できること: {}\n", g.capabilities.join(" / ")));
        }
        if !g.input_artifacts.is_empty() || !g.output_artifacts.is_empty() {
            out.push_str(&format!(
                "  渡すもの: {} → 返るもの: {}\n",
                g.input_artifacts.join(", "),
                g.output_artifacts.join(", ")
            ));
        }
        let roles = if g.roles.is_empty() {
            "-".to_string()
        } else {
            g.roles
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("  役割: {roles}\n"));
    }
    out
}

/// `context.available_genres` があれば「使える専門家」節を足す（ADR-0027 D1, ADR-0028 D2）。委譲できる run
/// （`build_execute_prompt`）にだけ、この run が子に割り当てられる分野の能力・入出力・役割の選択肢を伝える。
fn available_genres_section(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.available_genres.is_empty() {
        return out;
    }
    out.push_str("## 使える専門家 (available genres and roles you can delegate to)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(&format!(
        "\nIf part of this work belongs to a different genre, delegate it with `role` set to one \
         of that genre's roles and `genre` set to its id in `{artifacts}/delegate.json`.\n\n"
    ));
    out
}

/// Plan run 用の「使える専門家」節（ADR-0028 D3）。子タスクの `genre` / `role` を `artifacts/plan.json` で
/// 選べることを伝える点だけが `available_genres_section` と異なる（委譲ではなく分解なので）。
fn available_genres_section_for_plan(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.available_genres.is_empty() {
        return out;
    }
    out.push_str("## 使える専門家 (available genres and roles you can assign child tasks to)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(&format!(
        "\nIf a child task belongs to a different genre than this one, set its `genre` (and, one of \
         that genre's roles, its `role`) in `{artifacts}/plan.json`.\n\n"
    ));
    out
}

/// 1 つの分野の「成果物の名前は固定」の箇条書き（Phase 38。`名前: 説明` の `名前` と `説明` に分けて出す）。
fn harness_artifact_lines(genre: &crate::protocol::GenreContext) -> String {
    let mut out = format!(
        "- {}: この分野の担当は**ハーネス**で動く。成果物は次の名前で固定され、担当が別のファイルを書くことはできない。\n",
        genre.id
    );
    for (name, description) in genre.output_artifacts_named() {
        match description {
            Some(d) => out.push_str(&format!("  - `{name}`: {d}\n")),
            None => out.push_str(&format!("  - `{name}`\n")),
        }
    }
    out
}

/// Phase 38（ADR-0028 追記。実機のレビュー不合格から）: **Plan run** に、ハーネスで動く分野
/// （`GenreContext::is_harness`）の成果物の規約を出す。実機で、計画が研究文献調査課（PaperQA2）に
/// 「候補テーマを `candidates.json` にまとめよ」と書き、`artifact_exists: candidates.json` を条件に付けた。
/// `candidates.json`（現 `papers.json`）はハーネスが書く検索コーパスの固定名で、担当は計画が決めた名前の
/// ファイルを書けないため、答えの中身が良かったのにレビュアーが基準どおり不合格にした。
/// ハーネスでない分野（coding 等）しか無い設定では**何も出さない**（従来の文面と 1 バイトも変わらない）。
fn harness_artifacts_section_for_plan(context: &RunContext) -> String {
    let harness: Vec<&crate::protocol::GenreContext> = context
        .available_genres
        .iter()
        .filter(|g| g.is_harness() && !g.output_artifacts.is_empty())
        .collect();
    if harness.is_empty() {
        return String::new();
    }
    let mut out = String::from("## ハーネスで動く分野の成果物（名前は固定）\n");
    for genre in harness {
        out.push_str(&harness_artifact_lines(genre));
    }
    out.push_str(
        "受け入れ条件（`artifact_exists`）にはこの名前だけを使うこと。上に無い名前のファイルを要求しても、\n\
         その担当は書けない（条件は celeris が落とし、警告が残る）。**内容の要求は `objective` に書き、\n\
         レビュアー条件（`{\"type\":\"reviewer\"}`）で判定させること**（「X を Y に書け」ではなく\n\
         「答えに X を含めよ」）。\n\n",
    );
    out
}

/// ADR-0033 D4（Phase 24）: 組織図（id / name / brief / genre）。分解・委譲できる run にだけ渡り、
/// 「どの課に何を振るか」を `assignee` で決めさせる。空なら何も出さない（Phase 23 までと同じ出力）。
fn organization_section(context: &RunContext) -> String {
    let mut out = String::new();
    if context.organization.is_empty() {
        return out;
    }
    out.push_str("## 組織図 (who you can assign work to)\n");
    for n in &context.organization {
        let kind = match n.kind {
            task_core::OrgKind::Secretary => "秘書",
            task_core::OrgKind::Department => "部",
            task_core::OrgKind::Section => "課",
        };
        let parent = n.parent_id.as_deref().unwrap_or("-");
        let genre = n.genre.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "- {} [{kind}] {} (親: {parent}, 分野: {genre})",
            n.id, n.name
        ));
        if !n.brief.is_empty() {
            out.push_str(&format!(" — {}", n.brief));
        }
        out.push('\n');
    }
    out.push('\n');
    out
}

/// `context.children` があれば「集約 run」節を足す（ADR-0016 D3 / M4）。
fn children_section(context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    if context.children.is_empty() {
        return out;
    }
    out.push_str("## Delegated child tasks (this is the aggregate run)\n");
    for c in &context.children {
        let role = c.role.as_deref().unwrap_or("-");
        let status = serde_json::to_string(&c.status).unwrap_or_default();
        let status = status.trim_matches('"');
        let outcome = c.outcome.as_deref().unwrap_or("-");
        let workspace = c
            .workspace
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        let artifacts = if c.artifacts.is_empty() {
            "-".to_string()
        } else {
            c.artifacts
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        };
        // ADR-0041 D1: 子が worktree で作業したときだけブランチを足す（無い子の文面は従来どおり）。
        let branch = match &c.branch {
            Some(b) => format!(" branch={b}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "- {} [{role}] status={status} outcome={outcome} workspace={workspace}{branch} artifacts={artifacts}\n",
            c.title
        ));
    }
    out.push_str(&format!(
        "\nSummarize the results of the delegated child tasks in `{artifacts}/summary.md`. \
         The reviewer will check that `{artifacts}/summary.md` exists.\n\n"
    ));
    out
}

/// ADR-0046 D2 / D4 / D5（Phase 59）: 計画には**人選をさせない**。
///
/// Phase 58 までは「上の組織図を見て子タスクごとに `assignee` を必ず書け」と指示していたが、
/// ADR-0046 D5 で担当は決定的な matching が決めるようになった。代わりに、子タスクごとに
/// **`harness` / `skills` / `mode`** を宣言させる（それが matching の入力になる）。
/// 組織図を渡していない run（Phase 23 までの構成）では何も出さない。
fn assignee_instructions_for_plan(context: &RunContext) -> String {
    if context.organization.is_empty() {
        return String::new();
    }
    "## 誰がやるか (ADR-0046 D5)\n\
     **`assignee`（担当）は書くな。組織図から人を選ぶ必要は無い。** 誰がやるかは celeris が決定的に決める\n\
     （必要な skill と harness の重なりで選ぶ）。代わりに、子タスクごとに次の 3 つを書け:\n\
     - `harness`: その仕事の実行契約の id（下の「使える分野」から選ぶ）\n\
     - `skills`: その仕事に**必要な能力タグ**の配列（例 `[\"rust\",\"sqlite\"]`）。小文字・`[a-z0-9._-]`、最大 12 個。\n\
     \u{3000}分からなければ空でよい（その harness を既定に持つ担当に回る）\n\
     - `mode`: `prototype`（動くことを最短で示す）/ `production`（既定。テストと lint を通す）/ \
     `research`（主張に出典か計測を付ける）\n\
     - `repos` と、仕事の制約（検証方法・戻せるか・失敗したときの損失）は objective と acceptance に書け。\n\
     **担当（`assignee`）とモデル（`tier`）は選ぶな。** 書いても使われない（ADR-0069: 担当は matching、\n\
     lane は仕事の性質から celeris が決定的に決める）。\n\n"
        .to_string()
}

/// 同じことを委譲（`artifacts/delegate.json`）側にも書く（ADR-0033 D4）。
/// 部をまたぐ委譲は秘書の認可が要るので、それも伝える（SPEC §3.1）。
fn assignee_instructions_for_delegation(context: &RunContext) -> String {
    if context.organization.is_empty() {
        return String::new();
    }
    "委譲する子には goal（title / objective）・受け入れ条件・`genre`（harness）を書け。**担当（`assignee`）と\n\
     モデル（`tier`）は選ぶな**。書いても使われない（ADR-0069: 担当は celeris が skills と harness から決定的に\n\
     選び、lane は仕事の性質から決める）。上の組織図は「どんな担当がいるか」を知るためだけに使え。\n\n"
        .to_string()
}

/// `context.prior_review` があれば「前回の判定」として列挙する（ADR-0006 D2）。
fn prior_review_section(context: &RunContext) -> String {
    let mut out = String::new();
    if !context.prior_review.is_empty() {
        out.push_str("## Previous attempt's review result (this is a retry)\n");
        for pr in &context.prior_review {
            let verdict = if pr.pass { "pass" } else { "fail" };
            out.push_str(&format!(
                "- criterion {}: {verdict} ({})\n",
                pr.criterion, pr.reason
            ));
        }
        out.push('\n');
    }
    out
}

/// `context.answers` があれば「以前の質問への人間の回答」節として列挙する（ADR-0010 D3, P-10）。
/// Review プロンプトには使わない（`build_review_prompt` からは呼ばない）。
fn answers_section(context: &RunContext) -> String {
    let mut out = String::new();
    if !context.answers.is_empty() {
        out.push_str("## Answers from a human to your earlier questions\n");
        for Answer { question, answer } in &context.answers {
            out.push_str(&format!("- Q: {question}\n  A: {answer}\n"));
        }
        out.push('\n');
    }
    out
}

/// 結果ファイル（`<artifacts_dir>/result.json`）の書式指示（ADR-0006 D3。全 kind 共通。ADR-0036 D3）。
fn result_json_instructions(artifacts: &str) -> String {
    format!(
        "Always write `{artifacts}/result.json` (create the `{artifacts}/` directory if it does not exist \
         yet) as a single JSON object of the form `{{\"summary\": \"<what you did>\", \"evidence\": []}}`. \
         `evidence` may be left empty; if you fill it, each element must be an object of the form \
         `{{\"criterion\": <index>, \"command\": \"<what you ran>\", \"exit\": <code>, \"stdout_tail\": \"...\"}}` \
         (plain strings are not accepted; `command`, `exit` and `stdout_tail` may be omitted for a criterion that \
         did not involve running a command). \
         If you cannot proceed and need a decision from a human, instead write \
         `{{\"question\": \"<your question>\"}}` to `{artifacts}/result.json` and stop there. This is a \
         non-interactive run: you cannot ask a question any other way, and no one will read your final \
         chat message directly.\n"
    )
}

/// ADR-0072 D10（Phase E1）: 予算の予告と rolling checkpoint の指示。coding 系の execute run
/// （対話は除く。D10）にだけ足す。claude-code は `--max-turns` で turn の上限を実際に強制するが、
/// codex/acp/aider は強制しない（目安として出す。§7 U1 と同じ「分類できなければ安全側」の考え方）。
/// **決定的**（壁時計の現在時刻は埋め込まない。DESIGN 原則 1 / 同じ入力から同じプロンプトを保つため。
/// 「開始時刻」は `WorkerStarted` イベントに残るので、ここに書かなくても run の記録からは追える）。
fn budget_preamble(task: &Task, artifacts: &str) -> String {
    format!(
        "## 予算 (budget)\n\
         この run の上限の目安: 最大 {} turn（claude-code はこれを `--max-turns` で実際に強制する。\
         他の harness では目安）/ 最大 {} 秒（壁時計はどの harness でも celeris が強制する）。\n\
         残りが約 20% になったら、または context が長くなってきたと感じたら、作業を区切りのよい所で \
         止め、`{artifacts}/checkpoint.json` を最新の状態にしてから `{artifacts}/result.json` に \
         `{{\"yield\": {{\"completed\": [...], \"remaining\": [...], \"next_action\": \"...\"}}}}` \
         （フィールドは全て任意、`checkpoint.json` と同じ形）を書いて終了せよ（予算切れで打ち切られる \
         より、区切って自分から止まる方が良い。これは失敗ではなく、続きは新しい session が checkpoint \
         から引き継ぐ）。\n\
         意味のある区切り（1 つの小目標の完了・方針の決定・テストの実行）のたびに \
         `{artifacts}/checkpoint.json` を次の形で上書きせよ（無ければ新規に作る。全欄が任意）:\n\
         `{{\"completed\":[\"...\"],\"remaining\":[\"...\"],\
         \"decisions\":[{{\"what\":\"...\",\"why\":\"...\"}}],\
         \"files_changed\":[{{\"path\":\"...\",\"change\":\"modified\"}}],\
         \"tests_run\":[{{\"command\":\"...\",\"exit\":0}}],\
         \"known_failures\":[{{\"what\":\"...\"}}],\"next_action\":\"...\"}}`\n\n",
        task.budget.max_turns, task.budget.max_wall_secs
    )
}

/// 実行中の委譲の方法（ADR-0016 D2 / M8, ADR-0027 D1）。
fn delegation_instructions(artifacts: &str) -> String {
    format!(
        "If you want to delegate part of this work to another agent, write `{artifacts}/delegate.json` \
         (create the `{artifacts}/` directory if it does not exist yet) as a single JSON object of the form \
         `{{\"tasks\":[{{\"title\":\"...\",\"objective\":\"...\",\"acceptance\":[{{\"text\":\"...\",\
         \"check\":{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}}}],\"role\":\"<optional>\",\
         \"genre\":\"<optional>\",\
         \"depends_on\":[<index into this array, or an existing task id>]}}]}}`. \
         Do not choose an assignee or a model tier: celeris assigns the owner deterministically and picks the \
         model lane from the task's nature (ADR-0069). Describe the work, its acceptance checks, and constraints instead.\n\
         `check` may also be \
         `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, `{{\"type\":\"reviewer\"}}`, or `{{\"type\":\"human\"}}`. \
         celeris will validate this after this run ends and insert whatever proposals pass validation as child \
         tasks (how many are accepted per run is limited by configuration; any rejected proposal has its \
         reason recorded as an event you cannot see, but a human can). A task cannot list its own parent or \
         itself in `depends_on`. This task will not be considered done until any children you delegated have \
         finished.\n"
    )
}

/// ADR-0039 D3: 案件が作業場所を決めている run にだけ、委譲の指示に「子は同じ作業場所を継ぐ」を足す。
/// 決めていない案件では空文字列（Phase 42 までと 1 バイトも変わらない）。
fn delegate_workspace_instruction(context: &RunContext) -> String {
    if context.workspace_note.is_none() {
        return String::new();
    }
    "Children you delegate inherit this project's workspace (the location described above), so do not \
     tell them to `ssh` into another host and edit files there. Only if a child must work somewhere else \
     (a different repository or cluster), give it a `workspace` of \
     `{\"kind\":\"local\",\"path\":\"...\"}` or `{\"kind\":\"remote\",\"cluster\":\"...\",\"path\":\"...\"}`.\n"
        .to_string()
}

/// ADR-0039 D3: 計画 run（`build_plan_prompt`）用。子タスクが継ぐ作業場所と、`plan.json` の `workspace` の
/// 使いどころを伝える。案件が作業場所を決めていない run では空文字列（従来どおりの文面）。
fn workspace_section_for_plan(context: &RunContext) -> String {
    let Some(note) = &context.workspace_note else {
        return String::new();
    };
    format!(
        "## 子タスクの作業場所 (the workspace child tasks inherit)\n\
         {note}\n\
         コードを扱う仕事はこの場所で行われ、分解した子タスクはこの作業場所をそのまま継ぐ。担当に \
         `ssh` でリモートの作業ツリーへ直接書かせてはいけない（同期は celeris が行う）。**別の場所**\
         （別のリポジトリ・別のクラスタ）で作業させたい子にだけ、`workspace` に \
         `{{\"kind\":\"local\",\"path\":\"...\"}}` か \
         `{{\"kind\":\"remote\",\"cluster\":\"...\",\"path\":\"...\"}}` を書け。\n\n"
    )
}

/// `Execute`（および `Approval`）用プロンプト（ADR-0006 D2。既存のワーカー用プロンプトのまま）。
fn build_execute_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    // ADR-0072 D9（Phase E1）: 続きの実行（continuation）の節。`context.continuation` が無い run の
    // 出力はここで 1 バイトも増えない。
    out.push_str(&crate::preamble::continuation_section(context));
    // ADR-0072 D10（Phase E1）: 予算の予告と rolling checkpoint の指示。対話 run には出さない
    // （D10: 対話・レビュー・計画・研究 harness は除く）。
    if context.conversation_addressee.is_none() {
        out.push_str(&budget_preamble(task, artifacts));
    }
    // ADR-0072 D9（Phase E2）: 計画のある Task の WU の run では、`## Acceptance criteria` は
    // この WU の `done_when` にし、Task の受け入れ条件は「最終レビューで確かめる」参考として別に出す
    // （`context.work_unit` が無い run は E1 までと 1 バイトも変わらない）。
    if let Some(wu) = &context.work_unit {
        out.push_str("## Acceptance criteria (this WorkUnit)\n");
        if wu.done_when.is_empty() {
            out.push_str(
                "(no explicit done_when was given for this WorkUnit; use the objective above)\n",
            );
        }
        for (i, c) in wu.done_when.iter().enumerate() {
            out.push_str(&format!("{i}. {c}\n"));
        }
        out.push('\n');
        out.push_str("## 最終レビューで確かめる Task の受け入れ条件（参考）\n");
        for (i, c) in task.acceptance.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i, c.text));
        }
        out.push('\n');
    } else {
        out.push_str("## Acceptance criteria\n");
        for (i, c) in task.acceptance.iter().enumerate() {
            let detail = match &c.check {
                Check::Command { cmd, expect_exit } => format!(
                    " (a reviewer will independently re-run `{cmd}` in this directory afterwards and \
                     requires exit code {expect_exit}; your own claim of success is not trusted)"
                ),
                Check::ArtifactExists { name } => {
                    format!(" (a reviewer will check that the file `{artifacts}/{name}` exists)")
                }
                Check::KnowledgePage { path } => {
                    format!(" (a human will check the knowledge base page `{path}`)")
                }
                Check::Reviewer | Check::Human => String::new(),
            };
            out.push_str(&format!("{}. {}{}\n", i, c.text, detail));
        }
        out.push('\n');
    }
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&children_section(context, artifacts));
    out.push_str(&organization_section(context));
    out.push_str(&available_genres_section(context, artifacts));
    out.push_str(&assignee_instructions_for_delegation(context));
    out.push_str("## Instructions\n");
    out.push_str("Work in the current directory (it is a dedicated workspace for this task). ");
    // ADR-0054 D2 / Phase 98 追記: 対話 run（`conversation_addressee` が Some）は結果ファイルを書いて
    // 返事だけをする（`preamble::conversation_instructions` の `actions` 経由でしか仕事を作れない）。
    // 対話 run の一部（CoS）は read-only サンドボックスで `artifacts/delegate.json` を書けないため、
    // 汎用の delegate.json 段落をここで出すと「委譲ファイルを作成できない」と誤って諦める
    // （実機障害 2026-09-22 00:18 UTC、task 01M337NT3QT1FR1G6WHS9G6NDA）。対話でない run の文面は
    // 1 バイトも変えない。
    if context.conversation_addressee.is_some() {
        out.push_str(
            "この run は返事だけを書く。仕事は返事の `actions` で作る（ファイルは書けない）。\n",
        );
    } else {
        out.push_str(&delegation_instructions(artifacts));
        out.push_str(&delegate_workspace_instruction(context));
    }
    out.push_str("When you are done:\n");
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// `Plan` kind 用プロンプト（DESIGN §5.6, ADR-0007 D7）。目標を独立に検証可能な受け入れ条件を持つ
/// 子タスク群に分解させ、`artifacts/plan.json` に `PlanOutput` を書かせる。
fn build_plan_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         Decompose this goal into a set of child tasks, each with an independently verifiable \
         acceptance criterion or criteria (DESIGN §5.6: \"目標を、独立に検証可能な受け入れ条件を持つ \
         子タスク群に分解せよ\"). Prefer 3 to 6 child tasks when the size of the goal makes that \
         reasonable (DESIGN §6 Phase 5 acceptance criteria); use fewer or more only if the goal \
         clearly requires it.\n\n",
    );
    out.push_str(&format!(
        "Write your decomposition to `{artifacts}/plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"tasks\":[{{\"title\":\"...\",\"objective\":\"...\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"command\",\"cmd\":\"...\",\
         \"expect_exit\":0}}}}],\"depends_on\":[<index into this same tasks array>],\
         \"kind\":\"execute\"|\"plan\" (omit for \"execute\"),\
         \"harness\":\"<harness id>\", \"skills\":[\"<skill tag>\"], \
         \"mode\":\"prototype\"|\"production\"|\"research\" (optional), \
         \"repos\":[\"<repo name>\"] (optional)}}]}}\n\
         ```\n\
         Do not write `assignee` or `tier`: the owner and the model lane are decided by celeris (ADR-0069).\n\
         `check` may also be `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, \
         `{{\"type\":\"knowledge_page\",\"path\":\"...\"}}` (a page in the knowledge base), \
         `{{\"type\":\"reviewer\"}}`, or `{{\"type\":\"human\"}}`. Unknown fields are rejected, so do not \
         add any field not shown above. `depends_on` indices refer to positions within this same \
         `tasks` array and must form a DAG (no self-reference, no cycles). Every child task must have \
         at least one `acceptance` entry. If a child has a `human` check, its `acceptance` must also \
         include an `artifact_exists` or `knowledge_page` check pointing at the human-readable material \
         (ADR-0067: it must live in registered artifacts or the knowledge base, not in the target \
         repository's tracked files, so a human can see it from the GUI). `kind:\"plan\"` children are \
         only allowed while the total decomposition depth stays within {} (DESIGN §5.6 \"分解の深さは \
         上限 3\"; this plan itself already counts toward that limit). Any `command` check will later be \
         re-run for real inside the child task's own working directory by an independent reviewer, so \
         do not fabricate a command whose result you have not actually observed.\n\n",
        task_core::plan::MAX_PLAN_DEPTH
    ));
    let schema = serde_json::to_string(&task_core::plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    // ADR-0046 D5（Phase 59）: 計画に組織図は渡さない（人選をさせない）。代わりに harness / skills /
    // mode を宣言させ、担当は決定的な matching が決める。
    out.push_str(&assignee_instructions_for_plan(context));
    out.push_str(&available_genres_section_for_plan(context, artifacts));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&harness_artifacts_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.3（Phase F4a (b)）: 案件レベルの計画（マイルストーン Task の DAG）を
/// `{artifacts}/project-plan.json`（`celeris.project-plan/1`）に書かせるプロンプト。旧来の
/// `build_plan_prompt`（`plan.json`、単発の分解）とは別物 — ここでは**案件全体**の途中目標の並びを
/// 設計させる（SPEC §7「途中目標は予め大まかに決めておいて、適宜再設計する」）。
fn build_project_plan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         Design this project's milestones as a DAG: break the request into 1 to 12 milestones, each a \
         top-level, independently reportable unit of progress (SPEC §7: \"途中目標は予め大まかに決めておいて、\
         適宜再設計する\"). A milestone is not a single small task — it is a stage of the project a human \
         will explicitly approve or reject once it is reached (ADR-0038: ok / discuss / ng). Order them with \
         `depends_on` (a DAG, not necessarily a single chain): independent milestones may run in parallel, \
         while a milestone that needs another milestone's result depends on it.\n\n",
    );
    out.push_str(&format!(
        "Write your plan to `{artifacts}/project-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"celeris.project-plan/1\",\"rationale\":\"...\",\
         \"milestones\":[{{\"key\":\"<[a-z0-9-]{{1,32}}, unique>\",\"title\":\"...\",\"objective\":\"...\",\
         \"reach_criteria\":\"what must be shown for a human to call this milestone reached\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"human\"}}}}],\
         \"depends_on\":[\"<key of another milestone in this array>\"],\
         \"genre\":\"<optional>\",\"skills\":[\"<skill tag>\"],\"repos\":[\"<repo name>\"]}}]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model` or `lane`: the owner and the model lane are decided by \
         celeris (ADR-0069). `check` may also be `{{\"type\":\"artifact_exists\",\"name\":\"...\"}}`, \
         `{{\"type\":\"knowledge_page\",\"path\":\"...\"}}`, `{{\"type\":\"reviewer\"}}`, or \
         `{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}`. Unknown fields are rejected. Every \
         milestone must have at least one `acceptance` entry, and if it includes a `human` check, the \
         `acceptance` must also include an `artifact_exists` or `knowledge_page` check pointing at the \
         human-readable material (ADR-0067: it must live in registered artifacts or the knowledge base, not \
         in the target repository's tracked files). `depends_on` values must be keys of other milestones in \
         this same array and must form a DAG (no self-reference, no cycles). At most 12 milestones.\n\n",
    ));
    let schema = serde_json::to_string(&task_core::project_plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/project-plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&available_genres_section_for_project_plan(context));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.4（Phase F4b (e)）: 案件の replan。目的（objective）に現行の計画（`base_version` と各
/// マイルストーンの key・状態・変更できるか）が載っているので、それに対する**差分**を
/// `{artifacts}/project-plan.json`（`celeris.project-plan-delta/1`）に書かせる。
fn build_project_replan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         This project already has an approved milestone plan (a DAG of milestones; the objective above lists \
         it with its `base_version`, each milestone's key, state and whether it can still be changed). \
         Revise the plan by writing a **delta** against it — do not rewrite the whole plan. The current plan \
         keeps running until a human approves your delta, so only change what the reason for this replan \
         requires.\n\n\
         Rules: `modify` and `remove` may only name milestones that have not been dispatched yet (marked \
         変更可). A milestone that is running or finished cannot be modified or removed; if it must stop, \
         list it in `cancel` explicitly (a running one will be cancelled on approval; a finished one cannot \
         be cancelled). A key may appear in only one of `modify` / `remove` / `cancel`. New milestones go to \
         `add` with keys that do not collide with any existing key. After the delta, every `depends_on` must \
         name a milestone that is still in the plan (so when you remove or cancel a milestone, modify the \
         not-yet-dispatched milestones that depended on it), the plan must stay a DAG, and it must keep 1 to \
         12 milestones.\n\n",
    );
    out.push_str(&format!(
        "Write the delta to `{artifacts}/project-plan.json` (create the `{artifacts}/` directory if it does \
         not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"celeris.project-plan-delta/1\",\"base_version\":<the base_version above>,\
         \"rationale\":\"why the plan changes\",\
         \"add\":[<milestone objects in the celeris.project-plan/1 shape>],\
         \"modify\":[{{\"key\":\"<existing key>\",\"title\":\"<only the fields you change>\"}}],\
         \"remove\":[\"<existing key>\"],\"cancel\":[\"<existing key>\"]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model` or `lane` (ADR-0069). Unknown fields are rejected. Every \
         added milestone needs at least one `acceptance` entry, and a `human` check needs an \
         `artifact_exists` or `knowledge_page` check next to it (ADR-0067).\n\n",
    ));
    let schema = serde_json::to_string(&task_core::project_plan::delta_schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/project-plan.json` delta object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&available_genres_section_for_project_plan(context));
    out.push_str(&workspace_section_for_plan(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// ADR-0074 D3.3（Phase F4a (b)）: 案件計画専用の「使える専門家」節。`available_genres_section_for_plan`
/// と違い、milestone spec には `role` が無いので `genre` だけを宣言させる（子タスクではなくマイルストーン
/// の話であることも明示する）。
fn available_genres_section_for_project_plan(context: &RunContext) -> String {
    if context.available_genres.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("## 使える専門家 (available genres a milestone can be tagged with)\n");
    out.push_str(&genre_list_lines(context));
    out.push_str(
        "\nIf a milestone's work belongs to a particular genre, set its `genre` in \
         `project-plan.json`. Do not set `role` (milestones have none; the owning Task's role, if any, is \
         decided when it is dispatched).\n\n",
    );
    out
}

/// ADR-0072 D14（Phase E3）: task-local な planner run 用プロンプト。Complexity Gate が compound と
/// 判定した Task を WorkUnit の集合に分解させ、`artifacts/execution-plan.json` に
/// `ExecutionPlanSpec`（`celeris.execution-plan/1`）を書かせる。この run 自身は普通のワーカー run と
/// 同じ `result.json` の契約（`summary`/`evidence`、または `question`）で終わる。計画の検証・採用・
/// 不正なら 1 回だけ再試行・それでも駄目なら atomic に倒す判断は daemon（`task-dispatch`）が行う
/// （このプロンプトの役目ではない）。
fn build_execution_plan_prompt(
    task: &Task,
    context: &RunContext,
    run_id: &str,
    artifacts: &str,
) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         This task was judged too large or too broad for a single session (ADR-0072 Complexity Gate). \
         Decompose it into a small number of WorkUnits, each of which fits comfortably in one context \
         window (investigation, design, implementation, tests, release — whatever split makes sense for \
         this goal). Prefer the smallest number of WorkUnits that keeps each one focused; do not split \
         further than the goal actually requires. You are planning, not doing the work yourself: do not \
         write code or run the actual implementation in this run.\n\n",
    );
    // ADR-0074 D1.1（Phase F2b）: `[execution] parallel = true` のときだけ v2 を書かせる。
    let parallel = context
        .execution_planner
        .as_ref()
        .is_some_and(|p| p.parallel);
    let planner_schema = if parallel {
        task_core::EXECUTION_PLAN_SCHEMA_V2
    } else {
        task_core::EXECUTION_PLAN_SCHEMA
    };
    let max_work_units = context
        .execution_planner
        .as_ref()
        .map(|p| p.max_work_units)
        .unwrap_or(8);
    let work_unit_max_turns = context
        .execution_planner
        .as_ref()
        .map(|p| p.work_unit_max_turns)
        .unwrap_or(80);
    let work_unit_max_wall_secs = context
        .execution_planner
        .as_ref()
        .map(|p| p.work_unit_max_wall_secs)
        .unwrap_or(3600);
    let default_max_turns = context
        .execution_planner
        .as_ref()
        .map(|p| p.default_max_turns)
        .unwrap_or(30);
    let default_max_wall_secs = context
        .execution_planner
        .as_ref()
        .map(|p| p.default_max_wall_secs)
        .unwrap_or(1800);
    let tree = context
        .execution_planner
        .as_ref()
        .and_then(|p| p.tree.as_ref());
    if let (Some(planner), Some(tree)) = (context.execution_planner.as_ref(), tree) {
        // ADR-0079 D4 (2)（Phase R2b）: 木の節点の planner は /3（段階・unit = leaf | 子 task・決定）を書く。
        out.push_str(&tree_plan_shape_section(planner, tree, artifacts));
    } else {
        out.push_str(&format!(
        "Write your plan to `{artifacts}/execution-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"{planner_schema}\",\"rationale\":\"...\",\"work_units\":[{{\"key\":\"survey\",\
         \"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"repair\"|\"other\",\
         \"title\":\"...\",\"objective\":\"...\",\"depends_on\":[\"<key of another work unit>\"],\
         \"done_when\":[\"...\"],\"checks\":[{{\"cmd\":\"...\",\"expect_exit\":0}}],\
         \"context\":{{\"paths\":[\"...\"],\"from_work_units\":[\"<key>\"],\"knowledge\":[\"...\"]}},\
         \"harness\":\"<genre id, or omit to inherit this task's genre>\",\
         \"features\":{{\"judgment\":\"low\"|\"medium\"|\"high\",\"ambiguity\":\"low\"|\"medium\"|\"high\",\
         \"verifiability\":\"low\"|\"medium\"|\"high\",\"reversibility\":\"low\"|\"medium\"|\"high\",\
         \"consequence\":\"low\"|\"medium\"|\"high\"}} (see below),\
         \"budget\":{{\"max_turns\":40,\"max_wall_secs\":1800}} (optional),\
         \"outputs\":[\"...\"]}}]}}\n\
         ```\n\
         Do not write `assignee`, `tier`, `model`, or `lane`: the owner never changes (WorkUnits inherit \
         this task's owner) and the model lane is decided by celeris from each WorkUnit's nature \
         (ADR-0069, ADR-0074). Unknown fields are rejected, so do not add any field not shown above. \
         `key` must match `[a-z0-9-]{{1,32}}` and be unique within this plan. `depends_on` refers to \
         other `key`s in this same array and must form a DAG (no cycles). `checks` may only be \
         `{{\"cmd\":\"...\",\"expect_exit\":0}}` (a deterministic command check; do not fabricate one \
         you have not actually run — it will really be executed later). Give every mechanically \
         checkable WorkUnit an executable `checks` entry: it raises that WorkUnit's verifiability and \
         lets it route to a cheaper lane instead of defaulting to this task's own lane. `harness`, if \
         set, must be one of the genre ids listed below (available genres); an unset `harness` inherits \
         this task's own genre. Plan at most {max_work_units} WorkUnits (a plan with more will be \
         rejected and this run retried once). If you write a `budget` for a WorkUnit, `max_turns` will \
         be capped at {work_unit_max_turns} and `max_wall_secs` at {work_unit_max_wall_secs}; if you \
         omit it, the default is max({default_max_turns}, task budget) turns and \
         max({default_max_wall_secs}, task budget) seconds.\n\n"
    ));
    }
    // Phase F5-fix3: 検証が使う上限をすべて、計画の形の説明のすぐ後に出す。前の試行が拒否されていれば、
    // その理由も（同じ間違いを繰り返させない。dogfood 4 回目は 2 回とも `too many checks: 8 > 6`）。
    if let Some(planner) = &context.execution_planner {
        out.push_str(&plan_limits_section(planner));
        out.push_str(&previous_attempt_errors_section(planner, artifacts));
    }
    let schema = serde_json::to_string(&task_core::execution_plan::schema_value())
        .unwrap_or_else(|_| "{}".to_string());
    out.push_str(&format!(
        "### Schema for the `{artifacts}/execution-plan.json` object\n```json\n"
    ));
    out.push_str(&schema);
    out.push_str("\n```\n\n");
    out.push_str(&work_unit_features_section());
    if let Some(planner) = context
        .execution_planner
        .as_ref()
        .filter(|p| p.parallel && p.tree.is_none())
    {
        out.push_str(&parallel_phases_section(planner.max_phases));
    }
    if let Some(planner) = &context.execution_planner {
        out.push_str(&format!(
            "## Why this task was judged compound (Complexity Gate, rule `{}`, score {})\n",
            planner.gate_rule_id, planner.gate_score
        ));
        if planner.gate_signals.is_empty() {
            out.push_str(
                "(no scored signals — a human or the CoS marked this task compound directly)\n\n",
            );
        } else {
            for line in &planner.gate_signals {
                out.push_str(&format!("- {line}\n"));
            }
            out.push('\n');
        }
    }
    // ADR-0072 D17（Phase E4b 項目1）: replan のときだけ、今の計画・WU の状態・起こした理由を足す
    // （`replan = false` の run は 1 バイトも変わらない）。
    if let Some(planner) = context.execution_planner.as_ref().filter(|p| p.replan) {
        match &planner.tree {
            Some(tree) => out.push_str(&tree_replan_context_section(planner, tree)),
            None => out.push_str(&replan_context_section(planner)),
        }
    }
    if !context.available_genres.is_empty() {
        out.push_str("## Available genres (valid values for a WorkUnit's `harness`)\n");
        out.push_str(&genre_list_lines(context));
        out.push('\n');
    }
    out.push_str(&prior_review_section(context));
    out.push_str(&answers_section(context));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// Phase F5-fix3: planner context の上限を `ExecutionLimits` に戻す。0（古い request で欄が無い）は
/// `ExecutionLimits::default()` の値に倒す。
fn planner_limits(
    planner: &crate::protocol::ExecutionPlannerContext,
) -> task_core::ExecutionLimits {
    let d = task_core::ExecutionLimits::default();
    let or = |v: usize, dv: usize| if v == 0 { dv } else { v };
    task_core::ExecutionLimits {
        max_work_units: or(planner.max_work_units, d.max_work_units),
        work_unit_max_turns: if planner.work_unit_max_turns == 0 {
            d.work_unit_max_turns
        } else {
            planner.work_unit_max_turns
        },
        work_unit_max_wall_secs: if planner.work_unit_max_wall_secs == 0 {
            d.work_unit_max_wall_secs
        } else {
            planner.work_unit_max_wall_secs
        },
        max_rationale_chars: or(planner.max_rationale_chars, d.max_rationale_chars),
        max_title_chars: or(planner.max_title_chars, d.max_title_chars),
        max_objective_chars: or(planner.max_objective_chars, d.max_objective_chars),
        max_done_when_items: or(planner.max_done_when_items, d.max_done_when_items),
        max_done_when_chars: or(planner.max_done_when_chars, d.max_done_when_chars),
        max_checks: or(planner.max_checks, d.max_checks),
        max_plan_json_bytes: or(planner.max_plan_json_bytes, d.max_plan_json_bytes),
        // `max_work_units` は dispatcher が v1/v2 に応じて選んだ値（`max_work_units_v2` を含む）。
        max_work_units_v2: or(planner.max_work_units, d.max_work_units_v2),
        max_phases: or(planner.max_phases, d.max_phases),
        max_children: or(planner.max_children, d.max_children),
        // ADR-0079 D4 (2)（Phase R2b）: /3 の planner には検証と同じ木の上限を渡す（`context.tree`）。
        tree: match &planner.tree {
            Some(t) => task_core::TreeLimits {
                enabled: true,
                max_depth: t.max_depth,
                max_units_per_stage: t.max_units_per_stage,
                max_stages: t.max_stages,
                max_child_tasks_per_plan: t.max_child_tasks_per_plan,
                max_parallel_child_tasks: t.max_parallel_child_tasks,
                max_open_decisions_per_plan: t.max_decisions_per_plan,
                ..d.tree
            },
            None => d.tree,
        },
    }
}

/// Phase F5-fix3: 計画の上限（`task_core::execution_plan::validate` が拒否する条件）を全部並べる。
/// 値は run の `ExecutionPlannerContext`（dispatcher が実際に検証に使う `ExecutionLimits` から埋める）。
fn plan_limits_section(planner: &crate::protocol::ExecutionPlannerContext) -> String {
    let l = planner_limits(planner);
    let mut out = String::from(
        "### Plan limits (celeris validates the plan against these; a plan that exceeds ANY of them \
         is rejected as a whole)\n",
    );
    if planner.tree.is_some() {
        // ADR-0079 D3（Phase R2b）: /3 の計画の上限（検証で拒否）。
        out.push_str(&format!(
            "- `stages`: 1 to {} stages. At most {} units per stage (leaves + child tasks; celeris-added \
             integration steps and repairs do not count). At most {} units with `\"kind\":\"task\"`. At most \
             {} `decisions` (plan-level and unit-level together).\n",
            l.tree.max_stages,
            l.tree.max_units_per_stage,
            l.tree.max_child_tasks_per_plan,
            l.tree.max_open_decisions_per_plan
        ));
    } else {
        out.push_str(&format!(
            "- `work_units`: 1 to {} WorkUnits (celeris-added integration steps and repairs do not count).\n",
            l.max_work_units
        ));
    }
    if planner.parallel && planner.tree.is_none() {
        out.push_str(&format!(
            "- `phases`: 1 to {} phases. `children`: at most {} child tasks.\n",
            l.max_phases, l.max_children
        ));
    }
    out.push_str(&format!(
        "- Per WorkUnit: `title` at most {} characters; `objective` at most {} characters; at most {} \
         `done_when` items, each at most {} characters; **at most {} `checks`**. If more commands need to \
         run, combine related ones into a single check (e.g. `{{\"cmd\":\"cargo fmt --all -- --check && \
         cargo clippy --workspace -- -D warnings\",\"expect_exit\":0}}`) or move them into `done_when`.\n",
        l.max_title_chars,
        l.max_objective_chars,
        l.max_done_when_items,
        l.max_done_when_chars,
        l.max_checks
    ));
    out.push_str(&format!(
        "- `rationale` at most {} characters; the whole plan JSON at most {} bytes.\n",
        l.max_rationale_chars, l.max_plan_json_bytes
    ));
    if planner.tree.is_some() {
        out.push_str(&format!(
            "- A leaf `budget` above `max_turns` {} or `max_wall_secs` {} is **rejected** (not capped): such a \
             unit does not fit one run, so declare it as a child task or split it.\n",
            l.work_unit_max_turns, l.work_unit_max_wall_secs
        ));
    } else {
        out.push_str(&format!(
            "- A WorkUnit `budget` is not rejected but capped: `max_turns` at {}, `max_wall_secs` at {}.\n",
            l.work_unit_max_turns, l.work_unit_max_wall_secs
        ));
    }
    if planner.replan && planner.tree.is_none() {
        out.push_str(
            "- When replanning with a diff, the limits apply to the resulting plan after the diff is \
             applied (including the WorkUnits that carry over unchanged).\n",
        );
    }
    out.push_str(
        "Count these yourself before you finish: celeris does not trim or merge anything for you.\n\n",
    );
    out
}

/// Phase F5-fix3: 前の planner run の計画が拒否された理由（同じ計画の回の中）。拒否された計画のファイルは
/// dispatcher が `execution-plan.rejected.json` に移してある（そのまま再提出させない）。
fn previous_attempt_errors_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    artifacts: &str,
) -> String {
    if planner.previous_attempt_errors.is_empty() {
        return String::new();
    }
    let mut out = String::from("### Your previous plan was REJECTED\n");
    out.push_str(
        "A previous planner run for this same plan wrote a plan that celeris rejected with these \
         validation error(s):\n",
    );
    for e in &planner.previous_attempt_errors {
        out.push_str(&format!("- {e}\n"));
    }
    out.push_str(&format!(
        "\nThe rejected file was moved to `{artifacts}/execution-plan.rejected.json`. Do NOT resubmit it \
         unchanged and do not assume any plan file you find is valid: start from it if useful, fix \
         exactly the problems listed above (keep everything else as it was), check the result against \
         the plan limits above, and write the corrected plan to `{artifacts}/execution-plan.json`.\n\n"
    ));
    out
}

/// ADR-0079 D4 (2) / D7 / D12（Phase R2b）: 木の節点の planner の計画の形（`celeris.execution-plan/3`）、木の中の
/// 位置（深さ・残りの深さ・祖先）、leaf の基準、決定の要求の書き方、木の上限の残り、人の段階の名指し。
/// `context.execution_planner.tree` があるときだけ、/1・/2 の形の説明の代わりに出す。
fn tree_plan_shape_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    tree: &crate::protocol::TreePlannerContext,
    artifacts: &str,
) -> String {
    let schema = task_core::EXECUTION_PLAN_SCHEMA_V3;
    let l = planner_limits(planner);
    let mut out = String::new();
    out.push_str(&format!(
        "### Plan shape: `{schema}` (recursive task tree, ADR-0079)\n\
         Write your plan to `{artifacts}/execution-plan.json` (create the `{artifacts}/` directory if it \
         does not exist yet) as a single JSON object of exactly this shape:\n\
         ```json\n\
         {{\"schema\":\"{schema}\",\"rationale\":\"...\",\
         \"stages\":[{{\"key\":\"build\",\"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"other\",\
         \"title\":\"...\",\"review\":\"none\"|\"human\" (optional)}}],\
         \"units\":[\
         {{\"key\":\"api\",\"stage\":\"build\",\"kind\":\"investigate\"|\"design\"|\"implement\"|\"test\"|\"release\"|\"other\",\
         \"title\":\"...\",\"objective\":\"...\",\"depends_on\":[\"<unit key>\"],\"needs_decisions\":[\"<decision key>\"],\
         \"done_when\":[\"...\"],\"checks\":[{{\"cmd\":\"...\",\"expect_exit\":0}}],\
         \"context\":{{\"repo\":\"<one repository>\",\"paths\":[\"...\"],\"from_work_units\":[\"<key>\"],\"knowledge\":[\"...\"]}},\
         \"harness\":\"<genre id, or omit>\",\"features\":{{...}},\
         \"budget\":{{\"max_turns\":40,\"max_wall_secs\":1800}} (optional),\"outputs\":[\"...\"]}},\
         {{\"key\":\"part-b\",\"stage\":\"build\",\"kind\":\"task\",\"title\":\"...\",\"objective\":\"...\",\
         \"acceptance\":[{{\"text\":\"...\",\"check\":{{\"type\":\"command\",\"cmd\":\"...\",\"expect_exit\":0}}}}],\
         \"depends_on\":[\"<unit key>\"],\"needs_decisions\":[\"<decision key>\"],\"genre\":\"<optional>\",\
         \"skills\":[\"<skill tag>\"],\"repos\":[\"<subset of this task's repositories>\"],\"features\":{{...}}}}],\
         \"decisions\":[{{\"key\":\"h1\",\"question\":\"...\",\
         \"options\":[{{\"key\":\"a\",\"label\":\"...\",\"consequence\":\"...\"}},{{\"key\":\"b\",\"label\":\"...\"}}],\
         \"recommended\":\"a\",\"cost_of_reversal\":\"low\"|\"medium\"|\"high\",\"cost_note\":\"...\",\
         \"needed_before\":[\"<unit key>\"|\"stage:<stage key>\"]}}]}}\n\
         ```\n\
         A unit is either a **leaf** (any `kind` except `task`: one WorkUnit, finished by one run) or a \
         **child task** (`\"kind\":\"task\"`: its own task with its own acceptance, final review, gate and — if \
         needed — its own plan). Do not write `phases`, `work_units` or `children` (those are the older \
         shapes), and do not write `assignee`, `tier`, `model`, `lane` or `adopt`. Unknown fields are \
         rejected. Keys (stages, units, decisions, options) match `[a-z0-9-]{{1,32}}`; unit keys are unique \
         in the plan and must not start with `integrate-` (celeris adds one integration step per stage and \
         merges every leaf branch and child-task branch of the stage into this task's branch).\n\
         Stages run in array order. Units of the same stage may run in parallel; a unit may depend on at \
         most one unit of its own stage, and `depends_on` may point to the same or an earlier stage only. \
         `\"review\":\"human\"` pauses after that stage for a human check; use it only when a human really \
         has to look before the next stage.\n\n"
    ));

    out.push_str("#### Where this task sits in the tree\n");
    out.push_str(&format!(
        "- Depth: {} (task levels; the root task is depth 1). Max depth: {}. **Remaining depth: {}**.\n",
        tree.depth, tree.max_depth, tree.remaining_depth
    ));
    if tree.remaining_depth >= 1 {
        out.push_str(
            "- You may declare units with `\"kind\":\"task\"` (child tasks). Each child task runs its own \
             complexity gate: a small one runs as a single run, a large one plans itself.\n",
        );
    } else {
        out.push_str(
            "- Remaining depth is 0: **do NOT write any unit with `\"kind\":\"task\"`** (such a plan is \
             rejected). Every unit must be a leaf. If a piece of work cannot fit in a leaf, raise a decision \
             for it (see below) instead of forcing it into a leaf.\n",
        );
    }
    if !tree.ancestors.is_empty() {
        out.push_str("- Ancestors (root first):\n");
        for (i, a) in tree.ancestors.iter().enumerate() {
            let stage = a
                .stage
                .as_deref()
                .map(|s| format!(" (stage `{s}`)"))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - depth {}: \"{}\"{stage} — {}\n",
                i + 1,
                a.title,
                a.objective_excerpt.replace('\n', " ")
            ));
        }
    }
    out.push('\n');

    out.push_str(&format!(
        "#### Leaf criteria (ADR-0079 D4) — every leaf must meet ALL three\n\
         (a) **Bounded**: it finishes in one run — `budget.max_turns` ≤ {} and `budget.max_wall_secs` ≤ {} \
         (a larger budget is rejected, not capped). Continuation is a safety net, not a plan.\n\
         (b) **One area, one repository**: `context.repo` names at most one repository and `context.paths` \
         stays within one subtree.\n\
         (c) **At least one command check**: `checks` has at least one deterministic command \
         (`{{\"cmd\":\"...\",\"expect_exit\":0}}`) that really verifies the result.\n\
         Declare a unit as a child task (`\"kind\":\"task\"`) when it fails any of these, when it is a \
         deliverable that should be accepted and reviewed on its own, when it needs a human acceptance or a \
         decision (`needs_decisions`), or when it needs another department's skills. A child-task unit needs \
         `acceptance` (at least one criterion) and must NOT have `checks`, `budget`, `harness` or \
         `context.paths` (the child decides those itself); its `repos` must be a subset of this task's \
         repositories.\n\
         **A unit that cannot fit in a leaf must be declared as a child task{} or raised as a decision — never \
         squeezed into a leaf.** celeris re-gates every unit when it adopts the plan and records any \
         disagreement with your declaration.\n\n",
        l.work_unit_max_turns,
        l.work_unit_max_wall_secs,
        if tree.remaining_depth >= 1 {
            ""
        } else {
            " (not possible at this depth)"
        }
    ));

    out.push_str(&format!(
        "#### Decisions for a human (`decisions`, ADR-0079 D7)\n\
         Raise a decision only when the work needs a choice that you cannot make from the objective, the \
         acceptance criteria and the repository (for example: which external service to depend on, which of \
         two incompatible designs the human wants, whether to spend a scarce resource). Do not ask about \
         things you can decide yourself. Each decision has 2 to 5 `options` (with `key` and `label`, \
         optionally `consequence`), a `recommended` option key, `cost_of_reversal` (`low` | `medium` | \
         `high`, how expensive it is to change the answer later) and a non-empty `needed_before`: the unit \
         keys or `stage:<key>` that must wait for the answer. Units that wait list the decision key in their \
         `needs_decisions`. Only what waits for an answer stops; every other unit proceeds. At most {} \
         decisions per plan. You may also write `decisions` inside a unit (the unit is then added to \
         `needed_before`).\n\n",
        l.tree.max_open_decisions_per_plan
    ));

    out.push_str("#### Remaining limits of this tree\n");
    out.push_str(&format!(
        "- This plan: at most {} stages, {} units per stage, {} child-task units, {} decisions (a plan \
         beyond these is rejected).\n",
        l.tree.max_stages,
        l.tree.max_units_per_stage,
        l.tree.max_child_tasks_per_plan,
        l.tree.max_open_decisions_per_plan
    ));
    let tokens = tree
        .tokens_left
        .map(|t| format!(", tokens left: {t}"))
        .unwrap_or_default();
    out.push_str(&format!(
        "- Whole tree (all tasks under the root): leaves left: {}, runs left: {}, replans left: {} (this \
         task: {}){tokens}, open decisions left: {}. Work beyond these limits is stopped and a human is \
         asked, so plan within them: prefer fewer, well-bounded units.\n",
        tree.leaves_left,
        tree.runs_left,
        tree.replans_left,
        tree.node_replans_left,
        tree.open_decisions_left
    ));
    out.push_str(&format!(
        "- At most {} child tasks of this task run at the same time (the rest wait; nothing is dropped).\n\n",
        tree.max_parallel_child_tasks
    ));

    if !tree.stages_hint.is_empty() {
        out.push_str("#### Stages the human named (stages_hint, ADR-0079 D12)\n");
        for h in &tree.stages_hint {
            if h.scope.is_empty() {
                out.push_str(&format!("- \"{}\"\n", h.title));
            } else {
                out.push_str(&format!("- \"{}\": {}\n", h.title, h.scope));
            }
        }
        out.push_str(
            "Use these as the guide for your stages (their names and scope, in this order). They are input, \
             not a required structure: keep the whole scope the human asked for and do not narrow it.\n\n",
        );
    }
    out
}

/// ADR-0079 D9（Phase R2b）: /3 の replan の節。差分（`execution-plan-delta/1`）は /2 の形しか持たないので、/3 は
/// 計画の全体を書かせる（done の unit は daemon が採用した spec のまま持ち越す）。子 task の失敗は理由・checkpoint の
/// 要約つきで `replan_reason` と unit の要約に入っている。
fn tree_replan_context_section(
    planner: &crate::protocol::ExecutionPlannerContext,
    _tree: &crate::protocol::TreePlannerContext,
) -> String {
    let mut out = String::from("## You are REPLANNING an existing execution plan\n");
    out.push_str(&format!(
        "This is not the first plan for this task: a previous plan already ran, and something about it \
         needs to change. Write the whole new plan in the `{}` shape above (the diff schema is not \
         available for this shape). The plan limits apply to the resulting plan including the units that \
         carry over.\n\n",
        task_core::EXECUTION_PLAN_SCHEMA_V3
    ));
    if let Some(version) = planner.current_plan_version {
        out.push_str(&format!("Current (superseded) plan version: v{version}.\n"));
    }
    out.push_str(&format!(
        "Why this replan was triggered: {}\n\n",
        if planner.replan_reason.is_empty() {
            "(not recorded)"
        } else {
            planner.replan_reason.as_str()
        }
    ));
    if !planner.work_unit_summaries.is_empty() {
        out.push_str(
            "### Units in the current plan (status and, for child tasks, the child's outcome)\n",
        );
        for line in &planner.work_unit_summaries {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if !planner.preserve_done_keys.is_empty() {
        out.push_str(&format!(
            "These units are already done and carry over unchanged (celeris restores their adopted spec; you \
             may omit them, and you must not change them): {}.\n\n",
            planner.preserve_done_keys.join(", ")
        ));
    }
    out.push_str(
        "For a child-task unit whose child **failed** you may: keep the same unit key (celeris then creates a \
         new child task from that unit — the next attempt; fix its objective or acceptance so the retry can \
         succeed, using the failure reason above), split it into several units, drop it, or raise a decision \
         if a human has to choose. The failed child's branch and history are kept. A unit whose child is \
         still running keeps its child. A key that belonged to a unit you removed earlier cannot be reused.\n\n",
    );
    out
}

/// ADR-0074 D1.1（Phase F2b）: v2（工程と並列の WU）の書き方。`[execution] parallel = true` の planner
/// run にだけ足す（v1 のプロンプトは 1 バイトも変わらない）。
fn parallel_phases_section(max_phases: usize) -> String {
    let max_phases = if max_phases == 0 { 5 } else { max_phases };
    let mut out = String::from("### Phases and parallel WorkUnits (`celeris.execution-plan/2`)\n");
    out.push_str(&format!(
        "Group the WorkUnits into ordered phases. Add a top-level `\"phases\":[{{\"key\":\"build\",\
         \"kind\":\"implement\",\"title\":\"...\"}}, ...]` array (1 to {max_phases} phases; the array \
         order is the execution order; `key` matches `[a-z0-9-]{{1,32}}`) and give every WorkUnit a \
         `\"phase\":\"<phase key>\"`. Rules (a plan that breaks them is rejected):\n\
         - WorkUnits in the **same phase may run in parallel**, each in its own git worktree and branch. \
         Put independent pieces of work (different files/modules) in the same phase; celeris merges \
         each phase's branches deterministically at the end of the phase and re-runs the checks.\n\
         - Within a phase a WorkUnit may depend on **at most one** other WorkUnit of the same phase \
         (it then starts from that WorkUnit's branch). If it needs two or more, put it in a later phase.\n\
         - `depends_on` may point to WorkUnits in the same phase or an earlier phase, never a later one.\n\
         - Do not use keys starting with `integrate-` and do not write `\"kind\":\"integrate\"` \
         (celeris adds one integration step per phase itself).\n\n"
    ));
    // ADR-0074 D3.7（Phase F4b (f)）: 子 Task の提案（`children`）の書き方。
    out.push_str(
        "#### Child tasks (`children`, optional)\n\
         Only when part of this goal is really a **separate deliverable** — it needs a different \
         department's skills, a different repository, or a human wants to approve it on its own — propose \
         it as a child task instead of a WorkUnit: add a top-level `\"children\":[{\"key\":\"<[a-z0-9-]{1,32}>\",\
         \"title\":\"...\",\"objective\":\"...\",\"acceptance\":[{\"text\":\"...\",\"check\":{\"type\":\"command\",\
         \"cmd\":\"...\",\"expect_exit\":0}}],\"genre\":\"<optional>\",\"skills\":[\"<skill tag>\"],\
         \"features\":{...},\"depends_on\":[\"<another child key>\"]}]` array (at most 8). Do not write \
         `assignee`, `tier` or `model` (celeris picks the owner). A WorkUnit that needs a child's result \
         waits for it with `\"depends_on\":[\"child:<key>\"]`. celeris creates the children through the same \
         checks as delegation when it adopts the plan; a child in another department is first confirmed \
         with the secretary. Do not split a WorkUnit that is merely too large into children — make it \
         smaller WorkUnits instead. Otherwise leave `\"children\"` out.\n\n",
    );
    out.push_str(
        "Example: `{\"schema\":\"celeris.execution-plan/2\",\"rationale\":\"...\",\"phases\":[\
         {\"key\":\"build\",\"kind\":\"implement\",\"title\":\"core pieces\"},\
         {\"key\":\"verify\",\"kind\":\"test\",\"title\":\"end-to-end\"}],\"work_units\":[\
         {\"key\":\"api\",\"phase\":\"build\",...},{\"key\":\"store\",\"phase\":\"build\",...},\
         {\"key\":\"e2e\",\"phase\":\"verify\",\"depends_on\":[\"api\",\"store\"],...}]}`.\n\n",
    );
    out
}

/// ADR-0074 D5.1（Phase F1）: WU ごとの `features`（`TaskFeatureHints` の軸）の説明と例。planner
/// プロンプトの JSON 例の直後に足す（E6-1: 例に無く、planner は書かなかった。書かせても
/// `.ok()` で黙って捨てていた。今は `execution_plan::validate` が読めない `features` を拒否する）。
fn work_unit_features_section() -> String {
    let mut out = String::from(
        "### `features` (per-WorkUnit routing hints, ADR-0074 D5.1)\n\
         Write at least these five axes for every WorkUnit so celeris can route it to the cheapest \
         lane that still fits (a WorkUnit with no `features` just inherits this task's own lane, which \
         wastes budget on mechanical WorkUnits). Each axis is `\"low\"`, `\"medium\"`, or `\"high\"`. Do \
         not write `lane`, `tier`, `assignee`, or `model` here — celeris decides the lane from these \
         axes plus whether `checks` is set.\n\n",
    );
    out.push_str(
        "- `judgment`: how much open-ended judgment or design taste this WorkUnit needs. `low` = \
         mechanical (rename, update a price table, apply a known fix); `high` = real design or \
         investigation conclusions.\n\
         - `ambiguity`: how underspecified the goal is. `low` = the objective and `done_when`/`checks` \
         fully pin down the outcome; `high` = the WorkUnit has to make judgment calls about what \
         \"done\" even means.\n\
         - `verifiability`: whether the result can be checked deterministically. `high` = an executable \
         `checks` command decides pass/fail; `low` = only a human can tell.\n\
         - `reversibility`: how easy a mistake is to undo. `high` = changes live in this task's own \
         worktree/branch; `low` = touches production, other people's data, or something sent externally.\n\
         - `consequence`: how bad a mistake would be. `low` for routine internal changes; `high` for \
         anything security-, billing-, or data-loss-adjacent.\n\n",
    );
    out.push_str(
        "Example: updating a static price table with a `cargo test` check that confirms the new \
         values is `{\"judgment\":\"low\",\"ambiguity\":\"low\",\"verifiability\":\"high\",\
         \"reversibility\":\"high\",\"consequence\":\"low\"}` (mechanical, checkable, safe to redo — \
         routes to the cheap lane). Designing a new routing policy with no executable check is \
         `{\"judgment\":\"high\",\"ambiguity\":\"high\",\"verifiability\":\"low\",\
         \"reversibility\":\"high\",\"consequence\":\"medium\"}` (routes to the frontier lane, capped by \
         this task's own lane).\n\n",
    );
    out
}

/// ADR-0072 D17（Phase E4b 項目1）: replan run のプロンプトに足す節。今の計画の版・WU ごとの状態・
/// 起こした理由・保持すべき `done` の WU の key を出す。`v2` の出力は `done` の WU を変えてはいけない、
/// と明示する（D14 の検証がこれを拒否することも書く）。
fn replan_context_section(planner: &crate::protocol::ExecutionPlannerContext) -> String {
    let mut out = String::from("## You are REPLANNING an existing execution plan\n");
    out.push_str(
        "This is not the first plan for this task: a previous plan already ran, and something \
         about it needs to change.\n\n",
    );
    let delta_schema = task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA;
    let base_version = planner.current_plan_version.unwrap_or(1);
    out.push_str(&format!(
        "Prefer writing a DIFF instead of the full plan (ADR-0074 D5.3): write \
         `{{\"schema\":\"{delta_schema}\",\"base_version\":{base_version},\"rationale\":\"...\",\
         \"add\":[<new WorkUnit specs, same shape as above>],\
         \"modify\":[{{\"key\":\"<existing key>\", ...only the fields you are changing...}}],\
         \"remove\":[\"<key>\", ...]}}` to `{{artifacts}}/execution-plan.json`. Do NOT restate \
         WorkUnits you are not changing — they carry over automatically, including every WorkUnit \
         that is already done (you cannot touch a done WorkUnit's spec anyway; see below). If a \
         diff genuinely cannot express what you need, you may instead write the full \
         `\"schema\":\"{}\"` plan shape shown above (still subject to the done-WorkUnit rule).\n\n",
        task_core::EXECUTION_PLAN_SCHEMA
    ));
    if let Some(version) = planner.current_plan_version {
        out.push_str(&format!("Current (superseded) plan version: v{version}.\n"));
    }
    out.push_str(&format!(
        "Why this replan was triggered: {}\n\n",
        if planner.replan_reason.is_empty() {
            "(not recorded)"
        } else {
            planner.replan_reason.as_str()
        }
    ));
    if !planner.work_unit_summaries.is_empty() {
        out.push_str("### WorkUnits in the current plan\n");
        for line in &planner.work_unit_summaries {
            out.push_str(&format!("- {line}\n"));
        }
        out.push('\n');
    }
    if planner.preserve_done_keys.is_empty() {
        out.push_str(
            "No WorkUnit in the current plan is done yet, so you may replace all of them.\n\n",
        );
    } else {
        out.push_str(&format!(
            "IMPORTANT: these WorkUnit keys are already done and MUST appear unchanged (same \
             `key`, same spec — objective, depends_on, done_when, checks, context, harness, \
             budget, outputs) in your new plan; do not edit, rename, remove, or reorder them. A \
             plan that changes a done WorkUnit will be rejected: {}.\n\n",
            planner.preserve_done_keys.join(", ")
        ));
    }
    out
}

/// Phase 38（ADR-0028 追記）: レビュー対象のタスクがハーネスで動く分野なら、レビュアーにも成果物の規約を
/// 渡す（`context.subject_genre`。ディスパッチャが決定的に入れる）。ハーネスでない分野・分野が無いタスクの
/// レビューでは何も出さない（従来の文面と 1 バイトも変わらない）。
fn harness_artifacts_section_for_review(context: &RunContext) -> String {
    let Some(genre) = context.subject_genre.as_ref().filter(|g| g.is_harness()) else {
        return String::new();
    };
    let names = genre.output_artifacts_named();
    let Some(&(answer, _)) = names.first() else {
        return String::new();
    };
    let mut out = String::from("## この担当の成果物（名前は固定。判定はこの前提で行う）\n");
    out.push_str(&harness_artifact_lines(genre));
    out.push_str(&format!(
        "上のどれかに「答え」があり、それ以外は道具の記録である（例: `papers.json` は検索したコーパスで\n\
         あって答えではない。答えは `{answer}`）。テーマ候補・考察・結論といった**内容は `{answer}` の中で\n\
         判定せよ**。受け入れ条件が上に無いファイル名を求めていても、その担当にはそれを書く手段が無いので、\n\
         ファイル名の不一致だけを理由に不合格にはせず、要求された**内容**が `{answer}` にあるかで判定せよ。\n\n"
    ));
    out
}

/// `Review` kind 用プロンプト（DESIGN §5.7, ADR-0007 D5/D7）。対象タスクの成果物を読み取り専用で
/// 検証し `artifacts/review.json` に判定を書かせる。
fn build_review_prompt(task: &Task, context: &RunContext, run_id: &str, artifacts: &str) -> String {
    let mut out = prompt_header(task, context, run_id, artifacts);
    out.push_str(
        "## Instructions\n\
         You are a reviewer independently verifying another worker's output. You must not modify any \
         files in this directory — this is a read-only inspection of the working directory. If in \
         doubt about whether a criterion is actually satisfied, set `pass` to false and explain why: a \
         false pass is worse than a false fail (completion is decided by review, not by the worker's \
         own claim).\n\n",
    );
    out.push_str("## Acceptance criteria of the task under review\n");
    for (i, c) in task.acceptance.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i, c.text));
    }
    out.push('\n');
    out.push_str(&harness_artifacts_section_for_review(context));
    match &context.review {
        Some(review) => {
            out.push_str(&format!(
                "## Worker's self-reported summary (not to be trusted blindly)\n{}\n\n",
                review.summary
            ));
            out.push_str("## Criteria you must judge in this run\n");
            for idx in &review.criteria {
                out.push_str(&format!("- criterion {idx}\n"));
            }
            out.push('\n');
            out.push_str("## Evidence self-reported by the worker (not to be trusted blindly)\n");
            if review.evidence.is_empty() {
                out.push_str("(none reported)\n");
            }
            for e in &review.evidence {
                // ADR-0012 D3: command / exit / stdout_tail は任意。
                let command = e
                    .command
                    .as_deref()
                    .map(|c| format!(" command `{c}`"))
                    .unwrap_or_default();
                let exit = e.exit.map(|x| format!(" exit={x}")).unwrap_or_default();
                let tail = e
                    .stdout_tail
                    .as_deref()
                    .map(|t| format!(" stdout_tail={t:?}"))
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- criterion {}:{command}{exit}{tail}\n",
                    e.criterion
                ));
            }
            out.push('\n');
        }
        None => {
            out.push_str("## Review context\nno review context\n\n");
        }
    }
    out.push_str("## Input artifacts produced by the run under review\n");
    if context.inputs.is_empty() {
        out.push_str("(none)\n");
    }
    for a in &context.inputs {
        out.push_str(&format!(
            "- {} at `{}` (sha256={})\n",
            a.name, a.path, a.sha256
        ));
    }
    out.push('\n');
    out.push_str(&format!(
        "## Result\n\
         Write your verdicts to `{artifacts}/review.json` (create the `{artifacts}/` directory if it does \
         not exist yet) as a single JSON object of exactly this shape: \
         `{{\"verdicts\":[{{\"criterion\":<index>,\"pass\":<bool>,\"reason\":\"...\"}}]}}`. You must write \
         exactly one verdict for each criterion listed under \"Criteria you must judge in this run\" \
         above.\n\n"
    ));
    out.push_str(&result_json_instructions(artifacts));
    out
}

/// `artifacts/result.json`（ADR-0006 D3）。
#[derive(Debug, Deserialize)]
struct ResultFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    question: Option<String>,
    /// 生の JSON で受け、`lenient_evidence` で整形する（ADR-0006 D3: `evidence` の内容の正確さは要求しない。
    /// Phase 5 のドッグフードで、ワーカーが文字列の配列を書いて run 全体が `error` になる事故があった）。
    #[serde(default)]
    evidence: serde_json::Value,
    /// ADR-0072 D9/D10（Phase E1）: graceful yield（`{"yield": {...checkpoint の意味の欄...}}`）。
    /// 中身は寛容に読む（`task_core::WorkerCheckpointInput` と同じ形。生の JSON のまま保持し、
    /// 検証・合成はディスパッチャ側の `task_core::merge_checkpoint` が行う）。
    #[serde(default, rename = "yield")]
    r#yield: Option<serde_json::Value>,
}

/// `evidence` のうち `Evidence` として読めた要素だけを残す。配列でない／要素が不正でも `done` を失敗にしない。
fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<Evidence>(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// stream-json の最後に観測した `{"type":"result",...}`（ADR-0006 D4）。
#[derive(Debug, Clone)]
struct ResultMeta {
    subtype: String,
    is_error: bool,
    usage: Option<Usage>,
    /// `result` フィールド（文字列。エラー時の文面）。供給側失敗の分類に使う（ADR-0010 D5）。
    result: Option<String>,
    /// F5-fix5: `result` の `stop_reason`（`end_turn` など。無ければ `None`）。
    stop_reason: Option<String>,
    /// F5-fix5: この `result` を観測した時点で、まだ終わっていなかった background task の説明
    /// （`BackgroundTasks::outstanding`）。headless の run は turn を終えると background task を殺す。
    outstanding_background: Vec<String>,
}

/// F5-fix5: stream-json の `system` 行（`task_started` / `task_notification` / `task_updated`）から追う
/// background task（Claude Code CLI 2.1.283 の形。本番の run 01M3KF2HFMHPJR7YEB5HMT38MQ の stdout.jsonl
/// 17・52・53 行目）。未知の形は無視する（ADR-0006 D5）。
#[derive(Debug, Default)]
struct BackgroundTasks {
    /// 開始順の `(task_id, description)`（`is_backgrounded: false` と明示されたものは除く）。
    started: Vec<(String, String)>,
    /// 終わった（通知・状態更新で完了・失敗・停止・kill が観測された）task_id。
    finished: std::collections::BTreeSet<String>,
}

impl BackgroundTasks {
    fn observe(&mut self, value: &serde_json::Value) {
        let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
        let Some(task_id) = value.get("task_id").and_then(|s| s.as_str()) else {
            return;
        };
        match subtype {
            "task_started" => {
                if value.get("is_backgrounded").and_then(|b| b.as_bool()) == Some(false) {
                    return;
                }
                if self.started.iter().any(|(id, _)| id == task_id) {
                    return;
                }
                let description = value
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or(task_id);
                self.started
                    .push((task_id.to_string(), truncate(description, 300)));
            }
            // 終わりの通知（`status`: completed / failed / stopped …）。どの status でも「もう走っていない」。
            "task_notification" => {
                self.finished.insert(task_id.to_string());
            }
            "task_updated" => {
                let status = value
                    .pointer("/patch/status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if matches!(
                    status,
                    "completed" | "failed" | "killed" | "stopped" | "cancelled"
                ) {
                    self.finished.insert(task_id.to_string());
                }
            }
            _ => {}
        }
    }

    /// まだ終わっていない background task の説明（開始順）。
    fn outstanding(&self) -> Vec<String> {
        self.started
            .iter()
            .filter(|(id, _)| !self.finished.contains(id))
            .map(|(_, d)| d.clone())
            .collect()
    }
}

/// F5-fix5: headless の run が background task を残して turn を終えたときの、続きの run への申し送り
/// （`Terminal::Yielded` の checkpoint。ADR-0072 D9 の continuation）。worker 自身の `checkpoint.json`
/// があれば、その欄を土台にして `next_action` と `known_failures` だけを足す。
async fn headless_background_checkpoint(
    artifacts_dir: &Path,
    outstanding: &[String],
) -> serde_json::Value {
    let mut checkpoint = match tokio::fs::read_to_string(artifacts_dir.join("checkpoint.json"))
        .await
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    {
        Some(v @ serde_json::Value::Object(_)) => v,
        _ => serde_json::json!({}),
    };
    let commands = outstanding
        .iter()
        .map(|c| format!("`{c}`"))
        .collect::<Vec<_>>()
        .join("、");
    let previous_next = checkpoint
        .get("next_action")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("（前の run の次の一手: {s}）"))
        .unwrap_or_default();
    let next_action = format!(
        "前の run は headless なのに background で command を走らせたまま turn を終えたため、その command は \
         run の終わりと同時に殺され、結果は残っていない。background を使わず foreground で（Bash の \
         `timeout` を長めに指定して）もう一度実行し、結果を確かめてから続け、最後に result.json を書くこと: \
         {commands}{previous_next}"
    );
    if let Some(obj) = checkpoint.as_object_mut() {
        obj.insert("next_action".into(), serde_json::Value::String(next_action));
        let mut failures = obj
            .get("known_failures")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for c in outstanding {
            failures.push(serde_json::json!({
                "what": format!("headless_background_task: killed when the turn ended: {c}"),
            }));
        }
        obj.insert("known_failures".into(), serde_json::Value::Array(failures));
    }
    checkpoint
}

/// F5-fix5: 続き（continuation）に回してよい run か。coding 系の execute run だけ（ADR-0072 D10 の
/// 予算の予告と同じ範囲。対話・計画・レビューの run は continuation の仕組みを持たない）。
fn is_continuable_execute_run(task: &Task, context: &RunContext) -> bool {
    matches!(task.kind, TaskKind::Execute | TaskKind::Approval)
        && context.execution_planner.is_none()
        && context.conversation_addressee.is_none()
}

async fn run_claude_code(
    config: &ClaudeCodeConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    // Browser commands and page content must not be persisted through the harness's raw
    // stream capture. Parsing still uses the pipes; the browser supervisor emits safe audit events.
    let (stdout_log_path, stderr_log_path) = if req.context.browser.is_some() {
        (
            Path::new("/dev/null").to_path_buf(),
            Path::new("/dev/null").to_path_buf(),
        )
    } else {
        (run_dir.join("stdout.jsonl"), run_dir.join("stderr.log"))
    };
    // `stderr_task` (below) moves a copy into its `async move` block; this one stays available for
    // the crash-classification read after the loop (ADR-0010 D5).
    let stderr_log_path_for_task = stderr_log_path.clone();

    // 前回の run（リトライ）が残した結果ファイルを、今回の run の結果と誤読しないよう先に消す
    // （監査で指摘。ADR-0006 D3 は「この run が書いたファイル」を前提にしている）。
    // ADR-0036 D1/D2: 置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_rel = req.artifacts_rel();
    let result_path = req.artifact_path("result.json");
    let _ = tokio::fs::remove_file(&result_path).await;
    clear_delegate_file(&req.artifacts_dir).await;
    // ADR-0006 Phase 115 D2: 同じ理由（前回の run の名残と誤読しない）で、work_dir 側の名残候補も消す
    // （`work_dir != workspace` のときだけ意味がある。無ければ何もしない）。
    if let Some(work_dir) = req.work_dir.as_deref()
        && work_dir != req.workspace
    {
        let _ = tokio::fs::remove_file(work_dir.join("artifacts").join("result.json")).await;
    }

    let prompt = format!(
        "{}{}",
        work_dir_note(req.work_dir.as_deref(), &req.workspace, &req.artifacts_dir),
        build_prompt(&req.task, &req.context, run_id, &artifacts_rel)
    );
    // ADR-0023 D2 / M1: この run で何を渡したかを残す（`request.json` は構造、`prompt.txt` は実際の文面）。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &prompt, run_id).await;
    // ADR-0056 D3（Phase 79）: mount された skills を `.claude/skills/<name>/` に写す（claude-code が
    // 自動で読む形式。run が失敗しても打ち切らない。読み取れる限りは失敗しない見込み — 失敗すれば
    // ワーカー起動前の警告としてログに残す）。
    if let Err(e) = crate::skills::deliver_claude_code(req.cwd(), &req.context.skills).await {
        warn!("run {run_id}: failed to deliver skills to .claude/skills: {e}");
    }

    let mut command = Command::new(&config.command);
    // F5-fix10（本番障害 run 01M3Q21Z9JQWWANGHXJPNH1F8X）: プロンプトは argv に載せず stdin で渡す
    // （`-p/--print` は値を取らない真偽フラグで、位置引数の prompt が無ければ stdin を読む。Claude Code
    // CLI 2.1.284 の `--help`: `Usage: claude [options] [command] [prompt]`・`--input-format` 既定 "text"）。
    // 135,644 バイトの replan プロンプトが Linux の MAX_ARG_STRLEN（131072）を超えて spawn が E2BIG で
    // 落ちたため。書き込みは spawn 直後に `feed_stdin`（別タスク）で行う。
    command
        .arg("-p")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--permission-mode")
        .arg(&config.permission_mode)
        .arg("--max-turns")
        .arg(req.task.budget.max_turns.to_string())
        // F5-fix5: どの run（worker / planner / reviewer / 対話）にも、headless であること・turn を
        // 終えると run が終わること・長い command も foreground で走らせることを system prompt に足す。
        .arg("--append-system-prompt")
        .arg(crate::preamble::HEADLESS_RUN_NOTE);
    // ADR-0054 D1（Phase 67）: `context.session`（この run が継続セッションの一部）が無ければ、
    // Phase 66 までと同じ `--no-session-persistence`（session を残さない）。ある場合は、そのアダプタが
    // `claude-code` のときだけ、初回は `--session-id <id>`（これから使う id を固定）、2 回目以降は
    // `--resume <id>`（続ける）に切り替える。他アダプタ向けの `session` は無視する（渡り歩きは無い）。
    // ADR-0054 D1（Phase 67）: `resume` を頼んだ run かどうかは、後段の crash 分類（resume 失敗の
    // 検出）でも使う。
    let is_resuming = req
        .context
        .session
        .as_ref()
        .is_some_and(|s| s.adapter == ClaudeCodeAdapter::ID && s.resume);
    // ADR-0054 Phase 67b 追記: Claude Code CLI 2.1.278 は `--session-id`/`--resume` に渡す id が UUID
    // でなければ拒否する（本番で ULID を渡してすべての CoS 対話・部門長レビュー run が失敗した事故。
    // 2026-09-21）。celeris 側の発行（`crate::sessions` 相当。呼び出し元は `resolve_node_session` の
    // 自己修復）は Phase 67b で直したが、ここでも**境界で** spawn 前に拒否する（将来の回帰がテストで
    // 静かに ULID を通してしまわないよう、falsely-loud に落とす）。
    match req.context.session.as_ref() {
        Some(session) if session.adapter == ClaudeCodeAdapter::ID && session.resume => {
            if !crate::provider::is_valid_uuid(&session.session_id) {
                return Err(AdapterError::Other(format!(
                    "refusing to --resume claude-code session id {:?}: not a valid UUID \
                     (Claude Code CLI 2.1.278+ requires one; ADR-0054 Phase 67b)",
                    session.session_id
                )));
            }
            command.arg("--resume").arg(&session.session_id);
        }
        Some(session) if session.adapter == ClaudeCodeAdapter::ID => {
            if !crate::provider::is_valid_uuid(&session.session_id) {
                return Err(AdapterError::Other(format!(
                    "refusing to --session-id claude-code session id {:?}: not a valid UUID \
                     (Claude Code CLI 2.1.278+ requires one; ADR-0054 Phase 67b)",
                    session.session_id
                )));
            }
            command.arg("--session-id").arg(&session.session_id);
        }
        _ => {
            command.arg("--no-session-persistence");
        }
    }
    if let Some(model) = &config.model {
        command.arg("--model").arg(model);
    }
    // ADR-0054 D2（Phase 68）: CoS の対話 run だけ、読み取りだけの `celerisctl` を許す
    // （`Bash(celerisctl <サブコマンド>:*)` の形。claude-code の `--allowedTools` はこの許可リストに
    // 無い道具を拒否する＝それ以外は禁止のまま。ADR-0033 D4 の「対話 run は道具を使わない」の例外）。
    if req.context.conversation_addressee == Some(crate::protocol::ConversationAddressee::Secretary)
    {
        let allowed = crate::protocol::CONVERSATION_READONLY_CELERISCTL
            .iter()
            .map(|sub| format!("Bash(celerisctl {sub}:*)"))
            .collect::<Vec<_>>()
            .join(",");
        command.arg("--allowedTools").arg(allowed);
    }
    command.args(&config.extra_args);
    // ADR-0075 G3-fix1: 継いだ値を外してから重ねる（コンテナ実行では `container::wrap` が無視する）。
    crate::adapter::apply_env_removal(&mut command, &config.env_remove);
    // F5-fix5: headless の run では background task を無効にする（Claude Code CLI 2.1.283 は
    // `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` が立っていると Bash / Agent の `run_in_background` を道具の
    // schema から外す）。background が無いぶん、foreground の Bash が run の壁時計まで待てるよう
    // `BASH_MAX_TIMEOUT_MS`（既定 600000）を壁時計に合わせる。どちらも `config.env` が同名を持てば
    // そちらが勝つ（後に `envs` で重ねる）。
    command.env(DISABLE_BACKGROUND_TASKS_ENV, "1").env(
        BASH_MAX_TIMEOUT_ENV,
        limits
            .wall_clock
            .as_millis()
            .clamp(600_000, u128::from(u32::MAX))
            .to_string(),
    );
    command
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd());
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::container::wrap(command, config.container.as_deref());
    // F5-fix10: stdin はプロンプトを渡すためだけに開く（書き終えたら閉じるので、対話の入力待ちにはならない。
    // コンテナ実行は `container::argv` が `-i` を付けているので stdin がそのまま中に届く）。
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(ClaudeCodeAdapter::ID, &command)?;

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());
    // F5-fix10: プロンプトを stdin に流して閉じる（別タスク。下の stdout 読み取りと並行に進む）。
    let stdin_writer =
        crate::subprocess::feed_stdin(&mut child, prompt, ClaudeCodeAdapter::ID, run_id)?;
    // ADR-0054 D1（Phase 67）: 起動できたら、このアダプタ宛ての継続セッションの id をそのまま報告する
    // （claude-code は id を`自分で`固定するので、成功した spawn の直後に確定する）。
    if let Some(session) = req
        .context
        .session
        .as_ref()
        .filter(|s| s.adapter == ClaudeCodeAdapter::ID)
    {
        sink.session_established(&session.session_id);
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Other("worker stderr was not piped".into()))?;

    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr;
        match tokio::fs::File::create(&stderr_log_path_for_task).await {
            Ok(mut file) => {
                if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
                    warn!("failed to write worker stderr.log: {e}");
                }
            }
            Err(e) => warn!("failed to create worker stderr.log: {e}"),
        }
    });

    let mut stdout_file = tokio::fs::File::create(&stdout_log_path).await?;
    let mut reader = BufReader::new(stdout);

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_result: Option<ResultMeta> = None;
    let mut background = BackgroundTasks::default();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            // ADR-0072 D7（Phase E1）: wall-clock の打ち切りは予算切れ（continuation の対象）。
            // usage はここでは取れない（`result` メッセージを観測する前に打ち切っている）。
            timeout_terminal = Some(Terminal::BudgetExhausted {
                kind: task_core::BudgetKind::WallClock,
                message: "wall clock exceeded".into(),
                usage: None,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            // ADR-0072 D7（Phase E1）: idle timeout は E1 では harness_error に分類変更しない
            // （§7 U7。従来どおり retryable な `Error` のまま attempts を消費する）。
            timeout_terminal = Some(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let wait = (limits.wall_clock - wall_elapsed).min(limits.idle_timeout - idle_elapsed);

        let outcome = match tokio::time::timeout(
            wait,
            read_line_limited(&mut reader, MAX_LINE_BYTES),
        )
        .await
        {
            Err(_elapsed) => continue, // タイムアウト。ループ先頭で上限超過を検知する。
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                // claude 自身のフォーマットは celeris が定義したものではないため、寛容に無視する（ADR-0006 D5）。
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from claude stdout");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    handle_line(trimmed, sink, &mut last_result, &mut background);
                }
            }
        }
    }

    let exit_status = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };
    // F5-fix10: 子は刈り取った。まだ書き込み中なら（読まずに終わった子）打ち切る。
    stdin_writer.abort();

    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;

    let (mut terminal, provider_failure): (Terminal, Option<ProviderFailure>) = match (
        timeout_terminal,
        &last_result,
    ) {
        // タイムアウト（wall-clock / idle）は分類しない（ADR-0010 D5）。
        (Some(t), _) => (t, None),
        // `result` メッセージを一度も観測できずに exit した場合はクラッシュとして扱い、
        // artifacts/result.json（前回の run の名残や書きかけの内容）を一切信用しない（ADR-0006 D4）。
        // stderr.log の末尾を供給側失敗として分類する（ADR-0010 D5）。
        (None, None) => {
            let exit_repr = match exit_status.code() {
                Some(code) => code.to_string(),
                None => "signal".to_string(),
            };
            let tail = read_tail(&stderr_log_path, 4096).await;
            let pf = classify_provider_failure(&tail);
            // ADR-0054 D1（Phase 67）: resume を頼んだ run が、セッションを拒否されたように見える
            // crash なら報告する（ディスパッチャが `node_sessions` を retire し、次の run は新規
            // セッションになる）。文言は実機で確認していない（`provider::looks_like_resume_rejection`
            // のコメント参照）。
            if is_resuming && crate::provider::looks_like_resume_rejection(&tail) {
                sink.session_resume_failed(&tail);
            }
            (
                Terminal::Error {
                    message: format!("worker exited without a result message (exit={exit_repr})"),
                    retryable: true,
                },
                pf,
            )
        }
        (None, Some(meta)) => {
            // ADR-0006 Phase 115 D2: `result.json` を読む前に、work_dir 側の名残を採用する
            // （Phase 112 D3 相当の「回収」はこのアダプタには無いが、同じ原則で最優先に判定する）。
            adopt_result_json_written_under_work_dir(
                &req.artifacts_dir,
                req.work_dir.as_deref(),
                &req.workspace,
                run_id,
            )
            .await;
            let mut outcome = terminal_from_result(
                &req.artifacts_dir,
                &artifacts_rel,
                meta,
                config.model.as_deref(),
            )
            .await;
            // F5-fix5: `result` は success（`stop_reason: end_turn`）なのに `result.json` が無く、
            // その `result` の時点で background task がまだ走っていた＝headless の run が「通知を
            // 待つ」と言って turn を終え、CLI がその task を殺した（本番 run 01M3KF2HFMHPJR7YEB5HMT38MQ）。
            // coding 系の execute run は ADR-0072 D9 の continuation（新しい session + checkpoint）に
            // 回す（`Terminal::Yielded`。continuation の上限・進捗なしの判定はディスパッチャの既存の
            // 規則に任せる）。それ以外の run は従来どおり `result.json` 不在の失敗のまま、文言に分類名を足す。
            let missing_result_json = match &outcome {
                (Terminal::Error { message, .. }, None)
                    if message.starts_with(RESULT_JSON_MISSING_MARKER) =>
                {
                    Some(message.clone())
                }
                _ => None,
            };
            if let Some(message) = missing_result_json
                && req.context.browser.is_none()
                && !meta.is_error
                && meta.subtype == "success"
                && meta.stop_reason.as_deref().is_none_or(|r| r == "end_turn")
                && !meta.outstanding_background.is_empty()
            {
                let commands = meta.outstanding_background.join(" | ");
                if is_continuable_execute_run(&req.task, &req.context) {
                    sink.progress(&format!(
                            "{HEADLESS_BACKGROUND_TASK_CLASS}: the run ended its turn while background \
                             task(s) were still running; they were killed and no result.json was written. \
                             Handing over to a continuation run (re-run in the foreground): {commands}"
                        ));
                    outcome = (
                        Terminal::Yielded {
                            checkpoint: headless_background_checkpoint(
                                &req.artifacts_dir,
                                &meta.outstanding_background,
                            )
                            .await,
                            usage: usage_with_cost(meta.usage, config.model.as_deref()),
                        },
                        None,
                    );
                } else {
                    sink.progress(&format!(
                        "{HEADLESS_BACKGROUND_TASK_CLASS}: the run ended its turn while background \
                             task(s) were still running; they were killed: {commands}"
                    ));
                    outcome = (
                        Terminal::Error {
                            message: format!(
                                "{message} ({HEADLESS_BACKGROUND_TASK_CLASS}: background task \
                                     killed when the headless turn ended: {commands})"
                            ),
                            retryable: true,
                        },
                        None,
                    );
                }
            }
            // ADR-0054 D1（Phase 113 追記）: `result` メッセージは一度観測できたが、供給側の失敗
            // （`subtype: error_during_execution` かつ `is_error`）として終わった run は、上の
            // `(None, None)` のクラッシュ分類（resume 拒否の検出）を一切通らなかった。本番の
            // タスク 01M35X86XTK84F97QW0CN5PGMR / reviewer run 01M388BENASH3JEBWFS03KEQYT はこの
            // 穴に落ちた（stderr は `No conversation found with session ID: …` の 1 行だったが、
            // stdout の `result` の `result` フィールドにはその文言が無かったため、`result` を
            // 観測できてしまった＝ここに来て、resume 拒否として扱われなかった）。resume を頼んだ
            // run が `error_during_execution`/`is_error` で終わったときは、stderr の末尾と
            // `result` メッセージ自身の文面の両方を resume 拒否の文言と照らす。
            if is_resuming && meta.is_error && meta.subtype == "error_during_execution" {
                let tail = read_tail(&stderr_log_path, 4096).await;
                let result_text = meta.result.as_deref().unwrap_or("");
                if crate::provider::looks_like_resume_rejection(&tail)
                    || crate::provider::looks_like_resume_rejection(result_text)
                {
                    sink.session_resume_failed(&tail);
                }
            }
            outcome
        }
    };

    forward_delegate_file(&req.artifacts_dir, sink).await;

    // CLI result errors can reflect command arguments or page text even when raw capture is off.
    // Keep the failure classification and normal public summaries, but omit reflected error bodies.
    if req.context.browser.is_some() {
        match &mut terminal {
            Terminal::Error { message, .. } | Terminal::BudgetExhausted { message, .. } => {
                if message.starts_with(RESULT_JSON_MISSING_MARKER) {
                    *message = format!("{RESULT_JSON_MISSING_MARKER}browser result.json");
                } else {
                    *message = "browser harness failed".into();
                }
            }
            _ => {}
        }
    }
    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }
    // ADR-0072 E2（P-E0-2 の修正）: `result.json` そのものが無かった run（`RESULT_JSON_MISSING_MARKER`）
    // は `ProviderFailure` を持たない `Err(AdapterError)` にする。`provider_failure_outcome` は
    // これを分類できない失敗として扱い、ディスパッチャは ADR-0070 D3 の `InfraRequeue`
    // （attempts を消費しない。上限に達したときだけ `WorkerError{retryable:false}`）に倒す。
    if let Terminal::Error { message, .. } = &terminal
        && message.starts_with(RESULT_JSON_MISSING_MARKER)
    {
        return Err(AdapterError::Other(message.clone()));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// 壁時計の Unix 秒（ADR-0024 D4: 観測時刻は celeris の壁時計）。`claude_account` の確認・ログイン中継からも使う。
pub(crate) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// stream-json の 1 行を解釈する。既知でない `type` や JSON として不正な行は無視する（ADR-0006 D5）。
fn handle_line(
    line: &str,
    sink: &dyn EventSink,
    last_result: &mut Option<ResultMeta>,
    background: &mut BackgroundTasks,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let Some(ty) = value.get("type").and_then(|t| t.as_str()) else {
        return;
    };
    if ty == "rate_limit_event"
        && let Some(obs) = RateLimitObservation::from_stream_json(&value, now_unix_secs())
    {
        sink.rate_limit(obs);
    }
    match ty {
        // ADR-0048 D2（Phase 60a）: stream-json → 正規化した進行。`msg` の文面は Phase 59 までと同じ
        // （`tool_use: <名前> <入力>` / 本文そのまま）で、構造化フィールドを**足すだけ**。
        // 本文（`text`）は 1 つの assistant メッセージ分をまとめて 1 件にする。
        "assistant" => {
            if let Some(content) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                let mut texts: Vec<&str> = Vec::new();
                let flush = |texts: &mut Vec<&str>, sink: &dyn EventSink| {
                    if texts.is_empty() {
                        return;
                    }
                    let joined = texts.join("\n");
                    texts.clear();
                    let msg = truncate(&joined, 500);
                    sink.progress_with(&msg, &progress::text(&joined));
                };
                for item in content {
                    match item.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                texts.push(text);
                            }
                        }
                        Some("thinking") => {
                            flush(&mut texts, sink);
                            // 思考は**要約だけ**（本文は流さない）。要約が取れなければ何も出さない。
                            let summary = item
                                .get("thinking")
                                .or_else(|| item.get("text"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            if !summary.trim().is_empty() {
                                let fields = progress::thinking(&progress::one_line(summary));
                                let msg = format!(
                                    "thinking: {}",
                                    fields.summary.clone().unwrap_or_default()
                                );
                                sink.progress_with(&msg, &fields);
                            }
                        }
                        Some("tool_use") => {
                            flush(&mut texts, sink);
                            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            let input = item.get("input");
                            let shown = input
                                .map(|v| truncate(&v.to_string(), 200))
                                .unwrap_or_default();
                            sink.progress_with(
                                &format!("tool_use: {name} {shown}"),
                                &progress::tool_use(name, input),
                            );
                        }
                        _ => {}
                    }
                }
                flush(&mut texts, sink);
            }
        }
        // 道具の結果は `user` メッセージに `tool_result` として返る（`is_error` が失敗の印）。
        "user" => {
            if let Some(content) = value.pointer("/message/content").and_then(|c| c.as_array()) {
                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let body = tool_result_text(item);
                    let error = item
                        .get("is_error")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let fields = progress::tool_result(None, &body, error);
                    let head = fields.summary.clone().unwrap_or_default();
                    let msg = if error {
                        format!("tool_result (error): {head}")
                    } else {
                        format!("tool_result: {head}")
                    };
                    sink.progress_with(&msg, &fields);
                }
            }
        }
        "result" => {
            let subtype = value
                .get("subtype")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_error = value
                .get("is_error")
                .and_then(|b| b.as_bool())
                .unwrap_or(subtype != "success");
            // ADR-0061（Phase 104）: claude-code CLI の `result.usage` は Anthropic API と同じ形
            // （`cache_creation_input_tokens` / `cache_read_input_tokens` を含む）。`cost_usd` はここでは
            // 計算しない（model 文字列は呼び出し元でしか分からない。`terminal_from_result` が埋める）。
            let usage = value.get("usage").map(|u| Usage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
                cache_read_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()),
                cache_creation_tokens: u
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64()),
                cost_usd: None,
            });
            let result = value
                .get("result")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            let stop_reason = value
                .get("stop_reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            *last_result = Some(ResultMeta {
                subtype,
                is_error,
                usage,
                result,
                stop_reason,
                outstanding_background: background.outstanding(),
            });
        }
        // F5-fix5: background task の開始・終わり（`result` の後に届く kill の通知も含めて追う）。
        "system" => background.observe(&value),
        _ => {}
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// ADR-0048 D2: `tool_result` の本文。Claude Code は文字列の `content` と、`[{"type":"text","text":…}]`
/// の配列の両方を出す（どちらも読む）。
fn tool_result_text(item: &serde_json::Value) -> String {
    let Some(content) = item.get("content") else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(parts) = content.as_array() {
        let joined: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect();
        if !joined.is_empty() {
            return joined.join("\n");
        }
    }
    content.to_string()
}

/// `result` メッセージと結果ファイルから終端を合成する（ADR-0006 D3/D4, ADR-0010 D5）。呼び出し元は
/// `result` メッセージを一度でも観測できた場合にのみこれを呼ぶ（観測できなかった場合は
/// クラッシュとして扱い、この関数を呼ばずに `Error` にする。ADR-0006 D4）。`is_error`/`subtype != "success"`
/// のときは `result` のテキスト（無ければ `subtype`）を供給側失敗として分類する。
/// ADR-0061（Phase 104）: model が分かる時だけ静的単価表から USD を推定して埋める（不明なモデル・
/// token 欠落は `None` のまま。ここでは断定しない）。
fn usage_with_cost(usage: Option<Usage>, model: Option<&str>) -> Option<Usage> {
    usage.map(|mut u| {
        if let Some(model) = model {
            u.cost_usd = task_core::estimate_cost_usd(model, &u);
        }
        u
    })
}

/// ADR-0072 E2（P-E0-2 の修正）: `result` メッセージは観測できたのに `result.json` 自体が無いときの
/// `Terminal::Error.message` の接頭辞。`run` がこの接頭辞を見て `Err(AdapterError)`（InfraRequeue）に
/// 倒す（他の `result.json` の失敗〈壊れた JSON・必須欄の欠落〉は worker 自身の誤りのまま）。
const RESULT_JSON_MISSING_MARKER: &str = "claude exited without ";

/// F5-fix5: headless の run が background task を残して turn を終えた失敗の分類名（`Terminal` の文言・
/// 進行のメッセージに載せる。GUI の run の終わり方・進行の欄にそのまま出る）。
pub const HEADLESS_BACKGROUND_TASK_CLASS: &str = "headless_background_task";

/// F5-fix5: Claude Code CLI の background task を無効にする環境変数（2.1.283 で確認）。
const DISABLE_BACKGROUND_TASKS_ENV: &str = "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS";

/// F5-fix5: Claude Code CLI の Bash の `timeout` の上限（ミリ秒。既定 600000）。
const BASH_MAX_TIMEOUT_ENV: &str = "BASH_MAX_TIMEOUT_MS";

async fn terminal_from_result(
    artifacts_dir: &Path,
    artifacts_rel: &str,
    last_result: &ResultMeta,
    model: Option<&str>,
) -> (Terminal, Option<ProviderFailure>) {
    if last_result.is_error || last_result.subtype != "success" {
        // ADR-0072 D7（Phase E1）: `error_max_turns` は `--max-turns` の上限に当たったという
        // claude-code 自身の分類なので、字句判定なしで構造化できる（wall-clock は別経路。上の呼び出し元
        // の loop の timeout で検出する）。それ以外の `is_error`/`subtype != success` は、`result` の
        // 文言が context 超過の語彙に当たれば `BudgetExhausted{kind: Context}`（§7 U1: 実機の文言は
        // 未確認。分類できなければ従来どおり `Error`）。usage は予算切れでも運ぶ（従来は捨てていた）。
        let usage = usage_with_cost(last_result.usage, model);
        if last_result.subtype == "error_max_turns" {
            return (
                Terminal::BudgetExhausted {
                    kind: task_core::BudgetKind::Turns,
                    message: "claude result: error_max_turns".to_string(),
                    usage,
                },
                None,
            );
        }
        let text_for_classification = last_result
            .result
            .clone()
            .unwrap_or_else(|| last_result.subtype.clone());
        if task_core::looks_like_context_exceeded(&text_for_classification) {
            return (
                Terminal::BudgetExhausted {
                    kind: task_core::BudgetKind::Context,
                    message: format!(
                        "claude result: {}: {text_for_classification}",
                        last_result.subtype
                    ),
                    usage,
                },
                None,
            );
        }
        let pf = classify_provider_failure(&text_for_classification);
        let message = match &last_result.result {
            Some(result_text) => format!("claude result: {}: {result_text}", last_result.subtype),
            None => format!("claude result: {}", last_result.subtype),
        };
        return (
            Terminal::Error {
                message,
                retryable: true,
            },
            pf,
        );
    }

    let result_path = artifacts_dir.join("result.json");
    let text = match tokio::fs::read_to_string(&result_path).await {
        Ok(t) => t,
        Err(_) => {
            // ADR-0072 E2（P-E0-2 の修正）: `result` メッセージは観測できた（= claude 自身は正常に
            // 終わったと申告した）のに `result.json` そのものが無い（Phase 112 D3 / 115 D2 の
            // work_dir 側の回収を試みた後もなお無い）。これは worker 自身の判断の誤りというより、
            // 書き込みが間に合わなかった・消えたという供給側/インフラ側の事情に近い。呼び出し元
            // （`run`）はこの文言（`RESULT_JSON_MISSING_MARKER`）を見て `Err(AdapterError)`
            // （`ProviderFailure` は無し）に倒し、ADR-0070 D3 どおり attempts を消費しない
            // `InfraRequeue` の経路に乗せる（従来は `Ok(Terminal::Error{retryable:true})` になり、
            // `WorkerError{true}` として attempts を消費していた）。
            return (
                Terminal::Error {
                    message: format!("{RESULT_JSON_MISSING_MARKER}{artifacts_rel}/result.json"),
                    retryable: true,
                },
                None,
            );
        }
    };

    let terminal = match serde_json::from_str::<ResultFile>(&text) {
        Ok(rf) => {
            // ADR-0072 D9: 優先順位は `question` > `summary` > `yield`。
            if let Some(question) = rf.question {
                Terminal::Question { text: question }
            } else if let Some(summary) = rf.summary {
                Terminal::Done {
                    summary,
                    evidence: lenient_evidence(rf.evidence),
                    usage: usage_with_cost(last_result.usage, model),
                }
            } else if let Some(checkpoint) = rf.r#yield {
                Terminal::Yielded {
                    checkpoint,
                    usage: usage_with_cost(last_result.usage, model),
                }
            } else {
                Terminal::Error {
                    message: format!(
                        "{artifacts_rel}/result.json has neither 'summary', 'question' nor 'yield'"
                    ),
                    retryable: true,
                }
            }
        }
        Err(e) => Terminal::Error {
            message: format!("{artifacts_rel}/result.json is not valid JSON: {e}"),
            retryable: true,
        },
    };
    (terminal, None)
}

#[cfg(test)]
mod tests;
