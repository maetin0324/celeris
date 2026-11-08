//! ADR 2026-11-07-health-review-integration D3.5・D4.1・D5.1・D5.4・D6.3・D7.2 と付記『二度目の final review の
//! 差し戻しへの方針』(a): reviewer prompt に health の材料（`ReviewRequest.health`）と設計 review の材料
//! （`ReviewRequest.design`）を出し、`review.json` に構造化 verdict（`ReviewOutput.health`・`.design`）を書かせる。
//!
//! - `health_section` は `ReviewRequest.health` が `Some` のときだけ節を出す。`None`（WU の review・review 方針の
//!   無い repo・通常の小さい変更）では空文字列。
//! - `design_section` は `ReviewRequest.design` が `Some`（採用前の設計 review の run）のときだけ節を出す。
//! - `result_section` は reviewer prompt の Result 節を組む唯一の関数。設計 review では `design` の形、final review
//!   では `verdicts`（任意の `repair` を含む）の形と、health があれば `health` の形を 1 節で出す。例の JSON は
//!   `ReviewOutput` を serde で書き出し、placeholder を使わない。
//! - finding 種別は `HealthFindingKind::ALL` から列挙し、局所修復を許す種別は `allows_local_repair` から選ぶ。
//!   種別の意味は汎用規約（`GENERIC_RULES`）の各項目の括弧内の名前で指し、説明文をここに写さない。
//!
//! ここは純粋関数だけで、I/O も LLM も無い。

use std::collections::BTreeMap;

use task_core::design_review::{ReviewPolicy, RiskAssessment};
use task_core::execution_plan::DesignBrief;

use super::GENERIC_RULES;
use crate::protocol::{
    DesignVerdictOut, DiffStat, HealthFindingKind, HealthFindingOut, HealthFindingsSummary,
    HealthPolicyState, HealthRepoInput, HealthReviewInput, HealthSeverity, HealthVerdictOut,
    OwnerRow, PolicyVerdictOut, ReviewOutput, ReviewRepairHint, ReviewRequest, ReviewVerdictOut,
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

/// 付記 (a) A1（D3.5 2.）: 設計 review の節の見出し。
pub const DESIGN_HEADING: &str = "## 設計 review（採用前の計画）";

/// `risk.reasons` のうち、review 方針を読めなかった理由の語の頭（`task_core::design_review::assess_risk`）。
const POLICY_UNAVAILABLE_PREFIX: &str = "review_policy_unavailable:";

/// 付記 (a) A1: 設計 review の節。`ReviewRequest.design` が `Some` のときだけ出し、ほかは空文字列。
/// 対象は採用前の計画で、計画の要約・設計 brief の全欄・risk・review 方針（読めなければその理由）と判定の観点を出す。
pub fn design_section(review: Option<&ReviewRequest>) -> String {
    let Some(design) = review.and_then(|r| r.design.as_deref()) else {
        return String::new();
    };
    let mut out = format!(
        "{DESIGN_HEADING}\n\
         この run の review の対象は実装ではなく、planner が作った採用前の計画（下の要約と設計 brief）である。\
         作業ツリーはまだこの計画の変更を含まない。作業ツリーは読み取り専用で読んでよい（owner 地図と既存の実装の\
         確認に使う）が、file を変えない。受け入れ条件ごとの判定はしない。計画を上の「{generic}」と下の brief・\
         risk・review 方針に照らして判定し、結果を下の Result 節の `design` に書く。\n\n\
         ### 計画の要約\n{summary}\n\n",
        generic = generic_heading(),
        summary = design.plan_summary.trim_end(),
    );
    out.push_str(&brief_lines(design.brief.as_ref()));
    out.push_str(&risk_lines(&design.risk));
    out.push_str(&review_policy_lines(
        design.review_policy.as_ref(),
        &design.risk,
    ));
    out.push_str(&design_viewpoint_lines());
    out
}

fn brief_lines(brief: Option<&DesignBrief>) -> String {
    let Some(brief) = brief else {
        return "### 設計 brief\n\
                brief 無し（この計画は設計 brief を持たない）。計画の要約から原因・owner・撤去対象を読み取って判定する。\n\n"
            .to_string();
    };
    let mut out = String::from("### 設計 brief（planner が書いたもの）\n");
    out.push_str(&text_field("root_cause", &brief.root_cause));
    out.push_str(&text_field("owner", &brief.owner));
    out.push_str(&list_field("reuse", &brief.reuse));
    if brief.options.is_empty() {
        out.push_str("- options: 無し\n");
    } else {
        out.push_str("- options:\n");
        for option in &brief.options {
            out.push_str(&format!(
                "  - `{}`（chosen={}）: {}\n",
                option.name, option.chosen, option.reason
            ));
        }
    }
    for (name, items) in [
        ("new_state", &brief.new_state),
        ("new_branch", &brief.new_branch),
        ("new_fallback", &brief.new_fallback),
        ("new_port", &brief.new_port),
        ("removals", &brief.removals),
        ("invariants", &brief.invariants),
    ] {
        out.push_str(&list_field(name, items));
    }
    out.push('\n');
    out
}

fn text_field(name: &str, value: &str) -> String {
    if value.trim().is_empty() {
        format!("- {name}: 無し\n")
    } else {
        format!("- {name}: {}\n", value.trim_end())
    }
}

fn list_field(name: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!("- {name}: 無し\n");
    }
    let mut out = format!("- {name}:\n");
    for item in items {
        out.push_str(&format!("  - {item}\n"));
    }
    out
}

