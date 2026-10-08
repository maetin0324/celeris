//! `codex` アダプタ（ADR-0008 D3）。
//!
//! `codex exec --json` は celeris 独自のワーカープロトコルを話さない。`--json` が吐く JSON Lines
//! （`thread.started` → `item.*`（進捗）→ `turn.completed`/`turn.failed`）を読み、`claude-code`
//! （ADR-0006）と同じ「結果ファイル規約」（`artifacts/result.json`）で `RunOutcome` を合成する。
//! プロンプト組み立ては `claude_code::build_prompt` をそのまま再利用する（ADR-0008 D3: kind 別の
//! 文面をアダプタごとに複製しない）。生存監視（wall-clock・無出力タイムアウト・SIGTERM→SIGKILL）は
//! `subprocess.rs` の低レベル部分を再利用する。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use task_core::{RateLimitObservation, Usage};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::claude_code::{build_prompt, work_dir_note};
use crate::delegate_file::{clear_delegate_file, forward_delegate_file};
use crate::progress;
use crate::protocol::{Evidence, ProviderFailure, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, adopt_result_json_written_under_work_dir, kill_now,
    read_line_limited, read_tail, reap_after_terminal, write_result_json,
};

/// `[adapters.codex] resume_mode`（ADR-0054 D1。Phase 67）: このインストールの `codex` が
/// `exec resume <id>` サブコマンドを受け付けるかの**決定的な**判定。実機のバージョンを毎回 probe する
/// のではなく、設定で固定する（instructions: 「a config flag or a version probe done once, cached」の
/// うち前者。celeris はこのサンドボックスから実 CLI を起こせないため、実機で `codex exec resume --help`
/// 等を確認した上で運用側が設定すること。既定は `ExecResume`（新しめの codex-cli を想定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexResumeMode {
    /// `codex exec resume <id> --json …`。
    #[default]
    ExecResume,
    /// `codex exec --json -c experimental_resume=<id> …`（`resume` サブコマンドの無い古い版）。
    ExperimentalResume,
}

/// `[adapters.codex] resume_bypass`（ADR-0054 Phase 112 D1）。ADR-0095 D-b により、
/// `Dangerous` の場合も rules の効力を確認できない bypass は行わず fresh run に戻す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodexResumeBypass {
    #[default]
    Off,
    Dangerous,
}

/// `[adapters.codex]`（config.toml, ADR-0008 D4）。
#[derive(Debug, Clone)]
pub struct CodexConfig {
    /// 起動するコマンド名／パス。既定 `"codex"`。
    pub command: String,
    /// `exec --json` の後、プロンプトの前に追加する引数（サンドボックス／承認モードの指定など。
    /// 正確なフラグ名は Phase 0 の二次情報では確定していないため、運用側で指定する。ADR-0008 D3）。
    pub extra_args: Vec<String>,
    /// モデル指定（`--model`。省略時は codex の既定モデル）。
    pub model: Option<String>,
    /// ADR-0069 Phase 118 D1: `-c model_reasoning_effort="<値>"`（例 `"high"`）。省略時は付けない。
    pub reasoning_effort: Option<String>,
    /// 追加の環境変数。
    pub env: Vec<(String, String)>,
    /// ADR-0043 D3（Phase 56）: `Some` なら `codex` をコンテナの中で起こす（`container::wrap`）。
    pub container: Option<crate::container::SharedPlan>,
    /// ADR-0054 D1（Phase 67）: `context.session` が resume を求めたときの継続手段。
    pub resume_mode: CodexResumeMode,
    /// ADR-0054 Phase 112 D1: 翻訳しきれない `extra_args` の resume での扱い。
    pub resume_bypass: CodexResumeBypass,
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D2/D7: `[adapters.codex] subagents`。既定 `Deny`
    /// （`-c features.multi_agent=false` 等を `extra_args` の後ろに付ける）。
    pub subagents: crate::tool_policy::SubagentPolicy,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            command: "codex".to_string(),
            extra_args: Vec::new(),
            model: None,
            reasoning_effort: None,
            env: Vec::new(),
            container: None,
            resume_mode: CodexResumeMode::default(),
            resume_bypass: CodexResumeBypass::default(),
            subagents: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexAdapter {
    config: CodexConfig,
}

impl CodexAdapter {
    pub const ID: &'static str = "codex";

    pub fn new(config: CodexConfig) -> Self {
        Self { config }
    }
}

/// ADR-0095 D-b: execpolicy は argv の接頭辞だけを照合する。書き込み先の保護は D-a の
/// read-only bind に委ね、ここでは user manager へ直接つながるコマンドを拒否する。
const SYSTEMD_DENY_PREFIXES: &[&[&str]] = &[
    &["systemctl", "--user"],
    &["systemctl", "--user-unit"],
    &["systemd-run"],
    &["loginctl"],
    &["busctl", "--user"],
    &["dbus-send", "--session"],
];
static RULES_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn systemd_deny_rules() -> String {
    let mut rules =
        String::from("# ADR-0095 D-b: worker runs must not manage the host user systemd.\n");
    for prefix in SYSTEMD_DENY_PREFIXES {
        let pattern = serde_json::to_string(prefix).expect("static rule tokens serialize");
        rules.push_str(&format!(
            "prefix_rule(pattern={pattern}, decision=\"forbidden\", justification=\"本番 host の操作は人が実行する手順として書く（ADR-0095 付記 D-d）\")\n"
        ));
    }
    rules
}

/// Codex reads user rules from CODEX_HOME at startup. Resolve a relative CODEX_HOME against
/// the child's cwd, matching the path that the child will use; the default is ~/.codex.
fn codex_home(config: &CodexConfig, cwd: &Path) -> Result<PathBuf, AdapterError> {
    let home = config
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "CODEX_HOME")
        .map(|(_, value)| std::ffi::OsString::from(value))
        .or_else(|| std::env::var_os("CODEX_HOME"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex").into_os_string())
        })
        .ok_or_else(|| AdapterError::Other("CODEX_HOME and HOME are both unset".into()))?;
    let home = PathBuf::from(home);
    if home.as_os_str().is_empty() {
        return Err(AdapterError::Other("CODEX_HOME is empty".into()));
    }
    Ok(if home.is_absolute() {
        home
    } else {
        cwd.join(home)
    })
}

