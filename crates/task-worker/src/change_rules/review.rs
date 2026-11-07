//! ADR 2026-11-07-health-review-integration D4.1・D5.1・D5.4・D6.3: final review の prompt に health の材料
//! （`ReviewRequest.health`）を出し、`review.json` に構造化 health verdict（`ReviewOutput.health`）を書かせる。
//!
//! - `health_section` は `ReviewRequest.health` が `Some` のときだけ節を出す。`None`（WU の review・review 方針の
//!   無い repo・通常の小さい変更）では空文字列で、reviewer prompt は変わらない。
//! - `result_section` は reviewer prompt の Result 節を組む唯一の関数。health が無ければ従来の文面だけを出し、
//!   あれば同じ節に `health` の形を足す。
//! - finding 種別は `HealthFindingKind::ALL` から列挙し、局所修復を許す種別は `allows_local_repair` から選ぶ。
//!   種別の意味は汎用規約（`GENERIC_RULES`）の各項目の括弧内の名前で指し、説明文をここに写さない。
//!
//! ここは純粋関数だけで、I/O も LLM も無い。

use std::collections::BTreeMap;

use super::GENERIC_RULES;
use crate::protocol::{
    DiffStat, HealthFindingKind, HealthFindingsSummary, HealthPolicyState, HealthRepoInput,
    HealthReviewInput, OwnerRow, ReviewRequest,
};

/// health の材料の節の見出し（Result 節はこの見出しの名前で材料を指す）。
pub const HEALTH_HEADING: &str = "## Repo health の材料（celeris が作った決定的な材料）";

/// D4.1: health の材料の節。`ReviewRequest.health` が `None` なら空文字列。
pub fn health_section(review: &ReviewRequest) -> String {
    let Some(health) = &review.health else {
        return String::new();
    };
    let mut out = format!(
        "{HEALTH_HEADING}\n\
         この review には、celeris が review 前の同期の後に target（base）→ candidate の差分から決定的に作った\
         健全性の材料が付いている（LLM は使っていない）。上の「{generic}」を candidate の差分に当て、\
         違反を下の Result 節の `health` に書く。材料は手掛かりで、省略した path・行・finding は \
         `git diff <target_sha>..<reviewed_sha>` を自分で読んで確かめる。\n",
        generic = generic_heading(),
    );
    if health.repos.is_empty() {
        out.push_str("（repo ごとの材料は無い）\n");
    }
    for repo in &health.repos {
        out.push_str(&repo_block(repo));
    }
    out.push('\n');
    out
}

fn repo_block(repo: &HealthRepoInput) -> String {
    let mut out = format!(
        "### repo `{}`\n\
         - target_sha: `{}`（review 前の同期で決めた base）\n\
         - reviewed_sha: `{}`（review する candidate）\n",
        repo.repo_id, repo.target_sha, repo.reviewed_sha
    );
    out.push_str(&diff_lines(&repo.diff));
    out.push_str(&owner_lines(&repo.owner_rows));
    out.push_str(&findings_lines(&repo.findings));
    out.push_str(&policy_lines(&repo.policy));
    out.push_str(&invariant_lines(&repo.invariants));
    out
}

fn diff_lines(diff: &DiffStat) -> String {
    let mut out = format!(
        "- diff: {} file、+{} 行・-{} 行\n",
        diff.files, diff.insertions, diff.deletions
    );
    for path in &diff.paths {
        out.push_str(&format!(
            "  - `{}`（+{}・-{}）\n",
            path.path, path.insertions, path.deletions
        ));
    }
    let omitted = diff.files.saturating_sub(diff.paths.len() as u64);
    if omitted > 0 {
        out.push_str(&format!("  - ほか {omitted} file は上限で省略\n"));
    }
    out
}

fn owner_lines(rows: &[OwnerRow]) -> String {
    if rows.is_empty() {
        return "- owner 地図の行: 無し（変更 path を含む行が base 側の地図に無い）\n".to_string();
    }
    let mut out = String::from("- owner 地図の行（base 側。変更 path を含む行）:\n");
    for row in rows {
        out.push_str(&format!("  - `{}`: {}\n", row.path, row.row));
    }
    out
}

fn findings_lines(findings: &HealthFindingsSummary) -> String {
    let counts = &findings.counts;
    let mut out = format!(
        "- repo health の report: verdict=`{}`、reviewable={}、件数 findings={}・fail_grade={}・new={}・\
         worsened={}・existing={}\n",
        findings.verdict,
        findings.reviewable,
        counts.findings,
        counts.fail_grade,
        counts.new,
        counts.worsened,
        counts.existing
    );
    if !findings.reviewable {
        out.push_str(
            "  - この report は reviewable=false なので材料にしない。差分を自分で読んで判定する。\n",
        );
    }
    if findings.new.is_empty() {
        out.push_str("  - new・worsened の finding（例外の無いもの）: 無し\n");
    } else {
        out.push_str("  - new・worsened の finding（例外の無いもの）:\n");
        for finding in &findings.new {
            let path = finding
                .path
                .as_deref()
                .map(|p| format!(" path=`{p}`"))
                .unwrap_or_default();
            out.push_str(&format!(
                "    - `{}` severity=`{}` delta=`{}`{path}\n",
                finding.rule_id, finding.severity, finding.delta
            ));
        }
    }
    if findings.truncated > 0 {
        out.push_str(&format!(
            "  - 上限で省略した finding: {} 件（上の一覧に無い）\n",
            findings.truncated
        ));
    }
    out
}

