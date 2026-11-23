//! ADR 2026-11-23-project-observability-publish D3.2〜D3.4 の純関数の試験（保証の表の `publish_manifest_`・
//! `publish_signoff_` の task-core 側）。base には検査の関数が無いので、どれも変更前は落ちる（compile しない）。

use serde_json::{Value, json};

use super::*;
use crate::strategy::RefStatus;

const PROJECT: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBW";
const OTHER: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBX";
const TASK: &str = "01M7XQ9R1EHA8GQ0TZVRM4FBVX";
const COMMIT: &str = "074f2e64ab13c0ffee0000000000000000000000";

/// 基準の manifest: 確認済みの claim（内部の参照・再現条件あり）と推定の claim（limitations に載る）。
fn baseline() -> Value {
    json!({
        "schema": SCHEMA,
        "project_id": PROJECT,
        "manifest_id": "paper-a",
        "version": 1,
        "supersedes": null,
        "idempotency_key": "publish:paper-a:v1",
        "source": {"kind": "celeris_task_run", "label": "run"},
        "actor": {"kind": "worker", "id": "run-1"},
        "source_revision": {"request_sha256": "0".repeat(64), "roadmap_version": null},
        "timestamp": "2026-11-23T00:00:00Z",
        "target": {"kind": "paper", "venue": "IEEE Access"},
        "acl": {"classification": "internal", "outbound": false},
        "claims": [
            {"id": "c1", "statement": "BenchFS は 2 倍速い", "confidence": "confirmed", "evidence": ["e1"]},
            {"id": "c2", "statement": "16 node でも同じ", "confidence": "estimated", "evidence": ["e2"]}
        ],
        "evidence": [
            {"id": "e1", "stance": "support", "statement": "測定の commit",
             "references": [{"kind": "commit", "repo": "agent-platform", "sha": COMMIT}]},
            {"id": "e2", "stance": "unknown", "statement": "文献",
             "references": [{"kind": "url", "url": "https://example.org/paper"}]}
        ],
        "reproduction": [
            {"id": "r1", "claims": ["c1"], "commands": ["bash run.sh"],
             "environment": {"repo": "agent-platform", "commit": COMMIT, "host_class": "sirius"},
             "data": [], "expected": "2 倍"}
        ],
        "limitations": [{"id": "l1", "statement": "16 node は未測定", "claims": ["c2"]}],
        "open_questions": [],
        "decision_state": "proposed"
    })
}

fn ctx() -> ManifestContext {
    ManifestContext {
        project_id: PROJECT.into(),
        artifact_name: artifact_name("paper-a", 1),
        key_sha256s: vec!["a".repeat(64)],
        task_manifest_ids: vec!["paper-a".into()],
        previous: None,
        references: vec![(
            Reference::Commit {
                repo: "agent-platform".into(),
                sha: COMMIT.into(),
            },
            RefStatus::Found {
                project_id: Some(PROJECT.into()),
            },
        )],
    }
}

fn check(doc: &Value, ctx: &ManifestContext) -> Vec<ManifestViolation> {
    validate_manifest(doc.to_string().as_bytes(), ctx)
}

fn codes(v: &[ManifestViolation]) -> Vec<(ManifestRule, &'static str)> {
    v.iter().map(|v| (v.rule, v.code)).collect()
}

#[test]
fn publish_manifest_valid_baseline_has_no_violations() {
    assert_eq!(check(&baseline(), &ctx()), vec![]);
}

#[test]
fn publish_manifest_claim_without_evidence_is_invalid() {
    let mut doc = baseline();
    doc["claims"][1]["evidence"] = json!([]);
    let v = check(&doc, &ctx());
    assert!(
        v.iter().any(|v| v.rule == ManifestRule::UnsupportedClaim
            && v.path == "claims.c2"
            && v.code == "publish_manifest_invalid"),
        "{v:?}"
    );
    assert_eq!(unsupported_claims(&v), vec!["c2".to_string()]);
    // 文書に無い証拠の id を引いても根拠にならない。
    let mut doc = baseline();
    doc["claims"][1]["evidence"] = json!(["e9"]);
    assert_eq!(
        unsupported_claims(&check(&doc, &ctx())),
        vec!["c2".to_string()]
    );
}

#[test]
fn publish_manifest_confirmed_claim_needs_internal_reference() {
    let mut doc = baseline();
    doc["evidence"][0]["references"] = json!([{"kind": "url", "url": "https://example.org/x"}]);
    let v = check(&doc, &ctx());
    assert!(
        v.iter()
            .any(|v| v.rule == ManifestRule::UnsupportedClaim && v.path == "claims.c1"),
        "{v:?}"
    );
}

