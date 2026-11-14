//! ADR 2026-11-08-health-enforcement-loop D1.2・D2.6: delivery の merge candidate の照合（`check_required_evidence`）
//! が適用判定を呼び、`Applies` なら health の証跡を必須にする試験。同梱の検査器は `advance` の引数で渡す
//! （process の OnceLock `configure_bundled_checker` は呼ばない）。
use super::*;

/// 配送を 1 回進める。取り込み先は案件の repo 行（Celeris 自身の repo ではない）。ff したら `None`、ff せず
/// 再 review に戻したらその理由を返す。
pub(super) fn deliver(
    store: &task_core::SqliteStore,
    repo: &Path,
    d: &Delivery,
    bundled: Option<&CheckerId>,
) -> Option<String> {
    use task_core::DeliveryStore;
    register_repo(store, d, repo);
    let self_repo = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(self_repo.path(), &tmp.path().join("releases"));
    advance(store, &cfg, d, OffsetDateTime::now_utc(), bundled).unwrap();
    let main = sha(repo, "refs/heads/main").unwrap();
    if main == d.head {
        return None;
    }
    assert_eq!(main, d.base, "ff しないなら main は動かない");
    let after = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(after.state, State::Reviewing);
    assert_eq!(after.merge_candidate_sha, None);
    Some(after.detail)
}

/// `checker` の health の項目だけを持つ合格の証跡（定義指紋も `checker` で作る）を `d` に記録する。
pub(super) fn record_health(
    store: &task_core::SqliteStore,
    d: &Delivery,
    evidence: health::HealthEvidence,
    checker: &CheckerId,
) {
    record_results(store, d, vec![health_item(evidence)], &[], Some(checker));
}

/// 基底に検査器（`scripts/dev/repo-health/repo_health.py`）を持つ配送。main に検査器の commit を足し、
/// feature をその上の 1 commit にする（Celeris の repo と同じ形）。
fn merge_queued_with_base_checker() -> (task_core::SqliteStore, tempfile::TempDir, Delivery) {
    use task_core::DeliveryStore;
    let (store, dir, old) = merge_queued_without_evidence();
    let p = dir.path();
    fs::create_dir_all(p.join(health::CHECKER_DIR)).unwrap();
    fs::write(p.join(health::CHECKER_SCRIPT), "print('checker')\n").unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "checker"]).unwrap();
    let base = sha(p, "HEAD").unwrap();
    git_text(p, &["checkout", "feature"]).unwrap();
    git_text(p, &["reset", "--hard", &base]).unwrap();
    fs::write(p.join("feature"), "implemented on the checker base").unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "feature"]).unwrap();
    let head = sha(p, "HEAD").unwrap();
    git_text(p, &["checkout", "main"]).unwrap();
    let mut d = old.clone();
    d.base = base.clone();
    d.target_sha = Some(base);
    d.head = head.clone();
    d.reviewed_sha = Some(head.clone());
    d.merge_candidate_sha = Some(head);
    assert!(store.delivery_save(Some(&old), &d).unwrap());
    (store, dir, d)
}

