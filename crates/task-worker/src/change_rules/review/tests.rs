//! ADR 2026-11-07-health-review-integration D3.5・D4.1・D5.1・D5.4・D6.3・D7.2 と付記『二度目の final review の
//! 差し戻しへの方針』(a) の reviewer prompt の試験（prefix `change_rules_review_` と、設計 review の
//! `change_rules_design_review_`）。finding 種別は `HealthFindingKind::ALL` から、規約の本文は `GENERIC_RULES` から
//! 読み、ここには写さない。

use std::collections::BTreeMap;

use task_core::TaskKind;
use task_core::design_review::{PathClass, ReviewPolicy, RiskAssessment, RiskLevel};
use task_core::execution_plan::{DesignBrief, DesignOption};

use super::*;
use crate::claude_code::build_prompt;
use crate::protocol::{
    DesignReviewRequest, HealthFindingCounts, HealthReportFinding, PathStat, ReviewOutput,
    ReviewRequest, RunContext,
};

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
            reason: None,
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

/// 付記 (a) A3: prompt の Result 節から例の JSON（"`{" で始まり "}`" で終わる行。節に 1 つだけ）を取り出し、
/// `protocol::ReviewOutput` で読む。
fn result_example(prompt: &str) -> ReviewOutput {
    let result = section_after(prompt, "## Result\n");
    let examples: Vec<&str> = result
        .lines()
        .take_while(|l| !l.starts_with(RESULT_JSON_HEAD))
        .filter_map(|l| l.strip_prefix('`').and_then(|l| l.strip_suffix('`')))
        .filter(|l| l.starts_with('{'))
        .collect();
    assert_eq!(examples.len(), 1, "one example JSON in:\n{result}");
    serde_json::from_str(examples[0])
        .unwrap_or_else(|e| panic!("example is not a ReviewOutput ({e}): {}", examples[0]))
}

/// 全欄を埋めた設計 brief（`new_port` だけ空にして「無し」の表示を見る）。
fn full_brief() -> DesignBrief {
    DesignBrief {
        root_cause: "planner が owner を決めずに計画する".to_string(),
        owner: "crates/alpha/src/plan.rs の Planner::plan".to_string(),
        reuse: vec!["既存の validate_v3".to_string()],
        options: vec![
            DesignOption {
                name: "owner-fix".to_string(),
                chosen: true,
                reason: "原因の owner で直す".to_string(),
            },
            DesignOption {
                name: "caller-patch".to_string(),
                chosen: false,
                reason: "呼び出し元に特例が増える".to_string(),
            },
        ],
        new_state: vec!["event PlanChecked".to_string()],
        new_branch: vec!["risk が high なら確認".to_string()],
        new_fallback: vec!["方針が読めなければ unknown".to_string()],
        new_port: Vec::new(),
        removals: vec!["旧い check_plan 経路".to_string()],
        invariants: vec!["events は追記専用".to_string()],
    }
}

fn policy() -> ReviewPolicy {
    ReviewPolicy {
        owner_map: Some("docs/architecture-map.md".to_string()),
        high_risk: vec![PathClass {
            class: "dispatch".to_string(),
            paths: vec![
                "^crates/alpha/src/".to_string(),
                "^scripts/health/".to_string(),
            ],
        }],
        invariants: BTreeMap::from([("events-append-only".to_string(), "RULES.md".to_string())]),
        definition_sha256: "sha-policy".to_string(),
    }
}

fn design_request(
    brief: Option<DesignBrief>,
    risk: RiskAssessment,
    review_policy: Option<ReviewPolicy>,
) -> ReviewRequest {
    ReviewRequest {
        design: Some(Box::new(DesignReviewRequest {
            brief,
            plan_summary:
                "rationale: owner を直す\n- unit `fix`: Planner を直す (writes crates/alpha/src/)"
                    .to_string(),
            risk,
            review_policy,
        })),
        ..ReviewRequest::default()
    }
}

