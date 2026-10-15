//! `celerisctl campaign export <campaign-id> --out <dir>` — 判断・実行・成果・消費ログの campaign 単位 export
//! （ADR 2026-10-15-decision-log-export D5〜D8）。
//!
//! 集計と record 列の組み立ては `task_core::decision_log_export::build_campaign_export`（純関数）。ここは
//! 引数・読み出し・出力ファイルの書き込み・秘密の除去だけを行う。DB は読むだけ。
//! campaign が未実装の間は、campaign id に行へ記録された `campaign_id`（root task id など）をそのまま渡す。
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use serde_json::Value;
use task_core::SqliteStore;
use task_core::decision_log_export::ExportParams;

use crate::error::CliError;
use crate::outln;

#[derive(Subcommand, Debug)]
pub enum CampaignCommand {
    /// `records.jsonl` と `summary.json` を `--out` に書く（同じ入力なら byte 一致）。
    Export(CampaignExportArgs),
}

#[derive(Args, Debug)]
pub struct CampaignExportArgs {
    /// campaign id（campaign 未実装の間は行の `campaign_id` に記録した task / root id）。
    pub campaign_id: String,
    /// 出力ディレクトリ（無ければ作る）。
    #[arg(long)]
    pub out: PathBuf,
    /// 版情報として summary に載せる manifest の hash。
    #[arg(long)]
    pub manifest_hash: Option<String>,
}

pub fn run(store: &SqliteStore, command: CampaignCommand) -> Result<ExitCode, CliError> {
    match command {
        CampaignCommand::Export(args) => export(store, args),
    }
}

const SECRET_PREFIXES: [&str; 8] = [
    "sk-",
    "ghp_",
    "gho_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
    "eyJ",
];
const SECRET_KEYS: [&str; 6] = [
    "api_key",
    "apikey",
    "token",
    "secret",
    "password",
    "authorization",
];
const REDACTED: &str = "[redacted]";

/// 自由文の欄に混ざった秘密らしい値（既知の接頭辞の長い token、`key=value` の値、Bearer）を伏せる。
pub fn scrub_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bearer = false;
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let lower = word.to_ascii_lowercase();
        let secret_assign = word.split_once(['=', ':']).filter(|(k, v)| {
            !v.is_empty()
                && SECRET_KEYS
                    .iter()
                    .any(|sk| k.to_ascii_lowercase().ends_with(sk))
        });
        if bearer {
            out.push_str(REDACTED);
            bearer = false;
        } else if let Some((k, _)) = secret_assign {
            let sep = if word.contains('=') { '=' } else { ':' };
            out.push_str(&format!("{k}{sep}{REDACTED}"));
        } else if word.len() >= 16 && SECRET_PREFIXES.iter().any(|p| word.starts_with(p)) {
            out.push_str(REDACTED);
        } else {
            out.push_str(word);
            bearer = lower == "bearer";
        }
    }
    out
}

fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = scrub_str(s),
        Value::Array(a) => a.iter_mut().for_each(scrub_value),
        Value::Object(m) => m.values_mut().for_each(scrub_value),
        _ => {}
    }
}

fn export(store: &SqliteStore, args: CampaignExportArgs) -> Result<ExitCode, CliError> {
    let params = ExportParams {
        campaign_id: args.campaign_id.clone(),
        manifest_hash: args.manifest_hash,
        export_cursor: None,
    };
    let mut result = store.decision_log_export_campaign(&params)?;
    result.records.iter_mut().for_each(scrub_value);
    let mut summary = serde_json::to_value(&result.summary)
        .map_err(|e| CliError::msg(format!("summary: {e}")))?;
    scrub_value(&mut summary);
    let mut summary_text = serde_json::to_string_pretty(&summary)
        .map_err(|e| CliError::msg(format!("summary: {e}")))?;
    summary_text.push('\n');

    fs::create_dir_all(&args.out)
        .map_err(|e| CliError::msg(format!("create {}: {e}", args.out.display())))?;
    let write = |name: &str, body: &str| {
        let p = args.out.join(name);
        fs::write(&p, body).map_err(|e| CliError::msg(format!("write {}: {e}", p.display())))
    };
    write("records.jsonl", &result.records_jsonl())?;
    write("summary.json", &summary_text)?;
    outln!(
        "exported campaign {}: {} records -> {}",
        args.campaign_id,
        result.records.len(),
        args.out.display()
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_hides_secret_shapes_and_keeps_plain_text() {
        assert_eq!(scrub_str("timeout after 30s"), "timeout after 30s");
        assert_eq!(
            scrub_str("failed with sk-abcdefghijklmnop1234 here"),
            "failed with [redacted] here"
        );
        assert_eq!(scrub_str("api_key=abc123"), "api_key=[redacted]");
        assert_eq!(scrub_str("Bearer abc.def"), "Bearer [redacted]");
    }
}