type AppliesCase = (&'static str, fn(&task_core::SqliteStore, &Delivery));
type EvidenceMutation = (&'static str, fn(&mut health::HealthEvidence));

/// 不変条件: 適用判定が `Applies` の repo では、health の項目の証跡が無い合格（health 導入前の review が残した
/// 証跡・health の項目を欠く証跡・証跡の無い health の項目）で main に取り込まない。
#[test]
fn health_evidence_delivery_applies_requires_the_health_item() {
    let cases: [AppliesCase; 3] = [
        // health 導入前の review の証跡（定義指紋に検査器の版が無い）。
        ("定義指紋", |s, d| {
            record_results(s, d, Vec::new(), &[], None)
        }),
        // 定義指紋は今の検査器で作られているが、health の項目が無い。
        ("health の項目（", |s, d| {
            record_results(s, d, Vec::new(), &[], Some(&bundled()))
        }),
        // health の項目はあるが証跡が無い。
        ("health の項目に証跡がありません", |s, d| {
            let item = task_core::PhaseCheckResult::new(
                health::HEALTH_CHECK_ID.into(),
                true,
                String::new(),
            );
            record_results(s, d, vec![item], &[], Some(&bundled()))
        }),
    ];
    for (expected, setup) in cases {
        let (store, dir, d) = merge_queued_without_evidence();
        setup(&store, &d);
        let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
        assert!(detail.contains(expected), "{expected}: {detail}");
    }
}

/// 不変条件: health の証跡の `head_tree` が候補 `reviewed_sha` の tree と違えば取り込まない（別の候補を評価した
/// 証跡を同じ基底・候補の束縛に入れても通らない）。
#[test]
fn health_evidence_delivery_rejects_evidence_for_another_tree() {
    let (store, dir, d) = merge_queued_without_evidence();
    let mut evidence = health_evidence(dir.path(), &d, &bundled(), health::HealthVerdict::Pass);
    let base_tree = tree_of(dir.path(), &d.base);
    assert_ne!(base_tree, evidence.head_tree);
    evidence.head_tree = base_tree;
    record_health(&store, &d, evidence, &bundled());
    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("tree"), "{detail}");
}

/// 不変条件: health の証跡の検査器の版が今の適用判定の検査器と違えば取り込まない（同梱の検査器が更新された
/// 後に、古い版で評価した合格を使わない）。
#[test]
fn health_evidence_delivery_rejects_another_checker_version() {
    let (store, dir, d) = merge_queued_without_evidence();
    let older = CheckerId {
        source: health::CheckerSource::Bundled,
        sha256: "c".repeat(64),
    };
    // 定義指紋は今の版で作り、health の項目の検査器だけが古い版。
    record_health(
        &store,
        &d,
        health_evidence(dir.path(), &d, &older, health::HealthVerdict::Pass),
        &bundled(),
    );
    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("検査器の版"), "{detail}");
    // 古い版だけで揃えた証跡（定義指紋も古い版）も、今の版の定義指紋と一致しない。
    let (store, dir, d) = merge_queued_without_evidence();
    record_health(
        &store,
        &d,
        health_evidence(dir.path(), &d, &older, health::HealthVerdict::Pass),
        &older,
    );
    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("定義指紋"), "{detail}");
}

/// 不変条件: 基底に検査器がある repo では基底の検査器で照合し（D1.4・J3）、同梱の検査器で評価した証跡では
/// 取り込まない。基底の検査器の Pass なら取り込む。
#[test]
fn health_evidence_delivery_prefers_the_base_checker_over_the_bundled_one() {
    let (store, dir, d) = merge_queued_with_base_checker();
    let target = d.target_sha.clone().unwrap();
    let base = health::base_checker(&target, |args| git_text(dir.path(), args))
        .unwrap()
        .expect("基底に検査器がある");
    assert_eq!(base.source, health::CheckerSource::Base);
    record_evidence(&store, dir.path(), &d, &[]);
    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("定義指紋"), "{detail}");

    let (store, dir, d) = merge_queued_with_base_checker();
    record_health(
        &store,
        &d,
        health_evidence(dir.path(), &d, &base, health::HealthVerdict::Pass),
        &base,
    );
    assert_eq!(deliver(&store, dir.path(), &d, Some(&bundled())), None);
}

/// 不変条件: health の verdict が Pass でなければ取り込まない（Error・Fail・PolicyOnly。項目の `pass` が true
/// でも verdict で決まる）。dirty な作業ツリーの report と、report の digest の無い証跡も取り込まない（D2.7）。
#[test]
fn health_evidence_delivery_rejects_error_and_fail_evidence() {
    let cases: [EvidenceMutation; 6] = [
        ("Error", |e| e.verdict = health::HealthVerdict::Error),
        ("Fail", |e| e.verdict = health::HealthVerdict::Fail),
        ("policy の変更だけ", |e| {
            e.verdict = health::HealthVerdict::PolicyOnly
        }),
        ("Error", |e| e.verdict = health::HealthVerdict::Error),
        ("dirty", |e| e.reviewable = false),
        ("digest", |e| e.report_sha256.clear()),
    ];
    for (i, (expected, edit)) in cases.into_iter().enumerate() {
        let (store, dir, d) = merge_queued_without_evidence();
        let mut evidence = health_evidence(dir.path(), &d, &bundled(), health::HealthVerdict::Pass);
        edit(&mut evidence);
        let mut item = health_item(evidence);
        // 4 件目は項目の `pass` を true のまま残した Error（合否は verdict で決まる）。
        if i == 3 {
            item.pass = true;
        }
        record_results(&store, &d, vec![item], &[], Some(&bundled()));
        let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
        assert!(detail.contains(expected), "{i} {expected}: {detail}");
    }
}

