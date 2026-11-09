//! ADR 2026-11-08-health-enforcement-loop D2.6（G3 loop-points delivery-path）: delivery の merge candidate の照合
//! （`check_required_evidence`）が、review の記録した health の証跡をそのまま通し、証跡自身の `base_sha` が束縛の
//! `target_sha` と違う証跡と、承認を束縛から確かめられない PolicyOnly の証跡を通さない試験。
//!
//! 既存の `health_evidence_delivery_*` は証跡を手で作り、`base_sha` を常に `target_sha` にするので、review の証跡と
//! delivery の照合の食い違い・別の基底を評価した証跡を検出できない。同梱の検査器は引数で渡す（process の OnceLock
//! `configure_bundled_checker` は呼ばない）。
use super::health_evidence::{deliver, record_health};
use super::*;
use task_dispatch::health_review::gate::{self, HealthGateOutcome, HealthGateRequest};
use task_dispatch::health_review::{policy::REVIEW_POLICY_SCHEMA, review_item};

/// 合格の report を返す同梱の検査器を `root` に書いて読む（`health_review::stub_checker` と同じ入出力。LLM を
/// 呼ばず、外部ネットワークに出ない）。
fn passing_bundled_checker(root: &Path) -> gate::BundledChecker {
    let script = r#"import os, sys
here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
if "--print-review-policy" in args:
    with open(os.path.join(here, "policy.json")) as f:
        sys.stdout.write(f.read())
    sys.exit(0)
with open(os.path.join(here, "report.json")) as src:
    with open(args[args.index("--out") + 1], "w") as out:
        out.write(src.read())
sys.exit(0)
"#;
    let policy = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": "absent",
        "owner_map": null,
        "high_risk": [],
        "invariants": {},
        "definition_sha256": "d".repeat(64),
    });
    let report = serde_json::json!({
        "schema": gate::REPORT_SCHEMA,
        "tool": {"version": "delivery-test"},
        "inputs": {"profile": {"resolved": "builtin", "source": "builtin"}},
        "summary": {
            "verdict": "pass",
            "exit_code": 0,
            "reviewable": true,
            "counts": {"findings": 0, "fail_grade": 0, "new": 0, "worsened": 0, "existing": 0},
        },
        "rules": [],
        "findings": [],
        "policy": {"version": gate::POLICY_VERSION, "source": "builtin", "changed": false, "files": []},
    });
    let dir = root.join(health::CHECKER_DIR);
    fs::create_dir_all(&dir).unwrap();
    fs::write(root.join(health::CHECKER_SCRIPT), script).unwrap();
    fs::write(dir.join("policy.json"), policy.to_string()).unwrap();
    fs::write(dir.join("report.json"), report.to_string()).unwrap();
    fs::write(
        root.join(gate::CHECKER_TREE_FILE),
        format!("repo-health {}\n", "e".repeat(40)),
    )
    .unwrap();
    gate::read_bundled_checker(root).unwrap()
}

/// 不変条件: review が経路共通の入口（`gate::health_gate`）と review の項目（`review_item::item`）で作り、
/// review の記録と同じ形（`RequiredChecksEvaluated`、指紋は `definition_digest(cmds, 項目の検査器)`）で残した
/// health の証跡は、delivery の照合をそのまま通って main に取り込まれる。
#[test]
fn health_enforce_delivery_review_evidence_passes() {
    let (store, dir, d) = merge_queued_without_evidence();
    let checker_root = tempfile::tempdir().unwrap();
    let bundled = passing_bundled_checker(checker_root.path());
    let (target, reviewed) = (
        d.target_sha.clone().unwrap(),
        d.reviewed_sha.clone().unwrap(),
    );
    let outcome = gate::health_gate(&HealthGateRequest {
        repo_id: d.repo_id,
        worktree: dir.path(),
        base: &target,
        head: &reviewed,
        integrated_by_celeris: true,
        bundled: Some(&bundled),
        timeout: Duration::from_secs(60),
    });
    assert!(
        matches!(outcome, HealthGateOutcome::Evaluated(_)),
        "{outcome:?}"
    );
    let item = review_item::item(&outcome);
    let result = item.result.clone().expect("health の項目");
    assert!(result.pass, "{result:?}");
    assert_eq!(item.checker.as_ref(), Some(&bundled.id));
    let evidence = result.health.as_deref().expect("health の証跡");
    assert_eq!(
        evidence.base_sha, target,
        "review の証跡は束縛の基底を評価する"
    );
    record_results(&store, &d, vec![result], &[], item.checker.as_ref());

    assert_eq!(deliver(&store, dir.path(), &d, Some(&bundled.id)), None);
}

/// 不変条件: 束縛（event の `target_sha`）は今の基底でも、health の証跡自身の `base_sha` が違えば（別の基底を
/// 評価した証跡の写し）取り込まず、理由を残して再 review に戻す。ほかの欄は合格の証跡と同じにするので、
/// `base_sha` の照合だけがこの証跡を止める。
#[test]
fn health_enforce_delivery_base_sha_mismatch_is_rejected() {
    let (store, dir, d) = merge_queued_without_evidence();
    let mut evidence = health_evidence(dir.path(), &d, &bundled(), health::HealthVerdict::Pass);
    evidence.base_sha = d.head.clone();
    record_health(&store, &d, evidence, &bundled());

    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("health の証跡の基底"), "{detail}");
    assert!(detail.contains(&d.head), "{detail}");
}

/// 不変条件: PolicyOnly の証跡は、G2 の承認が `(repo, target_sha, reviewed_sha)` に結び付くと event から確かめられ
/// ない限り取り込まない。今の event では承認は `Approval` の子 task の状態と、`ReviewVerdict` の自由文（repo も基底も
/// 持たない）にしか残らないので、人の承認の verdict が同じ review run にあっても通さない。
#[test]
fn health_enforce_delivery_policy_only_is_rejected_without_bound_approval() {
    use task_core::TaskStore;
    let (store, dir, d) = merge_queued_without_evidence();
    let evidence = health_evidence(
        dir.path(),
        &d,
        &bundled(),
        health::HealthVerdict::PolicyOnly,
    );
    // review は PolicyOnly の項目を合格として記録する（人の承認は G2 の policy_review_gate が求める）。
    let item = task_core::PhaseCheckResult {
        cmd: health::HEALTH_CHECK_ID.into(),
        pass: true,
        summary: review_item::HEALTH_POLICY_ONLY.into(),
        health: Some(Box::new(evidence)),
    };
    record_results(&store, &d, vec![item], &[], Some(&bundled()));
    store
        .append_event(
            d.task_id,
            &Event::ReviewVerdict {
                run_id: d.review_run.clone(),
                criterion_idx: 0,
                pass: true,
                reason: "human(approval task 01TEST): health:policy approved by a human: \
                         scripts/dev/repo-health/profiles/default.toml"
                    .into(),
            },
        )
        .unwrap();

    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("policy の変更だけ"), "{detail}");
}