fn risk_lines(risk: &RiskAssessment) -> String {
    let level = serde_json::to_value(risk.level)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut out = format!("### risk（dispatcher の決定的な判定）\n- level: `{level}`\n");
    if risk.reasons.is_empty() {
        out.push_str("- reasons: 無し\n");
    } else {
        out.push_str("- reasons:\n");
        for reason in &risk.reasons {
            out.push_str(&format!("  - `{reason}`\n"));
        }
    }
    out.push('\n');
    out
}

fn review_policy_lines(policy: Option<&ReviewPolicy>, risk: &RiskAssessment) -> String {
    let mut out =
        String::from("### この repo の review 方針（target 側の profile の `[review]` 節）\n");
    let Some(policy) = policy else {
        let reasons: Vec<String> = risk
            .reasons
            .iter()
            .filter(|r| r.starts_with(POLICY_UNAVAILABLE_PREFIX))
            .map(|r| format!("`{r}`"))
            .collect();
        out.push_str("この repo の review 方針を読めなかった。");
        if reasons.is_empty() {
            out.push_str("読めなかった理由は risk の reasons に無い。\n");
        } else {
            out.push_str(&format!("理由: {}。\n", reasons.join("、")));
        }
        out.push_str(&format!(
            "owner 地図・高 risk の path class・保護する不変条件の一覧は無い。方針の代わりに、上の「{}」と\
             設計 brief だけで判定する。\n\n",
            generic_heading()
        ));
        return out;
    };
    match &policy.owner_map {
        Some(path) => out.push_str(&format!(
            "- owner 地図: `{path}`（計画が触る path の owner 行をこの file で引く）\n"
        )),
        None => out.push_str("- owner 地図: 無し\n"),
    }
    if policy.high_risk.is_empty() {
        out.push_str("- 高 risk の path class: 無し\n");
    } else {
        out.push_str("- 高 risk の path class（paths は repo 相対 path に当てる正規表現）:\n");
        for class in &policy.high_risk {
            let paths: Vec<String> = class.paths.iter().map(|p| format!("`{p}`")).collect();
            out.push_str(&format!("  - `{}`: {}\n", class.class, paths.join("、")));
        }
    }
    out.push_str(&invariant_lines(&policy.invariants));
    out.push('\n');
    out
}

