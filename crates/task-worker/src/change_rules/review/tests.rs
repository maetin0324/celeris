//! ADR 2026-11-07-health-review-integration D4.1・D5.1・D5.4・D6.3 の reviewer prompt の試験（prefix
//! `change_rules_review_`）。finding 種別は `HealthFindingKind::ALL` から、規約の本文は `GENERIC_RULES` から読み、
//! ここには写さない。

use std::collections::BTreeMap;

use task_core::TaskKind;

use super::*;
use crate::claude_code::build_prompt;
use crate::protocol::{
    HealthFindingCounts, HealthReportFinding, PathStat, ReviewRequest, RunContext,
};

/// health の無い review の Result 節（rules-brief の後の文面。artifacts は `artifacts`）。
const LEGACY_RESULT: &str = "## Result\n\
    Write your verdicts to `artifacts/review.json` (create the `artifacts/` directory if it does not exist \
    yet) as a single JSON object of exactly this shape: \
    `{\"verdicts\":[{\"criterion\":<index>,\"pass\":<bool>,\"reason\":\"...\"}]}`. You must write exactly \
    one verdict for each criterion listed under \"Criteria you must judge in this run\" above.\n\n";

/// Result 節の次に来る result.json の指示の先頭。
const RESULT_JSON_HEAD: &str = "Always write `artifacts/result.json`";

fn review_prompt(review: Option<ReviewRequest>) -> String {
    let mut task = crate::protocol::tests::sample_task();
    task.kind = TaskKind::Review;
    let context = RunContext {
        review,
        ..RunContext::default()
    };
    build_prompt(&task, &context, "run-change-rules-review", "artifacts")
}

fn request(health: Option<HealthReviewInput>) -> ReviewRequest {
    ReviewRequest {
        summary: "did the work".to_string(),
        criteria: vec![0],
        health,
        ..ReviewRequest::default()
    }
}

fn repo(repo_id: &str, policy_changed: bool) -> HealthRepoInput {
    HealthRepoInput {
        repo_id: repo_id.to_string(),
        target_sha: format!("{repo_id}-target"),
        reviewed_sha: format!("{repo_id}-reviewed"),
        diff: DiffStat {
            files: 3,
            insertions: 10,
            deletions: 4,
            paths: vec![
                PathStat {
                    path: "crates/alpha/src/lib.rs".to_string(),
                    insertions: 8,
                    deletions: 1,
                },
                PathStat {
                    path: "scripts/health/rules.py".to_string(),
                    insertions: 2,
                    deletions: 3,
                },
            ],
        },
        owner_rows: vec![OwnerRow {
            path: "crates/alpha/src/lib.rs".to_string(),
            row: "| alpha | crates/alpha | Alpha::run |".to_string(),
        }],
        findings: HealthFindingsSummary {
            verdict: "fail".to_string(),
            reviewable: true,
            counts: HealthFindingCounts {
                findings: 5,
                fail_grade: 1,
                new: 1,
                worsened: 1,
                existing: 3,
            },
            new: vec![
                HealthReportFinding {
                    rule_id: "size.file".to_string(),
                    fingerprint: "fp-1".to_string(),
                    severity: "error".to_string(),
                    delta: "new".to_string(),
                    path: Some("crates/alpha/src/lib.rs".to_string()),
                },
                HealthReportFinding {
                    rule_id: "deps.cycle".to_string(),
                    fingerprint: "fp-2".to_string(),
                    severity: "warning".to_string(),
                    delta: "worsened".to_string(),
                    path: None,
                },
            ],
            truncated: 0,
        },
        policy: HealthPolicyState {
            source: "base".to_string(),
            changed: policy_changed,
            files: if policy_changed {
                vec!["scripts/health/rules.py".to_string()]
            } else {
                Vec::new()
            },
        },
        invariants: BTreeMap::from([("events-append-only".to_string(), "RULES.md".to_string())]),
    }
}

fn section_after<'a>(prompt: &'a str, heading: &str) -> &'a str {
    let start = prompt
        .find(heading)
        .unwrap_or_else(|| panic!("{heading:?} missing in:\n{prompt}"));
    &prompt[start..]
}

/// 汎用規約の項目のうち、末尾の括弧に種別の名前を書いた行の説明文（`N. ` の後から括弧の前まで）。
fn generic_kind_descriptions() -> Vec<(HealthFindingKind, String)> {
    HealthFindingKind::ALL
        .iter()
        .map(|kind| {
            let marker = format!("（`{}`）", kind.as_str());
            let line = GENERIC_RULES
                .lines()
                .find(|l| l.contains(&marker))
                .unwrap_or_else(|| panic!("generic.md has no item for {}", kind.as_str()));
            let body = line.split_once(". ").map_or(line, |(_, rest)| rest);
            let description = body.split(&marker).next().unwrap_or_default().to_string();
            (*kind, description)
        })
        .collect()
}

