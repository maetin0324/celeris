//! `celerisctl curation validate` — ADR-0131 付記 D12: 日次知識整理の worker が書いた
//! `curation-plan.json` を、daemon（`celeris::knowledge_curation::check_plan`）と同じ検証で事前に点検する。
//! DB を開かず、ネットワークも使わない（ファイルだけを読む）。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::json;
use task_ops::knowledge_curation::{self as curation, Action, MAX_INBOX_CANDIDATES_PER_RUN};

use crate::error::CliError;
use crate::outln;

/// 作業場所の根を cwd から上へ探す段数（cwd を含む）。
const MAX_WALK_UP: usize = 6;

#[derive(Subcommand, Debug)]
pub enum CurationCommand {
    /// `curation-plan.json` を KB の写しに対して検証する（daemon と同じ規則・同じ文言）。
    Validate(ValidateArgs),
}

#[derive(Args, Debug, Clone, Default)]
pub struct ValidateArgs {
    /// `curation-plan.json` の path。省略時は cwd から上へ `artifacts/curation-plan.json` を探す。
    pub plan: Option<PathBuf>,
    /// KB の写し（`inputs/kb`）。省略時は作業場所の `inputs/kb`。
    pub kb: Option<PathBuf>,
    /// worker の `curation.diff`。省略時は作業場所の `artifacts/curation.diff` があれば使う。
    #[arg(long)]
    pub diff: Option<PathBuf>,
    /// daemon が書いた `inbox.json`。省略時は作業場所の `inputs/inbox.json` があれば使う。
    #[arg(long)]
    pub inbox: Option<PathBuf>,
    /// diff の照合をしない。
    #[arg(long)]
    pub no_diff: bool,
    /// 結果を JSON 1 行で出す。
    #[arg(long)]
    pub json: bool,
}

/// 引数と cwd から決めた、読むファイルの path。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub plan: PathBuf,
    pub kb: PathBuf,
    pub diff: Option<PathBuf>,
    pub inbox: Option<PathBuf>,
}

/// 検証に通った計画の件数と、読んだ path。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub merge: usize,
    pub new: usize,
    pub delete: usize,
    pub keep: usize,
    pub fix: usize,
    pub inbox: usize,
    pub human_decisions: usize,
    pub inbox_candidates: usize,
    pub plan_path: String,
    pub kb_path: String,
    pub diff_path: Option<String>,
    pub inbox_path: Option<String>,
}

fn is_workspace_root(dir: &Path) -> bool {
    dir.join("artifacts/curation-plan.json").is_file() && dir.join("inputs/kb").is_dir()
}

/// 読むファイルを決める（process の cwd は変えず、`cwd` を引数で受ける）。
pub fn resolve(args: &ValidateArgs, cwd: &Path) -> Result<Resolved, String> {
    let (plan, root) = match &args.plan {
        Some(plan) => {
            // `<root>/artifacts/curation-plan.json` の形を想定する。
            let root = plan.parent().and_then(Path::parent).map(Path::to_path_buf);
            (plan.clone(), root)
        }
        None => {
            let root = cwd
                .ancestors()
                .take(MAX_WALK_UP)
                .find(|dir| is_workspace_root(dir))
                .ok_or_else(|| {
                    "curation-plan.json が見つからない（作業場所の artifacts/curation-plan.json と inputs/kb を探した。PLAN と KB を引数で渡す）".to_string()
                })?
                .to_path_buf();
            (root.join("artifacts/curation-plan.json"), Some(root))
        }
    };
    let kb = match (&args.kb, &root) {
        (Some(kb), _) => kb.clone(),
        (None, Some(root)) => root.join("inputs/kb"),
        (None, None) => {
            return Err(format!(
                "KB の写しを決められない（{} の 2 つ上に作業場所が無い。KB を引数で渡す）",
                plan.display()
            ));
        }
    };
    let diff = if args.no_diff {
        None
    } else {
        args.diff.clone().or_else(|| {
            root.as_ref()
                .map(|r| r.join("artifacts/curation.diff"))
                .filter(|p| p.is_file())
        })
    };
    let inbox = args.inbox.clone().or_else(|| {
        root.as_ref()
            .map(|r| r.join("inputs/inbox.json"))
            .filter(|p| p.is_file())
    });
    Ok(Resolved {
        plan,
        kb,
        diff,
        inbox,
    })
}