async fn install_systemd_deny_rules(
    config: &CodexConfig,
    cwd: &Path,
) -> Result<PathBuf, AdapterError> {
    if config.extra_args.iter().any(|arg| {
        arg == "--ignore-rules"
            || arg.starts_with("--ignore-rules=")
            || arg == "--dangerously-bypass-approvals-and-sandbox"
    }) {
        return Err(AdapterError::Other(
            "codex --ignore-rules and --dangerously-bypass-approvals-and-sandbox are forbidden for worker runs (ADR-0095 D-b)".into(),
        ));
    }
    let home = codex_home(config, cwd)?;
    let rules_dir = home.join("rules");
    tokio::fs::create_dir_all(&rules_dir).await?;
    // Shared account homes can serve several runs at once. Never expose a partially written
    // policy to another codex process starting during this write.
    let sequence = RULES_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = rules_dir.join(format!(
        ".celeris-deny.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        file.write_all(systemd_deny_rules().as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&temporary, rules_dir.join("celeris-deny.rules")).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result?;
    Ok(home)
}

#[async_trait]
impl WorkerAdapter for CodexAdapter {
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
        run_codex(&self.config, &req, run_id, &limits, sink).await
    }

    /// ADR-0025 D2: `extra`（`CODEX_HOME` を含む）を `config.env` の末尾に足した複製を返す
    /// （`claude_code::ClaudeCodeAdapter::with_env` と同じ規則: 同名キーは後勝ち）。
    fn with_model(&self, model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.model = Some(model.to_owned());
        Some(Arc::new(Self::new(config)))
    }
    /// ADR-0069 Phase 118 D1: codex は `-c model_reasoning_effort="<値>"` に対応する。
    fn supports_reasoning_effort(&self) -> bool {
        true
    }
    fn with_reasoning_effort(&self, effort: &str) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.reasoning_effort = Some(effort.to_owned());
        Some(Arc::new(Self::new(config)))
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(CodexAdapter::new(config)))
    }
    /// ADR-0043 D3（Phase 56）: コンテナの中で `codex` を起こす複製。
    fn with_container(&self, plan: crate::container::SharedPlan) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.container = Some(plan);
        Some(Arc::new(CodexAdapter::new(config)))
    }
}

/// 壁時計の Unix 秒（ADR-0025 D3: 観測時刻は celeris の壁時計。`codex_account` からも使う）。
pub(crate) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `artifacts/result.json`（`claude_code::ResultFile` と同じ規約。ADR-0006 D3, ADR-0008 D3）。
#[derive(Debug, Deserialize)]
struct ResultFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    evidence: serde_json::Value,
    /// ADR-0072 D9（Phase E1）: graceful yield（`claude_code::ResultFile::r#yield` と同じ規約）。
    #[serde(default, rename = "yield")]
    r#yield: Option<serde_json::Value>,
}

/// ADR-0061（Phase 104）: model が分かる時だけ静的単価表から USD を推定して埋める
/// （`claude_code::terminal_from_result` と同じやり方。不明なモデル・token 欠落は `None` のまま）。
fn with_estimated_cost(usage: Option<Usage>, model: Option<&str>) -> Option<Usage> {
    usage.map(|mut u| {
        if let Some(model) = model {
            u.cost_usd = task_core::estimate_cost_usd(model, &u);
        }
        u
    })
}

fn lenient_evidence(value: serde_json::Value) -> Vec<Evidence> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value::<Evidence>(item).ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// `translate_resume_extra_args` の結果（ADR-0054 Phase 112 D1）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResumeArgTranslation {
    /// `-c key=value` として `exec resume` の argv に足す値（`value` は既に quote 済み）。
    config_overrides: Vec<String>,
    /// 翻訳できた operator の元のフラグ表記（INFO ログ用）。
    translated: Vec<String>,
    /// 翻訳できなかった operator の元のフラグ表記（WARN で落とすか、`resume_bypass` があれば
    /// `--dangerously-bypass-approvals-and-sandbox` に置き換える対象）。
    untranslatable: Vec<String>,
}

/// `[adapters.codex] extra_args` のうち `exec resume`（ADR-0054 Phase 68c のホワイトリスト）に直接は
/// 乗せられないフラグを、可能な限り意味を保った `-c key=value` に書き換える（ADR-0054 Phase 112 D1）。
/// 純粋関数（I/O・ログをしない。呼び出し側がログを出す）。
///
/// - `--approve-for-me` → `approval_policy="never"` + `sandbox_mode="workspace-write"`
/// - `--full-auto` → `approval_policy="on-failure"` + `sandbox_mode="workspace-write"`
/// - `--sandbox <MODE>` / `-s <MODE>` → `sandbox_mode="<MODE>"`
/// - `--ask-for-approval <POLICY>` / `-a <POLICY>` → `approval_policy="<POLICY>"`
/// - それ以外（値を取るはずのフラグの値欠落を含む）は `untranslatable` に残る。
fn translate_resume_extra_args(extra_args: &[String]) -> ResumeArgTranslation {
    let mut out = ResumeArgTranslation::default();
    let mut iter = extra_args.iter().peekable();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--approve-for-me" => {
                out.config_overrides
                    .push("approval_policy=\"never\"".to_string());
                out.config_overrides
                    .push("sandbox_mode=\"workspace-write\"".to_string());
                out.translated.push(flag.clone());
            }
            "--full-auto" => {
                out.config_overrides
                    .push("approval_policy=\"on-failure\"".to_string());
                out.config_overrides
                    .push("sandbox_mode=\"workspace-write\"".to_string());
                out.translated.push(flag.clone());
            }
            "--sandbox" | "-s" => match iter.next() {
                Some(mode) => {
                    out.config_overrides
                        .push(format!("sandbox_mode=\"{mode}\""));
                    out.translated.push(format!("{flag} {mode}"));
                }
                None => out.untranslatable.push(flag.clone()),
            },
            "--ask-for-approval" | "-a" => match iter.next() {
                Some(policy) => {
                    out.config_overrides
                        .push(format!("approval_policy=\"{policy}\""));
                    out.translated.push(format!("{flag} {policy}"));
                }
                None => out.untranslatable.push(flag.clone()),
            },
            other => out.untranslatable.push(other.to_string()),
        }
    }
    out
}

/// `turn.completed`/`turn.failed` の一度でも観測できた終端シグナル（ADR-0008 D3）。
#[derive(Debug, Clone)]
enum TurnSignal {
    Completed { usage: Option<Usage> },
    Failed { message: String },
}

/// 1 回の `codex exec` 起動（fresh でも resume でも）が読み取った生の観測結果。分類
/// （`Terminal`/`ProviderFailure` への変換）は呼び出し側（`run_codex`）が行う。Phase 98（ADR-0054
/// 追記）: この構造体に分けたのは、resume の RPC 失敗を**同じ run の中で** fresh 起動としてやり直す
/// （`run_codex_once` をもう一度呼ぶ）ために、読み取りループを 1 回分だけの単位に切り出す必要があった
/// ため。
struct CodexAttempt {
    exit_status: std::process::ExitStatus,
    last_signal: Option<TurnSignal>,
    last_error_message: Option<String>,
    conversation_reply: Option<String>,
    timeout_terminal: Option<Terminal>,
    /// Phase 98: stdout に JSON Lines が 1 行でも来たか（パースの成否は問わない）。「イベントを
    /// 一つも出さずに終わった」判定に使う。
    saw_any_stdout_line: bool,
}

/// ADR-0075 R7-8: 子プロセスに渡す env（`config.env`。同名は後勝ち）の `CARGO_TARGET_DIR`。空・相対パス・
/// コンテナ実行（パスはホストのもの）なら `None`。
fn cargo_target_writable_root(config: &CodexConfig) -> Option<std::path::PathBuf> {
    if config.container.is_some() {
        return None;
    }
    let (_, value) = config
        .env
        .iter()
        .rev()
        .find(|(k, _)| k == crate::build_cache::CARGO_TARGET_DIR_VAR)?;
    let path = std::path::PathBuf::from(value);
    (!value.is_empty() && path.is_absolute()).then_some(path)
}