/// (c)・I-health-none: health が無い review（WU の review・方針の無い repo）と review context の無い run では、
/// health の節も Result 節の追加文も出ず、Result 節は従来の文字列のまま result.json の指示に続く。
#[test]
fn change_rules_review_without_health_keeps_the_legacy_prompt() {
    for (case, prompt) in [
        ("health None", review_prompt(Some(request(None)))),
        ("no review context", review_prompt(None)),
    ] {
        assert!(!prompt.contains(HEALTH_HEADING), "{case}: health section");
        assert!(!prompt.contains("\"health\""), "{case}: health shape");
        assert!(!prompt.contains("health.policy"), "{case}: policy line");
        let result = section_after(&prompt, "## Result\n");
        assert!(
            result.starts_with(&format!("{LEGACY_RESULT}{RESULT_JSON_HEAD}")),
            "{case}: Result section changed:\n{result}"
        );
        for kind in HealthFindingKind::ALL {
            assert_eq!(
                prompt.matches(kind.as_str()).count(),
                1,
                "{case}: {} must come only from the generic rules",
                kind.as_str()
            );
        }
    }
    assert_eq!(result_section(None, "artifacts"), LEGACY_RESULT);
    assert_eq!(
        result_section(Some(&request(None)), "artifacts"),
        LEGACY_RESULT
    );
}

/// (a): health がある review には、repo ごとの SHA・diff 統計・owner 行・findings・policy・不変条件が 1 節で出る。
/// 節は worker の evidence の後、Result 節の前にあり、1 回だけ出る。
#[test]
fn change_rules_review_with_health_shows_every_field() {
    let health = HealthReviewInput {
        repos: vec![repo("alpha", true), repo("beta", false)],
    };
    let prompt = review_prompt(Some(request(Some(health))));
    assert_eq!(prompt.matches(HEALTH_HEADING).count(), 1);
    let evidence = prompt
        .find("## Evidence self-reported by the worker")
        .unwrap_or_default();
    let section_at = prompt.find(HEALTH_HEADING).unwrap_or_default();
    let result_at = prompt.find("## Result\n").unwrap_or_default();
    assert!(evidence < section_at && section_at < result_at);
    let section = &prompt[section_at..result_at];
    for needle in [
        "### repo `alpha`",
        "### repo `beta`",
        "target_sha: `alpha-target`",
        "reviewed_sha: `alpha-reviewed`",
        "- diff: 3 file、+10 行・-4 行",
        "  - `crates/alpha/src/lib.rs`（+8・-1）",
        "  - `scripts/health/rules.py`（+2・-3）",
        "  - ほか 1 file は上限で省略",
        "  - `crates/alpha/src/lib.rs`: | alpha | crates/alpha | Alpha::run |",
        "verdict=`fail`、reviewable=true、件数 findings=5・fail_grade=1・new=1・worsened=1・existing=3",
        "    - `size.file` severity=`error` delta=`new` path=`crates/alpha/src/lib.rs`\n",
        "    - `deps.cycle` severity=`warning` delta=`worsened`\n",
        "- policy: source=`base`、changed=true、変わった policy の file: `scripts/health/rules.py`\n",
        "- policy: source=`base`、changed=false\n",
        "  - `events-append-only`: `RULES.md`",
    ] {
        assert!(
            section.contains(needle),
            "{needle:?} missing in:\n{section}"
        );
    }
    assert_eq!(
        section.matches("`health.policy` を必ず書く").count(),
        1,
        "only the changed repo asks for health.policy"
    );
    assert!(!section.contains("上限で省略した finding"));
}