fn high_risk() -> RiskAssessment {
    RiskAssessment {
        level: RiskLevel::High,
        reasons: vec![
            "path_class:dispatch+new_state".to_string(),
            "new_fallback".to_string(),
        ],
    }
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

/// (a): health がある review には、repo ごとの SHA・diff 統計・owner 行・findings・policy・不変条件が 1 節で出る。
/// 節は worker の evidence の後、Result 節の前にあり、1 回だけ出る。
#[test]
fn change_rules_review_with_health_shows_every_field() {
    let health = HealthReviewInput {
        repos: vec![repo("alpha", true), repo("beta", false)],
        brief: None,
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
/// 不合格、を書く。例の JSON（verdicts と health を 1 つの object に持つ）は節の中に 1 つだけある。
#[test]
fn change_rules_review_result_section_asks_for_the_health_verdict() {
    let changed = request(Some(HealthReviewInput {
        repos: vec![repo("alpha", true), repo("beta", false)],
        brief: None,
    }));
    let added = result_section(Some(&changed), "artifacts");
    assert!(added.starts_with("## Result\n"));
    assert!(added.contains("\"health\":{\"findings\":[{\"kind\":"));
    assert!(added.contains("\"policy\":{\"approved\":true,\"reason\":"));
    assert!(!added.contains('<'), "no placeholder in:\n{added}");
    let kinds_line = added
        .lines()
        .find(|l| l.starts_with("- `kind` は次の"))
        .unwrap_or_default();
    assert!(kinds_line.contains(&format!("次の {} 種", HealthFindingKind::ALL.len())));
    let repair_line = added
        .lines()
        .find(|l| l.starts_with("- `repair` は任意。"))
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
        brief: None,
    }));
    let result = result_section(Some(&unchanged), "artifacts");
    assert!(result.contains("`policy` は省く"));
    assert!(!result.contains("`health.policy` は必須"));
    assert!(!result.contains("\"policy\":"), "no policy in the example");

    let prompt = review_prompt(Some(changed));
    let example = result_example(&prompt);
    let example_kind = example.health.map(|h| h.findings[0].kind);
    for kind in HealthFindingKind::ALL {
        let expected =
            2 + usize::from(kind.allows_local_repair()) + usize::from(example_kind == Some(kind));
        assert_eq!(
            prompt.matches(kind.as_str()).count(),
            expected,
            "{}: generic rules + kind list (+ local repair list) (+ example)",
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
        brief: None,
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
        brief: None,
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
    let section = health_section(&request(Some(HealthReviewInput {
        repos: vec![whole],
        brief: None,
    })));
    assert!(!section.contains("上限で省略"), "{section}");
    assert!(!section.contains("reviewable=false"));
    assert!(section.contains("- owner 地図の行: 無し"));
    assert!(section.contains("- 保護する不変条件: 無し"));

    assert_eq!(health_section(&request(None)), "");
}

/// D7.2・付記 (a) A2: health の無い final review（WU の review・方針の無い repo）と review context の無い run でも、
/// Result 節は `verdicts` に任意の `repair` を含む形を示す（旧い verdicts だけの形の文は無い）。health・design の
/// 形と health の節は出ず、種別の名前は汎用規約にだけ出る。Result 節の後に result.json の指示が続く。
#[test]
fn change_rules_review_final_result_without_health_shows_repair() {
    for (case, prompt) in [
        ("health None", review_prompt(Some(request(None)))),
        ("no review context", review_prompt(None)),
    ] {
        assert!(!prompt.contains(HEALTH_HEADING), "{case}: health section");
        assert!(!prompt.contains("\"health\""), "{case}: health shape");
        assert!(!prompt.contains("health.policy"), "{case}: policy line");
        let result = section_after(&prompt, "## Result\n");
        assert!(
            result.contains("\"repair\":{\"scope\":\"local\",\"class\":\"test\",\"hint\":"),
            "{case}: repair in the example:\n{result}"
        );
        assert!(result.contains("- `repair` は任意で、`pass` が `false` のときだけ意味を持つ。"));
        assert!(result.contains("`format`・`lint`・`test`・`doc`・`other`"));
        let own = result.split(RESULT_JSON_HEAD).next().unwrap_or_default();
        assert!(
            !own.contains("<index>") && !own.contains("<bool>"),
            "{case}"
        );
        assert!(
            !result.contains("as a single JSON object of exactly this shape"),
            "{case}: the old verdicts-only sentence"
        );
        assert!(
            result.contains(&format!("\n\n{RESULT_JSON_HEAD}")),
            "{case}"
        );
        let example = result_example(&prompt);
        assert_eq!(example.verdicts.len(), 1, "{case}");
        assert!(example.verdicts[0].repair.is_some(), "{case}");
        assert!(
            example.health.is_none() && example.design.is_none(),
            "{case}"
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
    assert_eq!(
        result_section(None, "artifacts"),
        result_section(Some(&request(None)), "artifacts")
    );
}

/// 付記 (a) A1・A2: build_prompt で作った設計 review の prompt に、計画の要約・brief の全欄（空の欄は「無し」）・
/// risk の level と reasons の全件・review 方針（owner 地図・高 risk の class と paths・不変条件）・判定の観点（7 種）と、
/// Result 節の `design` の形が出る。受け入れ条件の判定・worker の summary と evidence の節は出ない。
#[test]
fn change_rules_design_review_prompt_shows_the_plan_and_the_design_shape() {
    let prompt = review_prompt(Some(design_request(
        Some(full_brief()),
        high_risk(),
        Some(policy()),
    )));
    assert_eq!(prompt.matches(DESIGN_HEADING).count(), 1);
    let section_at = prompt.find(DESIGN_HEADING).unwrap_or_default();
    let result_at = prompt.find("## Result\n").unwrap_or_default();
    assert!(section_at < result_at);
    let section = &prompt[section_at..result_at];
    for needle in [
        "採用前の計画",
        "読み取り専用",
        "### 計画の要約\nrationale: owner を直す\n- unit `fix`: Planner を直す (writes crates/alpha/src/)\n",
        "- root_cause: planner が owner を決めずに計画する\n",
        "- owner: crates/alpha/src/plan.rs の Planner::plan\n",
        "- reuse:\n  - 既存の validate_v3\n",
        "  - `owner-fix`（chosen=true）: 原因の owner で直す\n",
        "  - `caller-patch`（chosen=false）: 呼び出し元に特例が増える\n",
        "- new_state:\n  - event PlanChecked\n",
        "- new_branch:\n  - risk が high なら確認\n",
        "- new_fallback:\n  - 方針が読めなければ unknown\n",
        "- new_port: 無し\n",
        "- removals:\n  - 旧い check_plan 経路\n",
        "- invariants:\n  - events は追記専用\n",
        "- level: `high`\n",
        "  - `path_class:dispatch+new_state`\n",
        "  - `new_fallback`\n",
        "- owner 地図: `docs/architecture-map.md`",
        "  - `dispatch`: `^crates/alpha/src/`、`^scripts/health/`\n",
        "  - `events-append-only`: `RULES.md`\n",
        "### 判定の観点",
    ] {
        assert!(
            section.contains(needle),
            "{needle:?} missing in:\n{section}"
        );
    }
    let viewpoint = section_after(section, "### 判定の観点");
    for kind in HealthFindingKind::ALL {
        assert!(
            viewpoint.contains(&format!("`{}`", kind.as_str())),
            "{}",
            kind.as_str()
        );
    }
    for absent in [
        "## Criteria you must judge in this run",
        "## Evidence self-reported by the worker",
        "## Worker's self-reported summary",
        HEALTH_HEADING,
    ] {
        assert!(
            !prompt.contains(absent),
            "{absent:?} in the design review prompt"
        );
    }
    let result = section_after(&prompt, "## Result\n");
    assert!(
        result.contains("`{\"verdicts\":[],\"design\":{\"pass\":false,\"findings\":[{\"kind\":")
    );
    assert!(result.contains(&format!(
        "- `kind` は次の {} 種のどれか",
        HealthFindingKind::ALL.len()
    )));
    assert!(result.contains("- `repair` は書かない。"));
    assert!(
        !result.contains("\"repair\""),
        "no repair in the design example"
    );
    assert!(!result.contains("Criteria you must judge"));
}

/// 付記 (a) A3: Result 節の例の JSON は placeholder を使わず、`protocol::ReviewOutput` でそのまま読める。設計 review
/// では `design` が `Some`（verdicts は空・repair 無し）、health 付きの final review では `health` と
/// `verdicts[0].repair` が `Some`。`ReviewOutput` は未知の欄を捨てるので、欄名の誤りは `Some` の確認で検出する。
#[test]
fn change_rules_design_review_result_examples_parse_as_review_output() {
    let design = result_example(&review_prompt(Some(design_request(
        Some(full_brief()),
        high_risk(),
        Some(policy()),
    ))));
    assert!(design.verdicts.is_empty());
    assert!(design.health.is_none());
    let verdict = design.design.unwrap_or_else(|| panic!("design missing"));
    assert!(!verdict.pass);
    assert_eq!(verdict.findings.len(), 1);
    assert_eq!(verdict.findings[0].severity, HealthSeverity::Blocking);
    assert!(verdict.findings[0].repair.is_none());

    let mut health_request = request(Some(HealthReviewInput {
        repos: vec![repo("alpha", true)],
        brief: None,
    }));
    health_request.criteria = vec![2];
    let final_review = result_example(&review_prompt(Some(health_request)));
    assert!(final_review.design.is_none());
    assert_eq!(final_review.verdicts.len(), 1);
    assert_eq!(
        final_review.verdicts[0].criterion, 2,
        "the first criterion to judge"
    );
    let repair = final_review.verdicts[0]
        .repair
        .clone()
        .unwrap_or_else(|| panic!("verdicts[0].repair missing"));
    assert_eq!(repair.scope, "local");
    let health = final_review
        .health
        .unwrap_or_else(|| panic!("health missing"));
    assert_eq!(health.findings.len(), 1);
    assert!(health.findings[0].kind.allows_local_repair());
    assert!(health.findings[0].repair.is_some());
    assert!(
        health.policy.is_some(),
        "policy changed: policy in the example"
    );
}

/// 付記 (a) A2: design の無い final review（health の有無に依らない）と review context の無い run の prompt には、
/// 設計 review の節も `design` の形も出ず、`design_section` は空文字列を返す。
#[test]
fn change_rules_design_review_absent_from_final_review() {
    let with_health = request(Some(HealthReviewInput {
        repos: vec![repo("alpha", false)],
        brief: None,
    }));
    for (case, review) in [
        ("health None", Some(request(None))),
        ("health Some", Some(with_health)),
        ("no review context", None),
    ] {
        assert_eq!(design_section(review.as_ref()), "", "{case}");
        let has_review = review.is_some();
        let prompt = review_prompt(review);
        assert!(!prompt.contains(DESIGN_HEADING), "{case}");
        assert!(!prompt.contains("\"design\""), "{case}: design shape");
        assert!(!prompt.contains("`design`"), "{case}: design instructions");
        assert!(!prompt.contains("### 判定の観点"), "{case}");
        assert_eq!(
            prompt.contains("## Criteria you must judge in this run"),
            has_review,
            "{case}"
        );
        assert!(result_example(&prompt).design.is_none(), "{case}");
    }
}

/// 付記 (a) A1・(d): brief の無い計画は「brief 無し」、review 方針を読めなかった計画（remote workspace など）は
/// 読めなかった旨と `review_policy_unavailable:` の理由を示し、汎用規約と brief だけで判定させる。理由の無い
/// risk でも読めなかった旨は出る。空の文字列の欄は「無し」と出す。
#[test]
fn change_rules_design_review_without_policy_or_brief_explains_why() {
    let unknown = RiskAssessment {
        level: RiskLevel::Unknown,
        reasons: vec!["review_policy_unavailable:remote workspace".to_string()],
    };
    let section = design_section(Some(&design_request(None, unknown, None)));
    for needle in [
        "brief 無し",
        "- level: `unknown`\n",
        "  - `review_policy_unavailable:remote workspace`\n",
        "この repo の review 方針を読めなかった。理由: `review_policy_unavailable:remote workspace`。",
        "設計 brief だけで判定する",
    ] {
        assert!(
            section.contains(needle),
            "{needle:?} missing in:\n{section}"
        );
    }
    assert!(section.contains(&format!("上の「{}」と", generic_heading())));
    assert!(!section.contains("- owner 地図"));
    assert!(!section.contains("- root_cause"));

    let mut empty = full_brief();
    empty.reuse.clear();
    empty.options.clear();
    empty.owner = String::new();
    let no_reason = RiskAssessment {
        level: RiskLevel::High,
        reasons: Vec::new(),
    };
    let section = design_section(Some(&design_request(Some(empty), no_reason, None)));
    for needle in [
        "- owner: 無し\n",
        "- reuse: 無し\n",
        "- options: 無し\n",
        "- reasons: 無し\n",
        "読めなかった理由は risk の reasons に無い。",
    ] {
        assert!(
            section.contains(needle),
            "{needle:?} missing in:\n{section}"
        );
    }
}