/// 不変条件: 適用判定が `Unavailable`（基底にも同梱にも検査器が無い・基底の事実を読めない）なら、合格の証跡が
/// あっても取り込まない。読めない基底の事実は、同梱の検査器があっても合格にしない。
#[test]
fn health_evidence_delivery_unavailable_is_rejected() {
    // 同梱の検査器が無く、一時 repo の基底にも検査器が無い。
    for checker in [None, Some(bundled())] {
        let (store, dir, d) = merge_queued_without_evidence();
        match &checker {
            Some(checker) => record_health(
                &store,
                &d,
                health_evidence(dir.path(), &d, checker, health::HealthVerdict::Pass),
                checker,
            ),
            None => record_results(&store, &d, Vec::new(), &[], None),
        }
        let detail = deliver(&store, dir.path(), &d, None).expect("ff しない");
        assert!(detail.contains("checker_missing"), "{detail}");
    }
    // 基底の commit が repo に無い（事実を読めない）。同梱の検査器を渡しても Unavailable。
    let (store, dir, mut d) = merge_queued_without_evidence();
    d.target_sha = Some("0".repeat(40));
    record_evidence(&store, dir.path(), &d, &[]);
    let error = check_required_evidence(&store, dir.path(), &d, Some(&bundled()))
        .unwrap()
        .unwrap_err();
    assert!(error.contains("facts_unreadable"), "{error}");
}

/// 不変条件: Celeris でない repo（基底に profile も検査器も無い）でも、同梱の検査器の Pass の証跡があれば
/// 取り込む（適用判定は repo の事実だけで決まり、案件名・repo 名で外れない）。
#[test]
fn health_evidence_delivery_non_celeris_repo_passes_with_bundled_pass() {
    let (store, dir, d) = merge_queued_without_evidence();
    let target = d.target_sha.clone().unwrap();
    let in_base = |path: &str| {
        health::base_has_file(&target, path, |args| git_text(dir.path(), args)).unwrap()
    };
    assert!(!in_base(health::PROFILES_DIR) && !in_base(health::CHECKER_SCRIPT));
    record_health(
        &store,
        &d,
        health_evidence(dir.path(), &d, &bundled(), health::HealthVerdict::Pass),
        &bundled(),
    );
    assert_eq!(deliver(&store, dir.path(), &d, Some(&bundled())), None);
}

/// 不変条件: `mode = prototype` は `workspace.toml` の必須検査だけを外し、health の項目は外さない（ADR の
/// 「採らなかった: task の mode で外す案」）。
#[test]
fn health_evidence_delivery_prototype_still_requires_health() {
    use task_core::TaskStore;
    let (store, dir, d) = merge_queued_in_mode(task_core::TaskMode::Prototype);
    assert_eq!(
        store.get(d.task_id).unwrap().unwrap().mode,
        task_core::TaskMode::Prototype
    );
    record_results(&store, &d, Vec::new(), &[], None);
    let detail = deliver(&store, dir.path(), &d, Some(&bundled())).expect("ff しない");
    assert!(detail.contains("定義指紋"), "{detail}");

    let (store, dir, d) = merge_queued_in_mode(task_core::TaskMode::Prototype);
    record_evidence(&store, dir.path(), &d, &[]);
    assert_eq!(deliver(&store, dir.path(), &d, Some(&bundled())), None);
}