/// ADR-0074 Phase F5-fix4: この run が触るリポジトリの git 管理領域（`--add-dir` で足す分）。
///
/// 対象のリポジトリは request に一覧として載っていないので、ディスパッチャの配置（ADR-0043 D2 /
/// ADR-0074 D1.2）から決定的に拾う: cwd（先頭のリポジトリ、または `tree`）、cwd の親が `repos/` なら
/// その兄弟（`<task_dir>/repos/<name>` や `<task_dir>/wu/<key>/repos/<name>` の他のリポジトリ）、
/// そして `<workspace>/repos/<name>`。それぞれ `local_worktree::git_admin_dirs` にかけ、作業ツリーの
/// 最上位であるものだけの gitdir と common dir を、重複なく、見つけた順に返す。git でなければ空。
fn git_writable_roots(req: &RunRequest) -> Vec<std::path::PathBuf> {
    let cwd = req.cwd();
    let mut candidates: Vec<std::path::PathBuf> = vec![cwd.to_path_buf()];
    let mut push_children = |parent: &std::path::Path| {
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        let mut children: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            // `kind = dir` / `mode = shared` の兄弟は実体へのシンボリックリンク（ADR-0043 D2）。
            // その先は celeris の worktree ではないので、管理領域を広げない。
            .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir()))
            .collect();
        children.sort();
        candidates.extend(children);
    };
    if let Some(parent) = cwd.parent()
        && parent.file_name() == Some(std::ffi::OsStr::new(crate::task_repos::REPOS_DIR_NAME))
    {
        push_children(parent);
    }
    push_children(&req.workspace.join(crate::task_repos::REPOS_DIR_NAME));
    let mut seen_candidates: Vec<std::path::PathBuf> = Vec::new();
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for candidate in candidates {
        let key = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen_candidates.contains(&key) {
            continue;
        }
        seen_candidates.push(key);
        for dir in crate::local_worktree::git_admin_dirs(&candidate) {
            if !roots.contains(&dir) {
                roots.push(dir);
            }
        }
    }
    roots
}