#[test]
fn publish_manifest_confirmed_claim_needs_reproduction() {
    let mut doc = baseline();
    doc["reproduction"][0]["claims"] = json!([]);
    let v = check(&doc, &ctx());
    assert!(
        v.iter()
            .any(|v| v.rule == ManifestRule::UnsupportedClaim && v.detail.contains("reproduction")),
        "{v:?}"
    );
}

#[test]
fn publish_manifest_supported_and_refuted_claim_cannot_be_confirmed() {
    let mut doc = baseline();
    doc["evidence"].as_array_mut().expect("evidence").push(
        json!({"id": "e3", "stance": "refute", "statement": "再測定で 1.1 倍",
                     "references": [{"kind": "commit", "repo": "agent-platform", "sha": COMMIT}]}),
    );
    doc["claims"][0]["evidence"] = json!(["e1", "e3"]);
    let v = check(&doc, &ctx());
    assert!(codes(&v).contains(&(ManifestRule::Contradicted, "publish_manifest_invalid")));
    // 反証のある claim は limitations か未解決に載る（M6）。
    assert!(
        v.iter()
            .any(|v| v.rule == ManifestRule::HiddenUncertainty && v.path == "claims.c1")
    );
}

#[test]
fn publish_manifest_unconfirmed_claim_must_be_listed_in_limitations() {
    let mut doc = baseline();
    doc["limitations"] = json!([]);
    let v = check(&doc, &ctx());
    assert_eq!(
        codes(&v),
        vec![(ManifestRule::HiddenUncertainty, "publish_manifest_invalid")]
    );
    // 未解決の問い（id = claim の id）に載せても足りる。
    doc["open_questions"] =
        json!([{"id": "c2", "statement": "16 node は測るか", "blocking": false}]);
    let v = check(&doc, &ctx());
    assert!(
        v.iter().all(|v| v.rule != ManifestRule::HiddenUncertainty),
        "{v:?}"
    );
}

#[test]
fn publish_manifest_refute_evidence_cannot_be_removed_in_next_version() {
    let mut v1 = baseline();
    v1["evidence"].as_array_mut().expect("evidence").push(
        json!({"id": "e3", "stance": "refute", "statement": "16 node で 1.1 倍", "references": []}),
    );
    v1["claims"][1]["evidence"] = json!(["e2", "e3"]);
    let mut v2 = baseline();
    v2["version"] = json!(2);
    v2["idempotency_key"] = json!("publish:paper-a:v2");
    v2["supersedes"] =
        json!({"task_id": TASK, "name": "publish/paper-a/v1.json", "sha256": "a".repeat(64)});
    let mut c = ctx();
    c.artifact_name = artifact_name("paper-a", 2);
    c.previous = Some(v1.to_string().into_bytes());
    let v = check(&v2, &c);
    assert!(
        codes(&v).contains(&(
            ManifestRule::RefuteRemoved,
            "publish_manifest_refute_removed"
        )),
        "{v:?}"
    );
    // 書き換えも同じ。
    let mut v2b = v1.clone();
    v2b["version"] = json!(2);
    v2b["idempotency_key"] = json!("publish:paper-a:v2");
    v2b["supersedes"] = v2["supersedes"].clone();
    v2b["evidence"][2]["statement"] = json!("1.9 倍");
    assert!(
        check(&v2b, &c)
            .iter()
            .any(|v| v.rule == ManifestRule::RefuteRemoved)
    );
}

#[test]
fn publish_manifest_secret_in_body_is_invalid() {
    let mut doc = baseline();
    doc["limitations"][0]["statement"] = json!("password: hunter2 を使う");
    let v = check(&doc, &ctx());
    assert!(codes(&v).contains(&(ManifestRule::Secret, "publish_manifest_secret")));
}

#[test]
fn publish_manifest_signed_state_in_document_is_invalid() {
    let mut doc = baseline();
    doc["signoff"] = json!({"by": "human:rmaeda", "option": "sign"});
    let v = check(&doc, &ctx());
    assert!(
        v.iter()
            .any(|v| v.rule == ManifestRule::Shape && v.path == "signoff"),
        "{v:?}"
    );
    let mut doc = baseline();
    doc["decision_state"] = json!("signed");
    let v = check(&doc, &ctx());
    assert!(
        v.iter()
            .any(|v| v.code == "publish_manifest_invalid" && v.path == "decision_state"),
        "{v:?}"
    );
}

