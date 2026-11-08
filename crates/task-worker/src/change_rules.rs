//! ADR 2026-11-07-health-review-integration D1.1〜D1.5・D3.4: 変更の規約を prompt に配る。
//!
//! - 汎用規約の正本は `change_rules/generic.md` の 1 file（`GENERIC_RULES`）。repo に依らない文だけを持ち、
//!   repo 固有の必須規則（作業場所の `CLAUDE.md`。`crate::required_rules`）とは互いに写さない（D1.5）。
//! - 組み立て口は `claude_code::prompt` の `prompt_header` だけ（D1.2）。全 run kind（execute・plan・replan・
//!   execution plan・review）がそこを通るので、汎用規約はどの harness の prompt にも 1 回だけ入る。adapter
//!   （codex・pi・acp・aider・claude_code）と CoS chat（`build_cos_chat_parts`）では足さない（D1.3）。
//! - repo 固有の規約（`repo_section`）は `RunContext.workspace` の review 方針（付記 B1・B4。dispatcher が worker と
//!   planner の run にだけ base 側から読んで入れる）から、汎用規約の直後に出す。無ければ何も出さない。
//! - `design_brief_section` は /3 の planner に設計 brief（`ExecutionPlanSpec.design_brief`）の書き方を示す（D3.4）。
//! - reviewer prompt の health の材料と Result 節は `review`（D4.1・D5.4・D6.3）。
//!
//! ここは純粋関数だけで、I/O も LLM も無い。

use crate::protocol::RunContext;

pub mod review;

/// D1.1: 汎用規約の正本（binary に埋め込む。本文の見出しは 1 行目）。
pub const GENERIC_RULES: &str = include_str!("change_rules/generic.md");

/// repo 固有の規約の節の見出し（planner の brief の節はこの見出しで class の一覧を指す）。
pub const REPO_RULES_HEADING: &str = "# 変更の規約（この repo 固有）";

/// D1.1・D1.2: 汎用規約の節。見出しは generic.md の 1 行目のまま足さず、本文も変えない。
pub fn section() -> String {
    format!("{}\n\n", GENERIC_RULES.trim_end())
}

/// 汎用規約の直後に出す repo 固有の規約（owner 地図・高 risk の path class・保護する不変条件）。
/// `RunContext` に review 方針が無い・中身が空なら空文字列（prompt は変わらない）。
pub fn repo_section(context: &RunContext) -> String {
    let Some(policy) = context.review_policy() else {
        return String::new();
    };
    if policy.owner_map.is_none() && policy.high_risk.is_empty() && policy.invariants.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "{REPO_RULES_HEADING}\n\
         汎用規約に加えて、この repo の review 方針（base 側の profile の `[review]` 節）に従う。\n"
    );
    if let Some(map) = &policy.owner_map {
        out.push_str(&format!(
            "- owner 地図: `{map}`（変更する path の owner の行をここで引く）\n"
        ));
    }
    if !policy.high_risk.is_empty() {
        out.push_str(
            "- 高 risk の path class（path は repo 相対 path に当てる正規表現。この class の path を変える計画は\
             設計 brief が要り、設計 review の対象になる）:\n",
        );
        for class in &policy.high_risk {
            let paths: Vec<String> = class.paths.iter().map(|p| format!("`{p}`")).collect();
            out.push_str(&format!("  - `{}`: {}\n", class.class, paths.join(", ")));
        }
    }
    if !policy.invariants.is_empty() {
        out.push_str("- 保護する不変条件（名前: 根拠の file）:\n");
        for (name, source) in &policy.invariants {
            out.push_str(&format!("  - `{name}`: `{source}`\n"));
        }
    }
    out.push('\n');
    out
}

/// D3.4: /3 の planner に設計 brief（`design_brief`）の書き方を示す節。欄は汎用規約の「実装前 brief」の
/// 7 項目に対応させる。高 risk の path class の一覧は `repo_section` の節を指し、ここには再掲しない。
/// 方針は `repo_section` と同じ `RunContext.workspace` から読む（付記 B4）。
pub fn design_brief_section(context: &RunContext) -> String {
    let mut out = String::from(
        "### 設計 brief（`design_brief`）\n\
         /3 の計画は最上位に `design_brief` を 1 つ書ける。上の汎用規約の「実装前 brief」の 7 項目を、次の欄に書く:\n\
         1. 根本原因 → `root_cause`（空にしない）\n\
         2. 責務 owner → `owner`（空にしない）\n\
         3. 既存の再利用先 → `reuse`（文字列の配列）\n\
         4. 選択肢 → `options`（`{\"name\":\"...\",\"chosen\":true|false,\"reason\":\"...\"}` を 2 件以上。\
         `chosen: true` はちょうど 1 件）\n\
         5. 新しい状態・分岐・fallback・port → `new_state`・`new_branch`・`new_fallback`・`new_port`（文字列の配列。\
         足さないものは欄を省く。「無し」と書いた要素も足す宣言として数える）\n\
         6. 撤去対象 → `removals`\n\
         7. 不変条件 → `invariants`（守る条件と、それを確かめる試験）\n\
         どの文字列も空にしない。形の誤った brief は計画ごと拒否される。\n",
    );
    match context.review_policy() {
        Some(policy) => {
            let classes = if policy.high_risk.is_empty() {
                "この repo の review 方針には高 risk の path class が無い。".to_string()
            } else {
                format!(
                    "計画の unit が書き込む path（unit の `expected_write_paths` と leaf の `context.paths`）が、\
                     上の「{REPO_RULES_HEADING}」節の高 risk の path class に合うなら brief は必須。\
                     brief の無い計画は `DesignBriefMissing`（`design_brief is required: ...`）で差し戻される。"
                )
            };
            out.push_str(&format!(
                "- {classes}\n\
                 - celeris は review 方針・書き込む path・brief から risk を決める（LLM は使わない）。高 risk の \
                 class の path を変えて `new_state`・`new_branch`・`new_fallback`・`new_port` のどれかを宣言した計画と、\
                 `new_fallback` を宣言した計画は High。review 方針を読めなかったときは Unknown。High と Unknown の\
                 計画は、採用の前に reviewer が設計 review をする。不合格なら指摘を受けて計画を作り直す（2 回続けて\
                 不合格なら人の判断を待つ）。\n\n"
            ));
        }
        None => out.push_str(
            "- この run には repo の review 方針（高 risk の path class）が渡されていない。brief は任意で、書けば形を\
             検証する。base 側に review 方針（profile の `[review]` 節）が無い repo では、risk の判定と設計 review は\
             行わない。review 方針を読めなかったときは risk が Unknown になり、採用の前に reviewer が設計 review をする。\n\n",
        ),
    }
    out
}

#[cfg(test)]
#[path = "change_rules/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "change_rules/worker_profile_tests.rs"]
mod worker_profile_tests;