/// `codex exec`（または `codex exec resume <id>`）を 1 回起動し、終了まで読み切る（ADR-0008 D3）。
/// `resume_id` が `Some` なら resume、`None` なら fresh（`--add-dir` 付き。成果物ディレクトリと、
/// `workspace-write` の run ではリポジトリの git 管理領域。Phase F5-fix4）。プロンプトは呼び出し側が
/// 一度だけ組んで渡す（Phase 98 のリトライでも同じ文面を使う。ADR-0054 D1 のとおり fresh 側の前置きは
/// 本来「差分でなく全量」だが、intra-run のやり直しでは前置きを組み直さない — 動くことを優先する）。
#[allow(clippy::too_many_arguments)]
async fn run_codex_once(
    config: &CodexConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
    prompt: &str,
    resume_id: Option<&str>,
    images: &[PathBuf],
    stdout_log_path: &std::path::Path,
    stderr_log_path: &std::path::Path,
) -> Result<CodexAttempt, AdapterError> {
    // `stderr_task` (below) moves a copy into its `async move` block; this one stays available for
    // the crash-classification read after the loop (ADR-0010 D5).
    let stderr_log_path_for_task = stderr_log_path.to_path_buf();
    let resume_id = resume_id.map(|s| s.to_string());

    // Install on every spawn, including resume and a fresh retry after a failed resume.
    let codex_home = install_systemd_deny_rules(config, req.cwd()).await?;

    let mut command = Command::new(&config.command);
    // CoS and standalone task workspaces need not be Git repositories.
    command.arg("exec");
    // ADR-0054 Phase 68b/68c: `codex exec resume` is a distinct clap subcommand from `codex exec`,
    // and does not accept every flag the parent does — production hit this twice:
    //   Phase 68b (2026-09-21 15:04 UTC, release b24bae9a796a): `--add-dir` rejected on resume.
    //   Phase 68c (2026-09-21 15:44 UTC, release a2942d5d8a94, this worktree's 68b already live):
    //     `--approve-for-me` (from `[adapters.codex] extra_args`) rejected on resume too, with a
    //     *narrower* usage line than `exec resume --help`'s OPTIONS list actually documents:
    //       error: unexpected argument '--approve-for-me' found
    //       Usage: codex exec resume --json --skip-git-repo-check --config <key=value> <SESSION_ID> [PROMPT]
    //     `~/.local/bin/codex exec resume --help` (codex-cli 0.155.1, re-checked for Phase 68c) still
    //     lists a broader OPTIONS set (-c/--config, --last, --all, --enable, --disable, -i/--image,
    //     --strict-config, -m/--model, --dangerously-bypass-approvals-and-sandbox,
    //     --dangerously-bypass-hook-trust, --worktree, --thread-source, --skip-git-repo-check,
    //     --ephemeral, --ignore-user-config, --ignore-rules, --output-schema, --json,
    //     -o/--output-last-message, -h/--help — no `--add-dir`, no `-s/--sandbox`, no
    //     `--approve-for-me`), same as it did for Phase 68b. Since the deployed binary's actual
    //     accepted set is narrower than what `--help` documents (and we cannot exec `codex exec
    //     resume` itself to verify further — only `--help` is allowed), we now treat resume as
    //     **whitelist-based** rather than "subtract the flags we know are missing": for the `resume`
    //     subcommand form we emit only `--json`, `--skip-git-repo-check`, and any number of
    //     `-c/--config <key=value>` pairs, plus the session id and prompt positionals — i.e. exactly
    //     the flags the production usage line above shows. Everything else the fresh form uses
    //     (`--add-dir`, `--model`, operator `extra_args` such as `--approve-for-me`/`--sandbox`) is
    //     either routed through an equivalent `-c key=value` (only `--model` has a documented one:
    //     `codex exec --help`'s own example is `-c model="o3"`) or dropped for resume with a comment
    //     below explaining why (the resumed thread already carries whatever approval/sandbox/
    //     writable-roots settings were set on the *first*, non-resume `exec` invocation that created
    //     it — unverified against the real CLI beyond `--help`, see agent-docs/PROGRESS.md Phase 68c 未解決事項).
    let mut is_exec_resume_subcommand = false;
    if let (Some(id), CodexResumeMode::ExecResume) = (&resume_id, config.resume_mode) {
        command.arg("resume").arg(id);
        is_exec_resume_subcommand = true;
    }
    // ADR 2026-10-05 cos-chat-home D4: native image input of a CoS chat run. `-i/--image` is on
    // both `codex exec --help` and `codex exec resume --help`. It takes several values, so it goes
    // before `--json` (the next flag ends its values; the trailing `-` prompt is never swallowed).
    for image in images {
        command.arg("--image").arg(image);
    }
    // ADR-0054 D2（Phase 68）: CoS の対話 run だけ read-only sandbox（読み取りの道具の代わり。codex には
    // claude-code の `--allowedTools` に相当する道具単位の許可リストが無いため、書き込みそのものを
    // 塞ぐ）。`--add-dir` した `artifacts_dir`（下。fresh run のみ。Phase 68b/68c 参照）は read-only でも
    // 書ける（celeris が渡す「結果ファイルを書く場所」の明示的な例外。codex の writable_roots の扱いに依る）。
    // それ以外の run は従来どおり `workspace-write`（result.json の契約に書き込みが要る）。
    // ADR 2026-10-05 cos-chat-home D2: a CoS chat run (`context.cos_chat`) is not this read-only
    // conversation; it gets the normal workspace-write tools within D3 (no ADR-0095 D-b flags).
    let sandbox_mode = if req.context.cos_chat.is_some() {
        "workspace-write"
    } else if req.context.conversation_addressee
        == Some(crate::protocol::ConversationAddressee::Secretary)
    {
        "read-only"
    } else {
        "workspace-write"
    };
    // `--json` / `--skip-git-repo-check` / `-c key=value` are on the `exec resume` whitelist above,
    // so this trio is identical for fresh and resume runs.
    command
        .arg("--json")
        .arg("--skip-git-repo-check")
        // The result.json contract needs writes; explicit extra_args override this default.
        .arg("-c")
        .arg(format!("sandbox_mode=\"{sandbox_mode}\""));
    if let (Some(id), CodexResumeMode::ExperimentalResume) = (&resume_id, config.resume_mode) {
        command.arg("-c").arg(format!("experimental_resume={id}"));
    }
    if let Some(model) = &config.model {
        if is_exec_resume_subcommand {
            // `--model` isn't on the Phase 68c whitelist; `-c model="..."` is the documented
            // equivalent (`codex exec --help`'s own `-c model="o3"` example).
            command.arg("-c").arg(format!("model=\"{model}\""));
        } else {
            command.arg("--model").arg(model);
        }
    }
    // ADR-0069 Phase 118 D1: `-c key=value` is on the `exec resume` whitelist (Phase 68c) the same
    // way `sandbox_mode`/`model` are above, so this applies identically to fresh and resume forms.
    if let Some(effort) = &config.reasoning_effort {
        command
            .arg("-c")
            .arg(format!("model_reasoning_effort=\"{effort}\""));
    }
    // Worktrees and shared workspaces keep results outside cwd. Grant only the
    // dispatcher-selected artifact directory, not its parent or other tasks (plus, since Phase
    // F5-fix4, the git admin dirs of this run's own repos; see `git_writable_roots`) — but only on the forms
    // of `codex exec` that accept `--add-dir` (see the usage-line comment above; `exec resume` does
    // not). A resumed thread already carries the writable-roots grant it received on the *first*
    // (non-resume) `exec` invocation that created it, since that one goes through plain `codex exec`
    // and does pass `--add-dir`; `-c experimental_resume=<id>` runs also go through plain `codex exec`
    // and keep getting `--add-dir` here.
    tokio::fs::create_dir_all(&req.artifacts_dir).await?;
    if is_exec_resume_subcommand {
        // ADR-0054 Phase 112 D1: operator-configured `extra_args` are arbitrary strings celeris does
        // not validate, and Phase 68c's production incident shows `exec resume` rejects at least one
        // of them (`--approve-for-me`) outright. Rather than dropping every flag wholesale (Phase
        // 68c), translate the ones with a documented `-c key=value` equivalent and only drop what's
        // left (see `translate_resume_extra_args` for the table and reasoning).
        let translation = translate_resume_extra_args(&config.extra_args);
        for pair in &translation.config_overrides {
            command.arg("-c").arg(pair);
        }
        if !translation.translated.is_empty() {
            tracing::info!(
                "run {run_id}: translated codex extra_args for `exec resume` (ADR-0054 Phase 112 \
                 D1): {:?} -> {:?}",
                translation.translated,
                translation.config_overrides
            );
        }
        if !translation.untranslatable.is_empty() {
            warn!(
                "run {run_id}: dropping codex extra_args on `exec resume` (no -c translation \
                 known; ADR-0054 Phase 112 D1): {:?}",
                translation.untranslatable
            );
        }
    } else {
        command.arg("--add-dir").arg(&req.artifacts_dir);
        // ADR-0074 Phase F5-fix4（本番障害 01M3JXB3DHVBWKWKPW04DTG6SJ）: cwd が git worktree だと、
        // `git commit` / `git merge` の書き込み先（per-worktree gitdir `…/.git/worktrees/<name>` と
        // 共通の `…/.git`）は cwd の外にあり、`workspace-write` では read-only になる（`ORIG_HEAD` で
        // `git merge main` が落ちた）。書き込みを許す run に限り、その 2 つを `--add-dir` で足す
        // （`--add-dir` は繰り返せる: codex-cli 0.157.0 で `codex exec --add-dir A --add-dir B --help`
        // は exit 0、単一値の `--model x --model y --help` は "cannot be used multiple times" で exit 2）。
        // read-only の CoS run には足さない（読み取り専用の意味を変えない）。resume は上のとおり
        // `--add-dir` を受け付けないので、最初の fresh `exec` で与えたこの許可を引き継ぐ前提のまま。
        if sandbox_mode == "workspace-write" {
            for dir in git_writable_roots(req) {
                command.arg("--add-dir").arg(dir);
            }
            // ADR-0075 R7-8（本番 run 01M3VCWE54P73CPFG09ZSW6Q6M）: run の `CARGO_TARGET_DIR`（scratch の
            // `<scratch>/targets/<owner>/target` など。cwd の外）も書ける場所として足す。codex は存在しない
            // root を書けるようにしない（実測）ので先に作る。作れなければ足さない（cargo が同じ誤りを出す）。
            if let Some(target) = cargo_target_writable_root(config) {
                match tokio::fs::create_dir_all(&target).await {
                    Ok(()) => {
                        command.arg("--add-dir").arg(&target);
                    }
                    Err(e) => warn!(
                        "run {run_id}: could not create CARGO_TARGET_DIR {} for the codex sandbox \
                         (ADR-0075 R7-8); not adding it as a writable root: {e}",
                        target.display()
                    ),
                }
            }
        }
        command.args(&config.extra_args);
    }
    // F5-fix10（本番障害 run 01M3Q21Z9JQWWANGHXJPNH1F8X。claude-code と同じ原因）: プロンプトは argv に
    // 載せず stdin で渡す。位置引数に `-` を置く（codex-cli の `codex exec --help`: "If not provided as an
    // argument (or if `-` is used), instructions are read from stdin"、`codex exec resume --help`:
    // "[PROMPT] … If `-` is used, read from stdin"。resume は SESSION_ID の後なので `-` を明示する）。
    // ADR 2026-10-07-worker-no-subagents-no-llm-cli D2: multi-agent（`spawn_agent` 系の道具）を既定で無効にする。
    // `-c key=value` は fresh / `exec resume` の両形で受け付ける（ADR-0054 Phase 68c）。`extra_args` の**後ろ**に
    // 置くので運用側の値では外れない。外せるのは `[adapters.codex] subagents = "allow" | "allow_cos"` だけ（D7）。
    if !config.subagents.allows(req) {
        for kv in crate::tool_policy::CODEX_FEATURE_OVERRIDES {
            command.arg("-c").arg(kv);
        }
    }
    // ADR 2026-10-08-cos-chat-prompt-cache D6: a CoS chat run's fixed Core goes in as codex's
    // developer instructions (a developer message ahead of AGENTS.md and the environment context,
    // the same bytes on `exec` and `exec resume`); only the run specific part is on stdin.
    if let Some(chat) = req.context.cos_chat.as_ref() {
        command
            .arg("-c")
            .arg(cos_chat_developer_instructions(&crate::cos_chat::core(
                chat,
            )));
    }
    command.arg("-");
    command
        .envs(config.env.iter().cloned())
        .env("CODEX_HOME", codex_home)
        .current_dir(req.cwd());
    // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
    let mut command = crate::db_guard::launch(command, config.container.as_deref());
    // F5-fix10: stdin はプロンプトを渡すためだけに開き、書き終えたら閉じる（コンテナは `-i` 付き）。
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    crate::subprocess::check_arg_lengths(CodexAdapter::ID, &command)?;

    let mut child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());
    // F5-fix10: プロンプトを stdin に流して閉じる（別タスク。下の stdout 読み取りと並行に進む）。
    let stdin_writer =
        crate::subprocess::feed_stdin(&mut child, prompt.to_string(), CodexAdapter::ID, run_id)?;

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

    let mut stdout_file = tokio::fs::File::create(stdout_log_path).await?;
    let mut reader = BufReader::new(stdout);

    let start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_signal: Option<TurnSignal> = None;
    let mut conversation_reply: Option<String> = None;
    // `{"type":"error","message":...}` を観測したら保持する（ADR-0010 D5: `turn.*` を一度も観測できずに
    // exit した場合の分類材料に使う）。
    let mut last_error_message: Option<String> = None;
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;
    // Phase 98: 少なくとも 1 行の stdout（イベント）を見たか。
    let mut saw_any_stdout_line = false;

    loop {
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            // ADR-0072 D7/§6 (i)（Phase E1）: wall-clock の打ち切りは予算切れ（continuation の対象）。
            // codex には turn の上限が無いので wall-clock だけがこの経路。
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
            // ADR-0072 D7: idle timeout は E1 では harness_error に分類変更しない（§7 U7）。
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
            Err(_elapsed) => continue,
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                sink.heartbeat();
                last_activity = Instant::now();
                saw_any_stdout_line = true;
                warn!("run {run_id}: discarding overlong line from codex stdout");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                saw_any_stdout_line = true;
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        if event["type"] == "item.completed"
                            && event["item"]["type"] == "agent_message"
                            && event["item"]["phase"] != "commentary"
                        {
                            conversation_reply = event["item"]["text"]
                                .as_str()
                                .filter(|text| !text.trim().is_empty())
                                .map(str::to_owned);
                        } else if event["type"] == "item.started" {
                            conversation_reply = None;
                        }
                    }
                    handle_line(
                        trimmed,
                        sink,
                        &mut last_signal,
                        &mut last_error_message,
                        req.context.cos_chat.is_some(),
                    );
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

    Ok(CodexAttempt {
        exit_status,
        last_signal,
        last_error_message,
        conversation_reply,
        timeout_terminal,
        saw_any_stdout_line,
    })
}