/// (b): health があれば Result 節に `health` の形が足され、種別は型の 7 種、局所修復は `allows_local_repair` の
/// 種別だけ、policy の file を変えた repo があれば `health.policy` が必須、health の無い・壊れた review.json は
/// 不合格、を書く。従来の文面はそのまま節の先頭に残る。
#[test]
fn change_rules_review_result_section_asks_for_the_health_verdict() {
    let changed = request(Some(HealthReviewInput {
        repos: vec![repo("alpha", true), repo("beta", false)],
    }));
    let result = result_section(Some(&changed), "artifacts");
    assert!(result.starts_with(LEGACY_RESULT));
    let added = &result[LEGACY_RESULT.len()..];
    assert!(added.contains("\"health\":{\"findings\":[{\"kind\":"));
    assert!(added.contains("\"policy\":{\"approved\":<bool>,\"reason\":\"...\"}"));
    let kinds_line = added
        .lines()
        .find(|l| l.starts_with("- `kind` は次の"))
        .unwrap_or_default();
    assert!(kinds_line.contains(&format!("次の {} 種", HealthFindingKind::ALL.len())));
    let repair_line = added
        .lines()
        .find(|l| l.starts_with("- `repair` は任意"))
        .unwrap_or_default();
    for kind in HealthFindingKind::ALL {
        let quoted = format!("`{}`", kind.as_str());
        assert!(kinds_line.contains(&quoted), "{quoted} missing");
        assert_eq!(
            repair_line.contains(&quoted),
            kind.allows_local_repair(),
            "local repair for {quoted}"
        );
    }
    assert!(added.contains(&format!(
        "上の「{}」で末尾の括弧に同じ名前を",
        generic_heading()
    )));
    assert!(
        added.contains("policy の file を変えた repo（`alpha`）があるので `health.policy` は必須")
    );
    assert!(!added.contains("`policy` は省く"));
    assert!(added.contains("`health` が無い review.json"));
    assert!(added.contains("全 criterion が不合格になる"));

    let unchanged = request(Some(HealthReviewInput {
        repos: vec![repo("beta", false)],
    }));
    let result = result_section(Some(&unchanged), "artifacts");
    assert!(result.contains("`policy` は省く"));
    assert!(!result.contains("`health.policy` は必須"));

    let prompt = review_prompt(Some(changed));
    for kind in HealthFindingKind::ALL {
        let expected = if kind.allows_local_repair() { 3 } else { 2 };
        assert_eq!(
            prompt.matches(kind.as_str()).count(),
            expected,
            "{}: generic rules + kind list (+ local repair list)",
            kind.as_str()
        );
    }
}

/// (b)・受け入れ条件 2: 種別の説明文は汎用規約の 1 か所だけにあり、health の節・Result 節と module の source
/// （review.rs）には写さない。種別の名前も source に書かず、`HealthFindingKind` から出す。汎用規約は 7 種の
/// どれにも括弧内の名前の項目を持つ（Result 節の参照先が必ずある）。
#[test]
fn change_rules_review_kind_descriptions_are_not_copied() {
    const SOURCE: &str = include_str!("../review.rs");
    const PROTOCOL: &str = include_str!("../../protocol/health.rs");
    let prompt = review_prompt(Some(request(Some(HealthReviewInput {
        repos: vec![repo("alpha", true)],
    }))));
    for (kind, description) in generic_kind_descriptions() {
        assert!(description.chars().count() >= 12, "{description:?}");
        assert_eq!(
            prompt.matches(description.as_str()).count(),
            1,
            "{}: description only from the generic rules",
            kind.as_str()
        );
        assert!(!SOURCE.contains(&description), "{description:?} copied");
        assert!(
            !SOURCE.contains(kind.as_str()),
            "{} written in review.rs",
            kind.as_str()
        );
    }
    let enum_body = PROTOCOL
        .split_once("pub enum HealthFindingKind {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(body, _)| body)
        .unwrap_or_default();
    let docs: Vec<&str> = enum_body
        .lines()
        .filter_map(|l| l.trim().strip_prefix("/// "))
        .collect();
    assert_eq!(docs.len(), HealthFindingKind::ALL.len());
    for doc in docs {
        assert!(
            !SOURCE.contains(doc),
            "{doc:?} copied from HealthFindingKind"
        );
        assert!(!prompt.contains(doc), "{doc:?} in the prompt");
    }
}

/// (a): 上限で切り詰めた材料は省いた数を出す（`findings.truncated` と、`diff.files` と `paths` の差）。
/// reviewable=false の report は材料にしないと書き、切り詰めが無ければ省略の行を出さない。
#[test]
fn change_rules_review_truncated_material_is_shown() {
    let mut truncated = repo("alpha", false);
    truncated.diff.files = 45;
    truncated.findings.truncated = 7;
    truncated.findings.reviewable = false;
    truncated.findings.new.clear();
    let section = health_section(&request(Some(HealthReviewInput {
        repos: vec![truncated],
    })));
    assert!(
        section.contains("  - ほか 43 file は上限で省略\n"),
        "{section}"
    );
    assert!(section.contains("  - 上限で省略した finding: 7 件（上の一覧に無い）\n"));
    assert!(section.contains("reviewable=false なので材料にしない"));
    assert!(section.contains("  - new・worsened の finding（例外の無いもの）: 無し\n"));

    let mut whole = repo("beta", false);
    whole.diff.files = 2;
    whole.owner_rows.clear();
    whole.invariants.clear();
    let section = health_section(&request(Some(HealthReviewInput { repos: vec![whole] })));
    assert!(!section.contains("上限で省略"), "{section}");
    assert!(!section.contains("reviewable=false"));
    assert!(section.contains("- owner 地図の行: 無し"));
    assert!(section.contains("- 保護する不変条件: 無し"));

    assert_eq!(health_section(&request(None)), "");
}
