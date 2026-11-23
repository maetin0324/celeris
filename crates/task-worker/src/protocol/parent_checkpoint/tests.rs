//! ADR 2026-11-22-parent-checkpoint-evaluator D3.1・D3.2 の wire の試験（prefix `parent_evaluation_wire_`）。
//! 既存の試験は `strategy_critic`・`design`・`health` の欄だけを見るので、親の途中評価の入力と出力の欄が wire・schema・
//! 文書（worker-protocol.md）に揃って載ることを検出できない。

use task_core::parent_checkpoint::{
    EVALUATION_SCHEMA, PARENT_EVALUATION_RUBRIC, ParentEvaluation, RecordStatus,
};

use super::*;
use crate::protocol::{ReviewOutput, ReviewRequest};

/// 入力の全欄を埋めた `ParentCheckpointRequest`（アーキv4 の E-mid の時点の形）。
pub(crate) fn sample_parent_checkpoint() -> ParentCheckpointRequest {
    let child = |id: &str, title: &str, status: task_core::Status| ParentCheckpointChild {
        task_id: id.parse().expect("fixed child id parses"),
        title: title.to_string(),
        status,
        reviewer_passed: status == task_core::Status::Done,
        criteria: vec![ParentCheckpointCriterion {
            text: "SCC の数を測る".to_string(),
            kind: "reviewer".to_string(),
            result: "pass".to_string(),
        }],
        reports: vec![task_core::parent_checkpoint::ChildReport {
            seq: 7,
            at: "2026-11-18T19:55:39Z".to_string(),
            role: task_core::parent_checkpoint::ReportRole::Worker,
            text: "未達: 循環は残った".to_string(),
            truncated: false,
        }],
    };
    ParentCheckpointRequest {
        decision_id: "01M82FC782QS6F23C4EN9P72CT".to_string(),
        root_id: "01M7MZT8KX547464YYS76QNP4G".parse().expect("fixed root id"),
        triggers: vec!["children:2".to_string(), "budget:50".to_string()],
        goal: ParentCheckpointGoal {
            request_sha256: "d".repeat(64),
            outcomes: vec![ParentCheckpointOutcome {
                id: "scc".to_string(),
                title: "task-core の SCC を減らす".to_string(),
                acceptance: vec!["SCC 17 → 12 以下".to_string()],
                reached: false,
            }],
        },
        strategy: StrategyRef {
            task_id: "01M7XQ9QQXY1DY56EXE0TT8X6W".to_string(),
            name: "strategy/arch-v4/v1.json".to_string(),
            sha256: "a".repeat(64),
            candidate: Some("fixed-plan".to_string()),
        },
        children: vec![
            child(
                "01M7MZT8KX547464YYS76QNP4A",
                "子 A",
                task_core::Status::Done,
            ),
            child(
                "01M7MZT8KX547464YYS76QNP4B",
                "子 B",
                task_core::Status::Done,
            ),
        ],
        metrics: ParentCheckpointMetrics {
            children_terminal: 2,
            children_total: 8,
            tokens_used: 1_000,
            max_tokens: Some(2_000),
        },
        previous: vec![ParentCheckpointPrevious {
            name: "parent-evaluation/01M82FC782QS6F23C4EN9P72CS.json".to_string(),
            sha256: "b".repeat(64),
        }],
        rubric: ParentCheckpointRequest::fixed_rubric(),
    }
}

/// D3.2 の形の評価（参照は子 2 の `Reference` の形）。
pub(crate) fn sample_evaluation() -> serde_json::Value {
    serde_json::json!({
        "schema": EVALUATION_SCHEMA,
        "verdict": "off_track",
        "next": "discriminating_experiment",
        "axes": {"execution": "succeeded", "epistemic": "advanced", "goal": "not_met"},
        "measured": [{"claim": "SCC は減っていない", "metric": "task-core SCC", "before": "55", "after": "55",
                      "references": [{"kind": "artifact", "task_id": "01M7MZT8KX547464YYS76QNP4A",
                                      "name": "scc.json", "sha256": "c".repeat(64)}]}],
        "learned": [{"statement": "子の done は SCC を動かさない",
                     "references": [{"kind": "task", "task_id": "01M7MZT8KX547464YYS76QNP4B"}]}],
        "refuted": [{"hypothesis": "H1", "statement": "file 分割で SCC が減る",
                     "references": [{"kind": "commit", "repo": "agent-platform", "sha": "0".repeat(40)}]}],
        "uncertainty": [{"statement": "S1 の file 数が減るか", "resolve_by": "S1 を 1 回測る"}],
        "reason": "子 A・B の done の後も SCC は 55 のまま"
    })
}