async fn run_codex(
    config: &CodexConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;
    crate::routing_context_transport::record(&run_dir, req, false).await;

    // 前回の run（リトライ）が残した結果ファイルを、今回の run の結果と誤読しない（ADR-0006 D3 と同じ理由）。
    // ADR-0036 D1/D2: 置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_rel = req.artifacts_rel();
    let result_path = req.artifact_path("result.json");
    let _ = tokio::fs::remove_file(&result_path).await;
    clear_delegate_file(&req.artifacts_dir).await;
    // ADR-0006 Phase 115 D2: 同じ理由で、work_dir 側の名残候補も消す（`work_dir != workspace` の
    // ときだけ意味がある。無ければ何もしない）。
    if let Some(work_dir) = req.work_dir.as_deref()
        && work_dir != req.workspace
    {
        let _ = tokio::fs::remove_file(work_dir.join("artifacts").join("result.json")).await;
    }

    let cos_parts =
        crate::claude_code::build_cos_chat_parts(&req.task, &req.context, run_id, &artifacts_rel);
    let work_note = work_dir_note(req.work_dir.as_deref(), &req.workspace, &req.artifacts_dir);
    let prompt = match &cos_parts {
        Some(parts) => format!("{work_note}{}", parts.variable),
        None => format!(
            "{work_note}{}",
            build_prompt(&req.task, &req.context, run_id, &artifacts_rel)
        ),
    };
    // ADR-0023 D2 / M1: この run で何を渡したかを残す（`request.json` は構造、`prompt.txt` は実際の文面）。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    let recorded = match &cos_parts {
        Some(parts) => {
            crate::cos_chat::prompt_record("-c developer_instructions", &parts.core, &prompt)
        }
        None => prompt.clone(),
    };
    crate::subprocess::write_run_prompt(&run_dir, &recorded, run_id).await;
    // ADR-0127 D1/D3: mount された skill のディレクトリを `.agents/skills/<name>/` に丸写しし（codex が
    // ネイティブに読む作業場所内の場所。account 共有の CODEX_HOME には書かない）、`AGENTS.md` の
    // celeris:skills 節は名前・説明・パスの一覧にする（本文は埋め込まない。run は落とさない）。
    if let Err(e) = crate::skills::deliver_agent_skills(req.cwd(), &req.context.skills).await {
        warn!("run {run_id}: failed to deliver skills to .agents/skills: {e}");
    }
    if let Err(e) = crate::skills::deliver_agents_md(req.cwd(), &req.context.skills).await {
        warn!("run {run_id}: failed to update AGENTS.md with skills: {e}");
    }

    // ADR-0054 D1（Phase 67）: `context.session` がこのアダプタ宛て（`adapter == "codex"`）で
    // `resume: true` のときだけ継続する。手段は `config.resume_mode` で決定的に選ぶ（実機 probe はしない）。
    let codex_session = req
        .context
        .session
        .as_ref()
        .filter(|s| s.adapter == CodexAdapter::ID);
    let resume_id = codex_session
        .filter(|s| s.resume)
        .map(|s| s.session_id.clone());
    // ADR-0054 Phase 112 D2: if this would go through the `exec resume` whitelist (Phase 68c) and
    // the operator's `extra_args` have flags D1 can't translate to `-c key=value`, start fresh.
    // ADR-0095 D-b also disables the formerly allowed dangerous bypass: its effect on execpolicy
    // cannot be verified here, so no resumed process may receive that flag.
    let resume_id = if resume_id.is_some() && config.resume_mode == CodexResumeMode::ExecResume {
        let translation = translate_resume_extra_args(&config.extra_args);
        if !translation.untranslatable.is_empty() {
            warn!(
                "run {run_id}: skipping `exec resume` and starting a fresh codex session instead \
                 (extra_args {:?} have no -c translation for `exec resume`; ADR-0095 D-b)",
                translation.untranslatable
            );
            None
        } else {
            resume_id
        }
    } else {
        resume_id
    };
    let cos_chat = req.context.cos_chat.as_ref();
    // ADR 2026-10-05 cos-chat-home D2: a CoS chat run whose resume cannot be guaranteed starts an
    // explicit fresh session (the prompt carries the DB history) and says why in a status.
    let resume_id = match cos_chat {
        Some(_) if codex_session_requested(req) => match (resume_id, config.resume_mode) {
            (Some(id), CodexResumeMode::ExecResume) => Some(id),
            (Some(_), CodexResumeMode::ExperimentalResume) => {
                cos_chat_explicit_fresh(
                    sink,
                    "codex resume_mode = experimental cannot guarantee `codex exec resume`",
                );
                None
            }
            (None, _) => {
                cos_chat_explicit_fresh(
                    sink,
                    "extra_args have no -c translation for `codex exec resume`; ADR-0095 D-b",
                );
                None
            }
        },
        _ => resume_id,
    };
    let images = match cos_chat {
        Some(chat) => cos_chat_images(chat, &run_dir, sink).await,
        None => Vec::new(),
    };

    let stdout_log_path = run_dir.join("stdout.jsonl");
    let mut stderr_log_path = run_dir.join("stderr.log");
    let mut attempt = run_codex_once(
        config,
        req,
        run_id,
        limits,
        sink,
        &prompt,
        resume_id.as_deref(),
        &images,
        &stdout_log_path,
        &stderr_log_path,
    )
    .await?;

    // Phase 98（ADR-0054 追記。実機障害 2026-09-22 00:18 UTC、task 01M337NT3QT1FR1G6WHS9G6NDA）:
    // `codex exec resume` が「イベントを一つも出さずに」非 0 で終わり、stderr に
    // `thread/resume`/`-32601`/`resume` を含む失敗行があるなら、これは通常の resume 拒否
    // （`looks_like_resume_rejection` が既に検出する `session not found` 等）ではなく、codex-cli 自身が
    // `exec resume` の JSON-RPC を実装していない（`list_turns is not supported yet`）という、**同じ
    // インストールでは毎回起きる**壊れ方。次 run を待たずに、**この run の中で** fresh セッション
    // （resume 無しの通常の `codex exec`）として 1 回だけやり直す（claude-code 側の Phase 67b の
    // 自己修復と同じ「壊れたら retire して仕切り直す」考え方を、run をまたがずに行うもの）。
    let mut resume_id_for_classification = resume_id.clone();
    if resume_id.is_some()
        && !attempt.saw_any_stdout_line
        && attempt.timeout_terminal.is_none()
        && attempt.last_signal.is_none()
        && !attempt.exit_status.success()
    {
        let tail = read_tail(&stderr_log_path, 4096).await;
        let combined = format!(
            "{} {tail}",
            attempt.last_error_message.as_deref().unwrap_or("")
        );
        if crate::provider::looks_like_resume_rpc_failure(&combined) {
            sink.session_resume_failed(&combined);
            let retry_stdout_log_path = run_dir.join("stdout.resume_retry.jsonl");
            let retry_stderr_log_path = run_dir.join("stderr.resume_retry.log");
            let _ = tokio::fs::remove_file(&result_path).await;
            clear_delegate_file(&req.artifacts_dir).await;
            attempt = run_codex_once(
                config,
                req,
                run_id,
                limits,
                sink,
                &prompt,
                None,
                &images,
                &retry_stdout_log_path,
                &retry_stderr_log_path,
            )
            .await?;
            stderr_log_path = retry_stderr_log_path;
            resume_id_for_classification = None;
        }
    }

    let CodexAttempt {
        exit_status,
        last_signal,
        last_error_message,
        conversation_reply,
        timeout_terminal,
        saw_any_stdout_line: _,
    } = attempt;
    let resume_id = resume_id_for_classification;

    let (terminal, provider_failure): (Terminal, Option<ProviderFailure>) = match (
        timeout_terminal,
        &last_signal,
    ) {
        // タイムアウト（wall-clock / idle）は分類しない（ADR-0010 D5）。
        (Some(t), _) => (t, None),
        // `turn.completed`/`turn.failed` を一度も観測できずに exit した場合はクラッシュとして扱い、
        // artifacts/result.json を一切信用しない（ADR-0006 D4 と同じ理由。ADR-0008 D3）。
        // 観測できた `{"type":"error",...}` のメッセージ、無ければ stderr.log の末尾を分類する（ADR-0010 D5）。
        (None, None) => {
            let exit_repr = match exit_status.code() {
                Some(code) => code.to_string(),
                None => "signal".to_string(),
            };
            let mut pf = last_error_message
                .as_deref()
                .and_then(classify_provider_failure);
            let tail = read_tail(&stderr_log_path, 4096).await;
            if pf.is_none() {
                pf = classify_provider_failure(&tail);
            }
            // ADR-0054 D1（Phase 67）: resume を頼んだ run が、セッションを拒否されたように見える crash
            // なら報告する。文言は実機で確認していない（`provider::looks_like_resume_rejection` 参照）。
            if resume_id.is_some() {
                let combined = format!("{} {tail}", last_error_message.as_deref().unwrap_or(""));
                if crate::provider::looks_like_resume_rejection(&combined) {
                    sink.session_resume_failed(&combined);
                }
            }
            (
                Terminal::Error {
                    message: format!(
                        "worker exited without a turn.completed/turn.failed message (exit={exit_repr})"
                    ),
                    retryable: true,
                },
                pf,
            )
        }
        (None, Some(TurnSignal::Failed { message })) => {
            if resume_id.is_some() && crate::provider::looks_like_resume_rejection(message) {
                sink.session_resume_failed(message);
            }
            // ADR-0072 D7/§6 (i)（Phase E1）: `turn.failed` の文言が context 超過の語彙に当たれば
            // `BudgetExhausted{kind: Context}`（実機の文言は未確認。§7 U1。分類できなければ従来どおり）。
            if task_core::looks_like_context_exceeded(message) {
                (
                    Terminal::BudgetExhausted {
                        kind: task_core::BudgetKind::Context,
                        message: format!("codex turn failed: {message}"),
                        usage: None,
                    },
                    None,
                )
            } else {
                let pf = classify_provider_failure(message);
                (
                    Terminal::Error {
                        message: format!("codex turn failed: {message}"),
                        retryable: true,
                    },
                    pf,
                )
            }
        }
        (None, Some(TurnSignal::Completed { usage })) => {
            // ADR-0006 Phase 115 D2: 結果ファイルを読む（`result_path` の有無を見る）前に、work_dir 側の
            // 名残を採用する。Phase 112 D3 の「最終メッセージから回収」より必ず先（`result_path` の
            // `NotFound` 判定がこの後にあるため、先に採用しておかないと本物の結果を無視して最終メッセージ
            // から再構成してしまう）。
            adopt_result_json_written_under_work_dir(
                &req.artifacts_dir,
                req.work_dir.as_deref(),
                &req.workspace,
                run_id,
            )
            .await;
            // A conversation can answer directly; work orders still require their artifacts.
            // A CoS chat run (transient task without `conversation`) also replies in the chat.
            let terminal = if exit_status.success()
                && (req.task.conversation.is_some() || cos_chat.is_some())
                && req.task.kind == task_core::TaskKind::Execute
                && matches!(tokio::fs::metadata(&result_path).await,
                    Err(ref error) if error.kind() == std::io::ErrorKind::NotFound)
                && let Some(summary) = conversation_reply
            {
                // ADR-0054 Phase 112 D3: the model may have meant `summary` to be the whole
                // `result.json` body (e.g. it could not write into `artifacts_dir`, as happens when
                // `exec resume` drops the `--add-dir` grant it would otherwise need). If it parses
                // as that shape (a non-empty `summary` string and an `actions` array), recover it:
                // write it to `result.json` as if the model had written it itself, and re-derive the
                // terminal from disk through the normal path (`terminal_from_result`) so
                // `actions`/`memory`/`milestone_proposal` all pick it up via the existing,
                // disk-reading machinery instead of the raw JSON text being treated as inert prose.
                // A CoS chat run must not use `result.actions`; its reply stays plain text.
                if cos_chat.is_none()
                    && crate::result_report::final_message_is_recoverable_result(&summary)
                {
                    match tokio::fs::write(&result_path, &summary).await {
                        Ok(()) => {
                            crate::progress::emit_status(
                                sink,
                                "result recovered from the final message (result.json was \
                                 missing; ADR-0054 Phase 112 D3)",
                            );
                            terminal_from_result(
                                &req.artifacts_dir,
                                &artifacts_rel,
                                *usage,
                                config.model.as_deref(),
                            )
                            .await
                        }
                        Err(e) => {
                            warn!(
                                "run {run_id}: failed to write the result recovered from the \
                                 final message: {e}"
                            );
                            Terminal::Done {
                                summary,
                                evidence: Vec::new(),
                                usage: with_estimated_cost(*usage, config.model.as_deref()),
                            }
                        }
                    }
                } else {
                    Terminal::Done {
                        summary,
                        evidence: Vec::new(),
                        usage: with_estimated_cost(*usage, config.model.as_deref()),
                    }
                }
            } else {
                terminal_from_result(
                    &req.artifacts_dir,
                    &artifacts_rel,
                    *usage,
                    config.model.as_deref(),
                )
                .await
            };
            (terminal, None)
        }
    };

    forward_delegate_file(&req.artifacts_dir, sink).await;

    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// CoS chat: did the dispatcher ask this adapter to resume a session?