/// daemon の `check_plan` と同じ順に検証する（計画の形 → KB 操作と受信箱の提案 → diff の path 集合）。
pub fn validate_paths(resolved: &Resolved) -> Result<Summary, String> {
    let raw = std::fs::read_to_string(&resolved.plan)
        .map_err(|e| format!("curation-plan.json を読めない: {e}"))?;
    let plan = curation::parse_plan(&raw)?;
    let known = match &resolved.inbox {
        Some(path) => {
            let raw =
                std::fs::read_to_string(path).map_err(|e| format!("inbox.json を読めない: {e}"))?;
            curation::known_task_ids_from_inbox_json(&raw)?
        }
        None => Default::default(),
    };
    // daemon と同じく、入力の task id が 1 つも無いときに受信箱の提案があれば拒否する。
    let validated = if !known.is_empty() {
        curation::validate_with_inbox(&resolved.kb, &plan, Some(&known))?
    } else if plan.inbox.is_empty() {
        curation::validate(&resolved.kb, &plan)?
    } else {
        return Err("入力に無い task への受信箱の提案がある".into());
    };
    if let Some(path) = &resolved.diff {
        let diff =
            std::fs::read_to_string(path).map_err(|e| format!("curation.diff を読めない: {e}"))?;
        curation::check_diff_matches(&validated, &diff)?;
    }
    let mut summary = Summary {
        inbox: plan.inbox.len(),
        human_decisions: validated.human_decisions.len(),
        inbox_candidates: plan
            .kb
            .iter()
            .filter(|item| item.path.starts_with("_inbox/"))
            .count(),
        plan_path: resolved.plan.display().to_string(),
        kb_path: resolved.kb.display().to_string(),
        diff_path: resolved.diff.as_ref().map(|p| p.display().to_string()),
        inbox_path: resolved.inbox.as_ref().map(|p| p.display().to_string()),
        ..Default::default()
    };
    for item in &validated.kb {
        match item.action {
            Action::Merge => summary.merge += 1,
            Action::New => summary.new += 1,
            Action::Delete => summary.delete += 1,
            Action::Keep => summary.keep += 1,
            Action::Fix => summary.fix += 1,
        }
    }
    Ok(summary)
}

fn validate_command(args: &ValidateArgs, cwd: &Path) -> ExitCode {
    let summary = match resolve(args, cwd).and_then(|r| validate_paths(&r)) {
        Ok(summary) => summary,
        Err(reason) => {
            eprintln!("error: {reason}");
            return ExitCode::from(1);
        }
    };
    if args.json {
        let value = json!({
            "ok": true,
            "plan": summary.plan_path,
            "kb": summary.kb_path,
            "diff": summary.diff_path,
            "inbox": summary.inbox_path,
            "counts": {
                "merge": summary.merge,
                "new": summary.new,
                "delete": summary.delete,
                "keep": summary.keep,
                "fix": summary.fix,
                "inbox": summary.inbox,
                "human_decisions": summary.human_decisions,
                "inbox_candidates": summary.inbox_candidates,
            },
        });
        outln!("{value}");
    } else {
        outln!(
            "ok: kb merge={} new={} delete={} keep={} fix={}, inbox={}, human_decisions={}, _inbox candidates={}/{}",
            summary.merge,
            summary.new,
            summary.delete,
            summary.keep,
            summary.fix,
            summary.inbox,
            summary.human_decisions,
            summary.inbox_candidates,
            MAX_INBOX_CANDIDATES_PER_RUN
        );
    }
    ExitCode::SUCCESS
}

/// 検証の失敗は `CliError` にせず、理由を stderr に出して exit code 1 を返す。
pub fn run(command: CurationCommand) -> Result<ExitCode, CliError> {
    match command {
        CurationCommand::Validate(args) => {
            let cwd = std::env::current_dir()
                .map_err(|e| CliError::msg(format!("cwd を読めない: {e}")))?;
            Ok(validate_command(&args, &cwd))
        }
    }
}

#[cfg(test)]
#[path = "curation_tests.rs"]
mod tests;