#[test]
fn publish_manifest_other_project_reference_is_invalid() {
    let mut c = ctx();
    c.references[0].1 = RefStatus::Found {
        project_id: Some(OTHER.into()),
    };
    let v = check(&baseline(), &c);
    assert!(codes(&v).contains(&(ManifestRule::OtherProject, "publish_manifest_other_project")));
    // 文書の案件が task の案件と違う。
    let mut doc = baseline();
    doc["project_id"] = json!(OTHER);
    assert!(
        check(&doc, &ctx())
            .iter()
            .any(|v| v.rule == ManifestRule::OtherProject && v.path == "project_id")
    );
    // 引けない参照は解決しない。
    let mut c = ctx();
    c.references[0].1 = RefStatus::Missing;
    assert!(
        check(&baseline(), &c)
            .iter()
            .any(|v| v.rule == ManifestRule::UnresolvedReference)
    );
}

#[test]
fn publish_manifest_same_version_other_sha_is_conflict() {
    let mut c = ctx();
    c.key_sha256s.push("b".repeat(64));
    let v = check(&baseline(), &c);
    assert_eq!(
        codes(&v),
        vec![(
            ManifestRule::Idempotency,
            "publish_manifest_idempotency_conflict"
        )]
    );
}

#[test]
fn publish_manifest_second_id_in_task_is_conflict() {
    let mut c = ctx();
    c.task_manifest_ids.push("paper-b".into());
    let v = check(&baseline(), &c);
    assert_eq!(
        codes(&v),
        vec![(ManifestRule::IdConflict, "publish_manifest_id_conflict")]
    );
}

#[test]
fn publish_manifest_limits_reject_long_commands_and_bad_commit() {
    let mut doc = baseline();
    doc["reproduction"][0]["commands"] = json!(["x".repeat(COMMAND_MAX_CHARS + 1)]);
    doc["reproduction"][0]["environment"]["commit"] = json!("ABC");
    let v = check(&doc, &ctx());
    assert_eq!(
        v.iter().filter(|v| v.rule == ManifestRule::Shape).count(),
        2,
        "{v:?}"
    );
    let big = vec![b' '; DOCUMENT_MAX_BYTES + 1];
    assert_eq!(validate_manifest(&big, &ctx()).len(), 1);
}

fn record(option: &str, by: &str, sha: &str) -> SignoffRecord {
    SignoffRecord {
        key: signoff_decision_key(1),
        question: signoff_question("paper-a", 1, sha),
        status: DecisionStatus::Answered,
        last_answer: Some((option.into(), by.into())),
    }
}

#[test]
fn publish_signoff_legacy_unprefixed_human_is_not_signoff() {
    let sha = "a".repeat(64);
    for by in [
        "human",
        "human:",
        LOCAL_HUMAN,
        "operator:claude-code",
        "mcp:chatgpt",
        "cos:01M5JJTVPW15CKFRCA0FAYCHK4",
        "run-01M84Q76RPS3T3E4RQYMDWGD7W",
    ] {
        assert!(!is_signoff_actor(by), "{by}");
        let r = record(SIGN_OPTION, by, &sha);
        assert_eq!(
            signoff_state("paper-a", 1, &sha, Some(&sha), Some(&r), false),
            SignoffState::Unsigned,
            "{by}"
        );
    }
    assert!(is_signoff_actor("human:web"));
}

#[test]
fn publish_signoff_state_follows_sha_version_and_answer() {
    let sha = "a".repeat(64);
    let other = "b".repeat(64);
    let signed = record(SIGN_OPTION, "human:web", &sha);
    assert_eq!(
        signoff_state("paper-a", 1, &sha, Some(&sha), Some(&signed), false),
        SignoffState::Signed
    );
    // 本文が変われば失効。
    assert_eq!(
        signoff_state("paper-a", 1, &sha, Some(&other), Some(&signed), false),
        SignoffState::Stale
    );
    // より新しい版の署名の決定があれば前の版の署名は上書きされた。
    assert_eq!(
        signoff_state("paper-a", 1, &sha, Some(&sha), Some(&signed), true),
        SignoffState::Superseded
    );
    // 問いの sha256 が記録と違う決定は署名に数えない。
    let wrong = record(SIGN_OPTION, "human:web", &other);
    assert_eq!(
        signoff_state("paper-a", 1, &sha, Some(&sha), Some(&wrong), false),
        SignoffState::Unsigned
    );
    let rejected = record(REJECT_OPTION, "human:web", &sha);
    assert_eq!(
        signoff_state("paper-a", 1, &sha, Some(&sha), Some(&rejected), false),
        SignoffState::Rejected
    );
    assert!(signoff_decision_key(64).len() <= 32);
    assert_eq!(
        signoff_subject(&signoff_question("paper-a", 3, &sha)),
        Some(SignoffSubject {
            manifest_id: "paper-a".into(),
            version: 3,
            sha256: sha.clone()
        })
    );
    assert_eq!(
        parse_artifact_name("publish/paper-a/v3.json"),
        Some(("paper-a".into(), 3))
    );
}