fn codex_session_requested(req: &RunRequest) -> bool {
    req.context
        .session
        .as_ref()
        .is_some_and(|s| s.adapter == CodexAdapter::ID && s.resume)
}

/// ADR 2026-10-08-cos-chat-prompt-cache D6: `developer_instructions=<TOML string>` for `-c`. The
/// value is a TOML string so codex parses it as one string whatever the Core contains.
pub fn cos_chat_developer_instructions(core: &str) -> String {
    format!(
        "developer_instructions={}",
        toml::Value::String(core.to_string())
    )
}

/// The explicit fresh start of a CoS chat run: the prompt already carries the summary and the
/// unsummarized DB history (`cos_chat::build_prompt`); the status names the reason.
fn cos_chat_explicit_fresh(sink: &dyn EventSink, why: &str) {
    crate::progress::emit_status(
        sink,
        &format!(
            "{} ({why})",
            crate::cos_chat::capability_reason(crate::cos_chat::MissingCapability::Continuation)
        ),
    );
}

/// ADR 2026-10-05 cos-chat-home D4: the `--image` paths of a CoS chat run. The route is the same
/// `image_delivery` judgement the prompt shows (`actual=`): native → `--image`, path+tool → only
/// the path in the prompt, unsupported → a status saying the image was not inspected. codex splits
/// `--image` values on `,`, so a path containing one is passed as a copy under the run dir.
async fn cos_chat_images(
    chat: &crate::protocol::CosChatContext,
    run_dir: &Path,
    sink: &dyn EventSink,
) -> Vec<PathBuf> {
    use crate::cos_chat::{ImageDelivery, image_delivery, image_delivery_reason};
    let mut images = Vec::new();
    for (index, attachment) in chat.attachments.iter().enumerate() {
        let delivery = image_delivery(attachment.delivery, chat.harness_capabilities.as_ref());
        match delivery {
            ImageDelivery::Native => {
                if !attachment.path.to_string_lossy().contains(',') {
                    images.push(attachment.path.clone());
                    continue;
                }
                let ext = attachment
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .filter(|e| !e.contains(','))
                    .unwrap_or("img");
                let dir = run_dir.join("cos-images");
                let copy = dir.join(format!("{index}.{ext}"));
                let copied = async {
                    tokio::fs::create_dir_all(&dir).await?;
                    tokio::fs::copy(&attachment.path, &copy).await
                }
                .await;
                match copied {
                    Ok(_) => images.push(copy),
                    Err(e) => crate::progress::emit_status(
                        sink,
                        &format!(
                            "attachment {}: {} (could not stage for --image: {e})",
                            attachment.id,
                            crate::cos_chat::capability_reason(
                                crate::cos_chat::MissingCapability::Image
                            )
                        ),
                    ),
                }
            }
            ImageDelivery::PathAndTool | ImageDelivery::Unsupported => {
                if let Some(reason) = image_delivery_reason(delivery) {
                    crate::progress::emit_status(
                        sink,
                        &format!("attachment {}: {reason}", attachment.id),
                    );
                }
            }
            ImageDelivery::FilePath => {}
        }
    }
    images
}