fn design_viewpoint_lines() -> String {
    format!(
        "### 判定の観点\n\
         - 上の「{generic}」の規約に照らして brief と計画を見る。末尾の括弧に名前のある {count} 項目（{kinds}）が、\
         Result 節の `design.findings[].kind` になる。\n\
         - brief が宣言した `new_state`・`new_branch`・`new_fallback`・`new_port` と `removals` が、計画の触る \
         path class と保護する不変条件に対して足りているかを見る。宣言の無い新しい状態・分岐・fallback・外への口を\
         計画が足すなら、それも finding にする。\n\
         - `root_cause` を `owner` で直す計画かを見る。owner 地図があれば、計画の write path の owner 行を作業ツリーで\
         引いて確かめる。\n\n",
        generic = generic_heading(),
        count = HealthFindingKind::ALL.len(),
        kinds = quoted(HealthFindingKind::ALL.iter().copied()),
    )
}

/// D5.4・付記 (a) A2: reviewer prompt の Result 節を組む唯一の関数。設計 review（`design` が `Some`）は
/// `verdicts` を空にして `design` を書く形、final review は `verdicts`（任意の `repair` を含む）の形と、
/// health があれば `health` の形を出す。例の JSON は `ReviewOutput` を serde で書き出したもので、そのまま読める（A3）。
pub fn result_section(review: Option<&ReviewRequest>, artifacts: &str) -> String {
    let mut out = format!(
        "## Result\n\
         Write your verdicts to `{artifacts}/review.json` (create the `{artifacts}/` directory if it does not \
         exist yet) as a single JSON object. 次の行の JSON は形の例で、そのまま JSON として読める。値は自分の\
         判定に置き換える。\n"
    );
    if review.is_some_and(|r| r.design.is_some()) {
        out.push_str(&format!("`{}`\n", example_json(&design_example())));
        out.push_str(&design_verdict_lines());
        return out;
    }
    let health = review.and_then(|r| r.health.as_ref());
    let criterion = review
        .and_then(|r| r.criteria.first().copied())
        .unwrap_or(0);
    out.push_str(&format!(
        "`{}`\n",
        example_json(&final_example(criterion, health))
    ));
    out.push_str(
        "- `verdicts` には上の \"Criteria you must judge in this run\" の criterion ごとにちょうど 1 件を書く。\
         `criterion` はその index（整数）、`pass` は `true` か `false`、`reason` は判定の理由。\n\
         - `repair` は任意で、`pass` が `false` のときだけ意味を持つ。計画を作り直さずに直せる局所的な不合格なら \
         `{\"scope\":\"local\",\"class\":…,\"hint\":…}` を書く。`class` は `format`・`lint`・`test`・`doc`・`other` の\
         どれか、`hint` は直し方の手掛かり。局所的に直せない不合格と合格の verdict では `repair` を省く。\n",
    );
    match health {
        Some(health) => out.push_str(&health_verdict_lines(health)),
        None => out.push('\n'),
    }
    out
}

/// 例の値。種別は `HealthFindingKind` から選び、source に名前を書かない。
const EXAMPLE_PATH: &str = "src/example.rs";

fn example_json(output: &ReviewOutput) -> String {
    serde_json::to_string(output).unwrap_or_default()
}

fn design_example() -> ReviewOutput {
    ReviewOutput {
        verdicts: Vec::new(),
        health: None,
        design: Some(DesignVerdictOut {
            pass: false,
            findings: vec![HealthFindingOut {
                kind: HealthFindingKind::ALL[0],
                severity: HealthSeverity::Blocking,
                path: Some(EXAMPLE_PATH.to_string()),
                reason: "brief と計画のどこが規約に反するか".to_string(),
                repair: None,
            }],
        }),
    }
}