fn policy_lines(policy: &HealthPolicyState) -> String {
    let mut out = format!(
        "- policy: source=`{}`、changed={}",
        policy.source, policy.changed
    );
    if !policy.files.is_empty() {
        let files: Vec<String> = policy.files.iter().map(|f| format!("`{f}`")).collect();
        out.push_str(&format!("、変わった policy の file: {}", files.join("、")));
    }
    out.push('\n');
    if policy.changed {
        out.push_str(
            "  - この candidate は健全性の policy の file を変えた。Result 節の `health.policy` を必ず書く。\n",
        );
    }
    out
}

fn invariant_lines(invariants: &BTreeMap<String, String>) -> String {
    if invariants.is_empty() {
        return "- 保護する不変条件: 無し\n".to_string();
    }
    let mut out = String::from("- 保護する不変条件（名前: 根拠の file）:\n");
    for (name, source) in invariants {
        out.push_str(&format!("  - `{name}`: `{source}`\n"));
    }
    out
}

/// D5.4: reviewer prompt の Result 節。`review.health` が無ければ従来の文面だけを返す。
pub fn result_section(review: Option<&ReviewRequest>, artifacts: &str) -> String {
    let mut out = format!(
        "## Result\n\
         Write your verdicts to `{artifacts}/review.json` (create the `{artifacts}/` directory if it does \
         not exist yet) as a single JSON object of exactly this shape: \
         `{{\"verdicts\":[{{\"criterion\":<index>,\"pass\":<bool>,\"reason\":\"...\"}}]}}`. You must write \
         exactly one verdict for each criterion listed under \"Criteria you must judge in this run\" \
         above.\n\n"
    );
    if let Some(health) = review.and_then(|r| r.health.as_ref()) {
        out.push_str(&health_verdict_lines(health));
    }
    out
}

/// D5.1・D6.3: `review.json` の `health` の書き方。種別は型から列挙する。
fn health_verdict_lines(health: &HealthReviewInput) -> String {
    let kinds = quoted(HealthFindingKind::ALL.iter().copied());
    let local = quoted(
        HealthFindingKind::ALL
            .iter()
            .copied()
            .filter(|k| k.allows_local_repair()),
    );
    let mut out = format!(
        "この review には上の「{heading}」が付いているので、同じ object に `verdicts` と並べて `health` も書く:\n\
         `{{\"verdicts\":[...],\"health\":{{\"findings\":[{{\"kind\":\"<種別>\",\"severity\":\"blocking|advisory\",\
         \"path\":\"<repo 相対 path。無ければ欄ごと省く>\",\"reason\":\"...\",\
         \"repair\":{{\"scope\":\"local\",\"class\":\"format|lint|test|doc|other\",\"hint\":\"...\"}}}}],\
         \"policy\":{{\"approved\":<bool>,\"reason\":\"...\"}}}}}}`\n\
         - `kind` は次の {count} 種のどれか: {kinds}。各種別の意味は、上の「{generic}」で末尾の括弧に同じ名前を\
         書いた項目を見る。\n\
         - `severity` は `blocking`（合否に使う。1 件でもあれば review は不合格）か `advisory`（記録と次の run への\
         申し送りだけに使う）。\n\
         - `repair` は任意。`\"scope\":\"local\"` を書けるのは {local} の blocking だけで、その finding は局所修復に回る。\
         ほかの種別の blocking と、`repair` の無い blocking は計画の作り直し（replan）になる。\n\
         - 違反が無ければ `\"findings\":[]` と書く。`health` は省かない。\n",
        heading = HEALTH_HEADING.trim_start_matches('#').trim(),
        count = HealthFindingKind::ALL.len(),
        generic = generic_heading(),
    );
    let changed: Vec<String> = health
        .repos
        .iter()
        .filter(|r| r.policy.changed)
        .map(|r| format!("`{}`", r.repo_id))
        .collect();
    if changed.is_empty() {
        out.push_str("- `policy` は省く（policy の file を変えた repo が無い）。\n");
    } else {
        out.push_str(&format!(
            "- policy の file を変えた repo（{}）があるので `health.policy` は必須。変更を認めるなら \
             `\"approved\":true`、認めないなら `false` と書き、`reason` に理由を書く。`approved: false` は不合格。\n",
            changed.join("、")
        ));
    }
    out.push_str(
        "- `health` が無い review.json と、`kind`・`severity` に上の値以外を書いた review.json は、\
         全 criterion が不合格になる。\n\n",
    );
    out
}

fn quoted(kinds: impl Iterator<Item = HealthFindingKind>) -> String {
    kinds
        .map(|k| format!("`{}`", k.as_str()))
        .collect::<Vec<_>>()
        .join("、")
}

/// 汎用規約の見出しの名前（generic.md の 1 行目から `#` を除いたもの）。
fn generic_heading() -> &'static str {
    GENERIC_RULES
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim()
}

#[cfg(test)]
mod tests;