/// codex の JSON Lines の 1 行を解釈する。既知でない `type` や JSON として不正な行は無視する
/// （`claude_code::handle_line` と同じ方針。ADR-0008 D3）。
fn handle_line(
    line: &str,
    sink: &dyn EventSink,
    last_signal: &mut Option<TurnSignal>,
    last_error_message: &mut Option<String>,
    cos_chat: bool,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let Some(ty) = value.get("type").and_then(|t| t.as_str()) else {
        return;
    };
    if ty == "token_count"
        && let Some(obs) = RateLimitObservation::from_codex_token_count(&value, now_unix_secs())
    {
        sink.rate_limit(obs);
    }
    if ty.starts_with("item.") {
        // ADR-0048 D2（Phase 60a）: codex の `item.*` を正規化する。`msg` は従来どおり行そのもの
        // （500 バイトで切る）で、構造化フィールドを**足すだけ**。
        let fields = item_progress(ty, value.get("item"), cos_chat);
        sink.progress_with(&truncate(line, 500), &fields);
        // ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: 始まった道具（`command_execution` の `command` など）
        // から別 LLM CLI / API の起動と subagent 道具を検出する。
        if ty != "item.completed"
            && let Some(item) = value.get("item")
        {
            let item_type = item
                .get("type")
                .or_else(|| item.get("item_type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if let Some(v) = crate::tool_policy::inspect_tool_use(item_type, Some(item)) {
                crate::claude_code::report_policy_violation(sink, &v);
            }
        }
        return;
    }
    match ty {
        // ADR-0054 D1（Phase 67）/ Phase 67c 追記: 新規セッション（`context.session.resume == false`）で
        // codex 自身が割り当てた thread/session id を報告する。**フィールド名は実機で確認していない**
        // （`thread.started` イベント自体は ADR-0008 の実機確認で観測済みだが、その本文の形は未確認）。
        // `codex exec resume <id>` は celeris が作った id ではなく codex 自身が報告した id しか受け付け
        // ないので、ここで読み損なうと（フィールド名が実機と違う等）その run は id を持たないまま
        // 終わる。celeris 側は空文字を「まだ確定していない」として扱い（
        // `crate::sessions::session_id_is_valid_for_adapter`。Phase 67c）、次の run を resume させずに
        // 新規セッションとして仕切り直すので、誤って読めなくても run 自体は失敗しない。`thread_id` /
        // `threadId` / `session_id`（`session_configured` 系の実装がこの名前を使うことがある）の
        // どれかで受け取る。
        "thread.started" | "session_configured" => {
            if let Some(id) = value
                .get("thread_id")
                .or_else(|| value.get("threadId"))
                .or_else(|| value.get("session_id"))
                .and_then(|v| v.as_str())
            {
                sink.session_established(id);
            }
        }
        "turn.completed" => {
            // codex exec --json の turn.completed.usage は cached_input_tokens を公開する。
            // 古い stream で欄が無ければ None のままにする。
            let usage = value.get("usage").map(|u| Usage {
                input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()),
                output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()),
                cache_read_tokens: u.get("cached_input_tokens").and_then(|v| v.as_u64()),
                cache_creation_tokens: u.get("cache_write_input_tokens").and_then(|v| v.as_u64()),
                cost_usd: None,
                duplicate_reads: None,
                session_resumed: None,
                context_tokens: None,
            });
            *last_signal = Some(TurnSignal::Completed { usage });
        }
        "error" => {
            if let Some(m) = value.get("message").and_then(|m| m.as_str()) {
                *last_error_message = Some(m.to_string());
            }
        }
        "turn.failed" => {
            // 実機（codex-cli 0.154.0）では `error` はオブジェクト（`{"message":"..."}`）で返る。
            // 将来のバージョンで文字列に変わっても読めるよう両方を受け付ける。
            let message = value
                .get("error")
                .map(describe_error)
                .unwrap_or_else(|| "turn.failed".to_string());
            *last_signal = Some(TurnSignal::Failed { message });
        }
        _ => {}
    }
}