/// 不変条件: `parent_checkpoint` の入力は `request.json` に key として出て読み戻せる。rubric は
/// `task_core::parent_checkpoint::PARENT_EVALUATION_RUBRIC` の E1〜E6 と同じ。`parent_evaluation` の出力は値のまま読め、
/// task-core の `ParentEvaluation`（形の定義の正本）として読める。
#[test]
fn parent_evaluation_wire_roundtrip() {
    let request = ReviewRequest {
        parent_checkpoint: Some(Box::new(sample_parent_checkpoint())),
        ..ReviewRequest::default()
    };
    let json = serde_json::to_value(&request).expect("serializes");
    assert!(json.get("parent_checkpoint").is_some(), "{json}");
    assert_eq!(json["criteria"], serde_json::json!([]));
    let back: ReviewRequest = serde_json::from_value(json).expect("round trip");
    assert_eq!(back, request);
    let rubric: Vec<(String, String)> = ParentCheckpointRequest::fixed_rubric()
        .into_iter()
        .map(|i| (i.id, i.text))
        .collect();
    let table: Vec<(String, String)> = PARENT_EVALUATION_RUBRIC
        .iter()
        .map(|(id, text)| ((*id).to_string(), (*text).to_string()))
        .collect();
    assert_eq!(rubric, table);

    let review_json = serde_json::json!({"verdicts": [], "parent_evaluation": sample_evaluation()});
    let out: ReviewOutput = serde_json::from_value(review_json.clone()).expect("review.json reads");
    assert!(out.verdicts.is_empty());
    assert_eq!(out.parent_evaluation.as_ref(), Some(&sample_evaluation()));
    let typed: ParentEvaluation =
        serde_json::from_value(sample_evaluation()).expect("task-core shape");
    assert_eq!(typed.schema, EVALUATION_SCHEMA);
    assert_eq!(serde_json::to_value(&out).expect("serializes"), review_json);
}

/// 不変条件: 形の崩れた評価（未知の欄 `achieved`・未知の verdict）でも `review.json` は `ReviewOutput` として読め、
/// 形の判定は task-core の `check_evaluation` が `invalid` として記録する（wire の型が先に読み込みエラーにすると、
/// 違反の記録が残らず「review.json が読めない」になる）。
#[test]
fn parent_evaluation_wire_malformed_evaluation_still_reads() {
    let mut bad = sample_evaluation();
    bad["achieved"] = serde_json::json!(true);
    bad["verdict"] = serde_json::json!("done");
    let out: ReviewOutput =
        serde_json::from_value(serde_json::json!({"verdicts": [], "parent_evaluation": bad}))
            .expect("review.json still reads");
    let facts = task_core::parent_checkpoint::EvaluationFacts {
        resolved: Vec::new(),
        goal_reached: false,
        children: Default::default(),
    };
    let checked =
        task_core::parent_checkpoint::check_evaluation(out.parent_evaluation.as_ref(), &facts);
    assert_eq!(checked.status, RecordStatus::Invalid);
    let good = task_core::parent_checkpoint::check_evaluation(Some(&sample_evaluation()), &facts);
    assert_eq!(good.status, RecordStatus::Checked, "{:?}", good.violations);
}

fn defs() -> serde_json::Value {
    crate::protocol::schema_value()["$defs"].clone()
}

fn properties(defs: &serde_json::Value, name: &str) -> Vec<String> {
    defs[name]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("$defs.{name}.properties missing"))
        .keys()
        .cloned()
        .collect()
}

/// 不変条件: 生成した schema の `ReviewRequest.parent_checkpoint`・`ReviewOutput.parent_evaluation` と、その中の型の欄は、
/// worker-protocol.md の §10.2 の「親の途中評価」の節に欄名として全部書いてある（子 5 の final review は md の欠けで
/// 不合格になった）。評価の型は task-core の `ParentEvaluation` から生成され、未知の欄を受けない。
#[test]
fn parent_evaluation_wire_schema_matches_md() {
    let defs = defs();
    assert!(properties(&defs, "ReviewRequest").contains(&"parent_checkpoint".to_string()));
    assert!(properties(&defs, "ReviewOutput").contains(&"parent_evaluation".to_string()));
    assert_eq!(defs["ParentEvaluation"]["additionalProperties"], false);
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/protocol/worker-protocol.md"
    );
    let md = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let start = md
        .find("#### 親の途中評価")
        .unwrap_or_else(|| panic!("section missing in {path}"));
    let section = &md[start..];
    let end = section[5..].find("\n#").map_or(section.len(), |i| i + 5);
    let section = &section[..end];
    // 節の code span（`…`）に現れる識別子の集合。欄は `{a, b}`・`a[]`・`a?` の形で書いてよい。
    let named: std::collections::BTreeSet<&str> = section
        .split('`')
        .skip(1)
        .step_by(2)
        .flat_map(|span| span.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
        .filter(|w| !w.is_empty())
        .collect();
    for name in [
        "ParentCheckpointRequest",
        "ParentCheckpointGoal",
        "ParentCheckpointOutcome",
        "StrategyRef",
        "ParentCheckpointChild",
        "ParentCheckpointCriterion",
        "ParentCheckpointMetrics",
        "ParentCheckpointPrevious",
        "ParentEvaluation",
        "ParentEvaluationAxes",
        "ParentEvaluationMeasured",
        "ParentEvaluationLearned",
        "ParentEvaluationRefuted",
        "ParentEvaluationUncertainty",
    ] {
        assert!(named.contains(name), "{name} missing in md");
        for field in properties(&defs, name) {
            assert!(
                named.contains(field.as_str()),
                "{name}.{field} missing in md section"
            );
        }
    }
    for name in [
        "ParentEvaluationVerdict",
        "ParentEvaluationNext",
        "ParentEvaluationExecution",
        "ParentEvaluationEpistemic",
        "ParentEvaluationGoal",
    ] {
        let values = defs[name]["enum"]
            .as_array()
            .unwrap_or_else(|| panic!("$defs.{name}.enum missing"));
        for v in values {
            let v = v.as_str().expect("enum value is a string");
            assert!(named.contains(v), "{name} value {v} missing in md");
        }
    }
    for heading in ["context.review", "parent_checkpoint?"] {
        assert!(md.contains(heading), "{heading} missing");
    }
    let table_row = md
        .lines()
        .find(|l| l.starts_with("| `context.review`"))
        .expect("context.review row");
    assert!(table_row.contains("parent_checkpoint?"), "{table_row}");
}