fn final_example(criterion: usize, health: Option<&HealthReviewInput>) -> ReviewOutput {
    let local_repair = || ReviewRepairHint {
        scope: "local".to_string(),
        class: "test".to_string(),
        hint: Some("直し方の手掛かり".to_string()),
    };
    let health = health.map(|health| {
        let kind = HealthFindingKind::ALL
            .iter()
            .copied()
            .find(|k| k.allows_local_repair())
            .unwrap_or(HealthFindingKind::ALL[0]);
        HealthVerdictOut {
            findings: vec![HealthFindingOut {
                kind,
                severity: HealthSeverity::Blocking,
                path: Some(EXAMPLE_PATH.to_string()),
                reason: "差分のどこが規約に反するか".to_string(),
                repair: Some(local_repair()),
            }],
            policy: health
                .repos
                .iter()
                .any(|r| r.policy.changed)
                .then(|| PolicyVerdictOut {
                    approved: true,
                    reason: "policy の変更を認める理由".to_string(),
                }),
        }
    });
    ReviewOutput {
        verdicts: vec![ReviewVerdictOut {
            criterion,
            pass: false,
            reason: "判定の理由（確かめたコマンドと出力の要点）".to_string(),
            repair: Some(local_repair()),
        }],
        health,
        design: None,
    }
}

/// 種別の一覧の行（health と design で共通）。
fn kind_line() -> String {
    format!(
        "- `kind` は次の {count} 種のどれか: {kinds}。各種別の意味は、上の「{generic}」で末尾の括弧に同じ名前を\
         書いた項目を見る。\n",
        count = HealthFindingKind::ALL.len(),
        kinds = quoted(HealthFindingKind::ALL.iter().copied()),
        generic = generic_heading(),
    )
}

/// 付記 (a) A2: 設計 review の `design` の書き方。`repair` は書かせない（局所修復は無い）。
fn design_verdict_lines() -> String {
    let mut out = String::from(
        "- この run は設計 review なので、`verdicts` は空の配列 `[]` のまま書く（判定する受け入れ条件が無い）。\
         判定は `design` に書く。\n",
    );
    out.push_str(&kind_line());
    out.push_str(
        "- `severity` は `blocking`（1 件でもあれば計画は不合格）か `advisory`（記録と planner への申し送りだけに\
         使う）。blocking が 1 件でもあれば `\"pass\":false`、無ければ `\"pass\":true` と書く。\n\
         - `path` は任意。計画が触る repo 相対 path を書き、無ければ欄ごと省く。\n\
         - `repair` は書かない。計画の不合格は planner が計画を作り直して直し、局所修復は無い。\n\
         - 違反が無ければ `\"pass\":true,\"findings\":[]` と書く。`design` は省かない。\n\
         - `design` が無い review.json と、`kind`・`severity` に上の値以外を書いた review.json は、判定を読めない\
         不合格に数える。\n\n",
    );
    out
}

/// D5.1・D6.3: `review.json` の `health` の書き方。種別は型から列挙する。
fn health_verdict_lines(health: &HealthReviewInput) -> String {
    let local = quoted(
        HealthFindingKind::ALL
            .iter()
            .copied()
            .filter(|k| k.allows_local_repair()),
    );
    let mut out = format!(
        "この review には上の「{heading}」が付いているので、同じ object に `verdicts` と並べて `health` も書く\
         （上の例の `health`）:\n",
        heading = HEALTH_HEADING.trim_start_matches('#').trim(),
    );
    out.push_str(&kind_line());
    out.push_str(&format!(
        "- `severity` は `blocking`（合否に使う。1 件でもあれば review は不合格）か `advisory`（記録と次の run への\
         申し送りだけに使う）。\n\
         - `path` は任意。repo 相対 path を書き、無ければ欄ごと省く。\n\
         - `repair` は任意。`\"scope\":\"local\"` を書けるのは {local} の blocking だけで、その finding は局所修復に回る。\
         ほかの種別の blocking と、`repair` の無い blocking は計画の作り直し（replan）になる。\n\
         - 違反が無ければ `\"findings\":[]` と書く。`health` は省かない。\n"
    ));
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