/// ADR-0048 D2（Phase 60a）: codex の `item.*` イベント → 正規化した進行（ここだけがアダプタ固有）。
///
/// - 道具（`command_execution` / `mcp_tool_call` / `web_search` / `file_change` / `patch_apply`）は
///   `item.started` / `item.updated` が `tool_use`、`item.completed` が `tool_result`
///   （`exit_code != 0` は `error`）。
/// - `agent_message` は `text`、`reasoning` は `thinking`（要約だけ）。
/// - それ以外（`todo_list` や知らない item）は `status`（節目）。
///
/// `cos_chat`: a CoS chat run streams the whole `agent_message` (newlines kept) as its text delta.
fn item_progress(
    ty: &str,
    item: Option<&serde_json::Value>,
    cos_chat: bool,
) -> task_core::ProgressFields {
    let Some(item) = item else {
        return progress::status();
    };
    // 実機（codex-cli 0.154）は `type`、古い版・別実装は `item_type` を使う。
    let item_type = item
        .get("type")
        .or_else(|| item.get("item_type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let completed = ty == "item.completed";
    match item_type {
        "agent_message" => {
            let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if cos_chat {
                progress::text(text)
            } else {
                progress::text(&progress::one_line(text))
            }
        }
        "reasoning" => {
            let text = item
                .get("summary")
                .or_else(|| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            progress::thinking(&progress::one_line(text))
        }
        "command_execution" | "mcp_tool_call" | "web_search" | "file_change" | "patch_apply" => {
            if completed {
                let body = item
                    .get("aggregated_output")
                    .or_else(|| item.get("output"))
                    .and_then(|o| o.as_str())
                    .unwrap_or("");
                let error = item
                    .get("exit_code")
                    .and_then(|c| c.as_i64())
                    .is_some_and(|c| c != 0)
                    || item.get("status").and_then(|s| s.as_str()) == Some("failed");
                progress::tool_result(Some(item_type), body, error)
            } else {
                let summary = item
                    .get("command")
                    .or_else(|| item.get("query"))
                    .or_else(|| item.get("path"))
                    .or_else(|| item.get("tool"))
                    .and_then(|v| v.as_str())
                    .map(progress::one_line)
                    .unwrap_or_else(|| progress::one_line(&item.to_string()));
                task_core::ProgressFields::of(task_core::ProgressKind::ToolUse)
                    .with_tool(item_type)
                    .with_summary(progress::truncate_chars(
                        &summary,
                        progress::SUMMARY_MAX_CHARS,
                    ))
                    .with_detail(item.to_string())
            }
        }
        // 知らない item は節目として残す（Console は折り畳んだ見出しに最後の `status` を出す）。
        other => {
            let summary = if other.is_empty() {
                ty.to_string()
            } else {
                format!("{ty} {other}")
            };
            progress::status().with_summary(summary)
        }
    }
}

/// `turn.failed` の `error` フィールドから人間向けの文字列を作る（文字列でもオブジェクトでも読める）。
fn describe_error(value: &serde_json::Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    if let Some(msg) = value.get("message").and_then(|m| m.as_str()) {
        return msg.to_string();
    }
    value.to_string()
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

/// `turn.completed` と結果ファイルから終端を合成する。呼び出し元は `turn.completed` を一度でも
/// 観測できた場合にのみこれを呼ぶ（`claude_code::terminal_from_result` と同じ構造。ADR-0008 D3）。
async fn terminal_from_result(
    artifacts_dir: &std::path::Path,
    artifacts_rel: &str,
    usage: Option<Usage>,
    model: Option<&str>,
) -> Terminal {
    let usage = with_estimated_cost(usage, model);
    let result_path = artifacts_dir.join("result.json");
    let text = match tokio::fs::read_to_string(&result_path).await {
        Ok(t) => t,
        Err(_) => {
            return Terminal::Error {
                message: format!("codex exited without {artifacts_rel}/result.json"),
                retryable: true,
            };
        }
    };

    // ADR-0090 D1: クラスタ job の終了待ち（`question` が無ければ `summary` より優先）。
    if let Some(terminal) = crate::adapter::result_file_wait(&text, usage) {
        return terminal;
    }
    match serde_json::from_str::<ResultFile>(&text) {
        Ok(rf) => {
            // ADR-0072 D9: 優先順位は `question` > `summary` > `yield`。
            if let Some(question) = rf.question {
                Terminal::Question { text: question }
            } else if let Some(summary) = rf.summary {
                Terminal::Done {
                    summary,
                    evidence: lenient_evidence(rf.evidence),
                    usage,
                }
            } else if let Some(checkpoint) = rf.r#yield {
                Terminal::Yielded { checkpoint, usage }
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
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cos_chat_tests;
