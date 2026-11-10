//! ADR 2026-11-10-health-generic-fixtures (3) の (e)・(f)（unit rs-stale）: Celeris でない isolated Git fixture と本物の
//! 検査器で、review の health の合格が古くなったら（候補の head・main 側の base・base 側の profile が変わったら）前の Pass を
//! 写さずに検査器を走らせ直すこと、reviewer の合格の再利用の経路でも health を飛ばさないこと、baseline・policy の改変と
//! 履歴の細工・候補が持ち込んだ検査器で合格しないことを確かめる（試験名は `health_generic_stale_`・
//! `health_generic_reviewer_reuse_`・`health_generic_baseline_weakening_not_pass`）。
//!
//! 土台は `health_generic.rs` の helper（同梱の本物の検査器・builder の CLI・候補の適用）を `pub(super)` で使い、写さない。
//! main を進める check は `target_sync.rs` の [`bump_main_writing`] と同じく git の plumbing で `refs/heads/main` だけを
//! 進める（作業ツリーは触らない）。外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の試験で足りない理由: `health_review/reuse_tests.rs` の `health_enforce_reviewer_reuse_*` は stub の検査器に
//! `review_item::evaluate_all` を直接当て、dispatcher の review の流れ（review の repair・main 同期・reviewer の合格の
//! 再利用）を通らない。`target_sync.rs` の reviewer の再利用の試験は health の当たらない repo で、`health_generic.rs` は
//! 1 回の候補の判定だけを見る。本物の検査器の合格が流れの中で古くなる場合はどれも確かめない。
use super::health_enforce::non_celeris_dispatcher;
use super::health_generic::{
    Fixture, REVIEW_PASS, apply_candidate, assert_dep_inversion_fail, assert_pass, blocking_rules,
    build_fixture, changed_paths, configure_real_bundled_checker, dep_inversion_rules,
    repair_classes, restore_base, review_health,
};
use super::target_sync::{bump_main_writing, reviewing_task, set_check};
use super::*;
use std::path::Path;
use task_core::required_checks::health::{CheckerId, HealthEvidence, HealthVerdict};

/// fixture の profile（builder の `PROFILE`。base 側の policy の file）。
const PROFILE: &str = "scripts/dev/repo-health/profiles/default.toml";

/// main（review の target）。review の repair はここの tree に戻す（main が進んだ後も候補の差分を消す）。
const MAIN: &str = "refs/heads/main";

/// `target_sync::reviewing_task` の acceptance[0]（候補が main の子孫）。
const DEFAULT_CHECK: &str = "git merge-base --is-ancestor main HEAD";

/// review の repair の key（`review_verdict.rs` の `repair-{n}`）のうち、試験が動作を決める数。
const REPAIRS: usize = 4;

/// 1 回の review の流れの結果（fixture・作業ツリー・flag の dir は試験の終わりまで持つ）。
struct Flow {
    _dirs: Vec<tempfile::TempDir>,
    fx: Fixture,
    repo: PathBuf,
    store: Arc<dyn TaskStore>,
    task: Task,
    events: Vec<Event>,
    status: Status,
}

/// main の fixture の profile で層 `app` の許可（`["core", "auth"]`）を空にした内容を出す command（policy の変更）。
fn narrow_app_layer() -> String {
    format!(r#"git show {MAIN}:{PROFILE} | sed 's/^allow = \["core", "auth"\]$/allow = []/'"#)
}

/// 1 回目の review の後に起こす変化（作業ツリーで、`flag` を受け取る）。
type Change = Box<dyn Fn(&Path, &Path) + Send + Sync>;

/// review の作り方。
struct Setup {
    kind: &'static str,
    variant: &'static str,
    /// acceptance に `Check::Reviewer` を足し、reviewer run は合格を返す。
    reviewer: bool,
    /// `Some` なら acceptance[0] を `test -e <flag> && <既定の check>` にする。1 回目の review はこの command だけが落ちて（health の
    /// 項目は記録される）review の repair（`repair-1`）に回り、その repair が変化を起こして flag を立てる。
    /// `None` なら acceptance[0] は既定の check（[`DEFAULT_CHECK`]）。
    change: Option<Change>,
    /// `Some((file, blob_cmd))` なら acceptance[0] を、1 回目の検査の最中にだけ main の `file` を `blob_cmd` の出力に
    /// 書き換える check（`target_sync.rs` の [`bump_main_writing`]）にする。
    bump: Option<(String, String)>,
}

impl Setup {
    fn new(kind: &'static str, variant: &'static str) -> Self {
        Self {
            kind,
            variant,
            reviewer: false,
            change: None,
            bump: None,
        }
    }

    fn after_first_review(mut self, change: impl Fn(&Path, &Path) + Send + Sync + 'static) -> Self {
        self.change = Some(Box::new(change));
        self
    }
}

/// fixture を作り、atomic の task（reviewing）の作業ツリーに `prepare` で候補を入れて review を起こし、止まるまで流す。
/// review の repair は、`repair-1` が `Setup.change`（あれば）を起こし、ほか（`change` の無い `repair-1` を含む）は
/// 作業ツリーを main の tree に戻す。
async fn review(setup: Setup, prepare: impl FnOnce(&Path, &Fixture)) -> Flow {
    let fixtures = tempfile::tempdir().unwrap();
    let repo = fixtures.path().join(setup.kind);
    let fx = build_fixture(setup.kind, setup.variant, &repo);
    let ws = tempfile::tempdir().unwrap();
    let flags = tempfile::tempdir().unwrap();
    let flag = flags.path().join("changed");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = reviewing_task(&repo, &store);
    if setup.change.is_some() {
        // merge-base の語を持つ check の不合格は局所の repair（MergeBase。ADR-0074 D6.2）に分類される。flag が無い間
        // （1 回目の review）だけ落ちる。
        set_check(
            &mut task,
            format!("test -e {} && {DEFAULT_CHECK}", flag.display()),
        );
    }
    if let Some((file, blob)) = &setup.bump {
        set_check(&mut task, bump_main_writing(&flag, file, blob));
    }
    let mut adapter = ParallelWuAdapter::new(Duration::from_millis(5));
    if setup.reviewer {
        task.acceptance.push(Criterion {
            text: "reviewer".into(),
            check: Check::Reviewer,
        });
        adapter = adapter.with_review(REVIEW_PASS);
    }
    store.insert(&task).unwrap();
    let change: Option<Arc<Change>> = setup.change.map(Arc::new);
    for n in 1..=REPAIRS {
        let (change, flag) = (change.clone(), flag.clone());
        adapter = adapter.with_action(&format!("repair-{n}"), move |dir| match &change {
            Some(change) if n == 1 => change(dir, &flag),
            _ => restore_base(dir, MAIN),
        });
    }
    let mut d = non_celeris_dispatcher(store.clone(), Arc::new(adapter), ws.path(), 1);
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), fx.base());
    prepare(&wt.dir, &fx);
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);
    let events = events_of(&store, task.id);
    let status = store.get(task.id).unwrap().unwrap().status;
    Flow {
        _dirs: vec![fixtures, ws, flags],
        fx,
        repo,
        store,
        task,
        events,
        status,
    }
}

/// fixture の候補をそのまま入れる。
fn candidate(dir: &Path, fx: &Fixture) {
    apply_candidate(dir, fx.head());
}

/// `cmd` を作業ツリー `dir` で `sh -c` で流す（main を進める git の plumbing。作業ツリーは触らない）。
fn sh(dir: &Path, cmd: &str) {
    let out = std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// review の health の項目。前の Pass（1 つ目）の後に検査器が走り直していなければ（写した・止まった）落とす。
fn health_twice(flow: &Flow) -> Vec<(bool, HealthEvidence)> {
    let items = review_health(&flow.events);
    assert!(
        items.len() >= 2,
        "the checker did not run again after the first Pass: {items:?} {:?}",
        flow.events
    );
    items
}

/// 検査器を走らせ直した証跡（前の Pass を写していない。写すと `reused_from` が付く。ADR health-enforcement-loop D2.5）。
fn assert_fresh(evidence: &HealthEvidence, bundled: &CheckerId) {
    assert_eq!(evidence.reused_from, None, "{evidence:?}");
    assert_eq!(evidence.checker.as_ref(), Some(bundled), "{evidence:?}");
}

/// flag を立て、main を空の commit（tree は同じ）で 1 つ進める。
fn advance_main_empty(dir: &Path, flag: &Path) {
    std::fs::write(flag, "").unwrap();
    sh(
        dir,
        "git update-ref refs/heads/main \"$(git commit-tree 'refs/heads/main^{tree}' -p refs/heads/main -m 'main advances')\"",
    );
}

/// 不変条件 (e): 良い変更の Pass の後に候補の head が変わり（層を宣言した profile で層の逆向きの依存を足す）、
/// 前の Pass を写さずに本物の検査器が走り直して Fail にする（`RequiredChecksEvaluated` が増える）。1 回目の review の
/// repair が dep-inversion の候補の commit（同じ base の別の fixture）を head に重ねる。2 回目の Fail の repair は main
/// の tree に戻す。
#[tokio::test]
async fn health_generic_stale_head_change_reevaluates() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    for kind in ["ts", "rust"] {
        let inverted = tempfile::tempdir().unwrap();
        let from = inverted.path().join(kind);
        let dep = build_fixture(kind, "dep-inversion", &from);
        let head = dep.head().to_string();
        let setup = Setup::new(kind, "good").after_first_review(move |dir, flag| {
            git_out(dir, &["fetch", "-q", &from.to_string_lossy(), &head]);
            git_out(dir, &["cherry-pick", &head]);
            std::fs::write(flag, "").unwrap();
        });
        let flow = review(setup, candidate).await;
        assert_eq!(flow.fx.base(), dep.base(), "the variants share the base");
        let items = health_twice(&flow);
        let (good, stale) = (&items[0], &items[1]);
        assert!(good.0, "{good:?}");
        assert_pass(&good.1, &bundled);
        assert_eq!(good.1.head_sha, flow.fx.head());
        assert!(!stale.0, "{stale:?}");
        assert_ne!(stale.1.head_tree, good.1.head_tree);
        assert_eq!(stale.1.base_sha, flow.fx.base());
        assert_fresh(&stale.1, &bundled);
        assert_dep_inversion_fail(kind, &stale.1, &bundled, &stale.1.head_sha.clone());
        assert_eq!(flow.status, Status::Done, "{kind}: {:?}", flow.events);
    }
}

/// 不変条件 (e): 良い変更の Pass の後に main 側が進めば（ここでは tree の同じ空の commit。同期の後も head の tree は
/// 変わらない）、同じ tree の前の Pass も写さず、新しい base で本物の検査器が走り直す。tree だけで再利用を決める欠けが
/// あれば、2 回目は `reused_from` 付きの写しになって落ちる。
#[tokio::test]
async fn health_generic_stale_base_change_reevaluates() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    for kind in ["ts", "rust"] {
        let setup = Setup::new(kind, "good").after_first_review(advance_main_empty);
        let flow = review(setup, candidate).await;
        let main = git_out(&flow.repo, &["rev-parse", MAIN]);
        assert_ne!(main, flow.fx.base(), "main did not advance");
        let items = health_twice(&flow);
        let (first, last) = (&items[0], &items[items.len() - 1]);
        assert!(first.0 && last.0, "{items:?}");
        assert_pass(&first.1, &bundled);
        assert_eq!(first.1.base_sha, flow.fx.base());
        assert_pass(&last.1, &bundled);
        assert_fresh(&last.1, &bundled);
        assert_eq!(last.1.base_sha, main);
        assert_eq!(last.1.head_tree, first.1.head_tree, "only the base moved");
        assert_ne!(last.1.head_sha, first.1.head_sha);
        assert_eq!(flow.status, Status::Done, "{kind}: {:?}", flow.events);
    }
}

/// 不変条件 (e): 良い変更の Pass の後に base 側の profile が変われば（main が層 `app` の許可を空にする）、前の Pass を
/// 写さず、新しい base の profile で本物の検査器が走り直す。良い変更の `app` → `core` の新しい import が、新しい policy
/// で fail 級の `deps.violation` になる（policy の指紋も変わる）。ts だけ: rust の良い変更は `Cargo.toml` の依存を
/// 足さないので、層の許可を変えても新しい辺が無い。
#[tokio::test]
async fn health_generic_stale_policy_change_reevaluates() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let narrow = narrow_app_layer();
    let setup = Setup::new("ts", "good").after_first_review(move |dir, flag| {
        // flag を立て、main の profile を書き換えた commit で main を進める（`target_sync.rs` の check と同じ plumbing）。
        sh(dir, &bump_main_writing(flag, PROFILE, &narrow));
    });
    let flow = review(setup, |dir, fx| {
        let profile = git_out(dir, &["show", &format!("{MAIN}:{PROFILE}")]);
        assert!(
            profile.contains(r#"allow = ["core", "auth"]"#),
            "the fixture profile changed; update the sed: {profile}"
        );
        candidate(dir, fx);
    })
    .await;
    let main = git_out(&flow.repo, &["rev-parse", MAIN]);
    let narrowed = git_out(&flow.repo, &["show", &format!("{MAIN}:{PROFILE}")]);
    assert!(narrowed.contains("allow = []"), "{narrowed}");
    let items = health_twice(&flow);
    let (good, stale) = (&items[0], &items[1]);
    assert!(good.0, "{good:?}");
    assert_pass(&good.1, &bundled);
    assert_eq!(good.1.base_sha, flow.fx.base());
    assert!(!stale.0, "{stale:?}");
    assert_fresh(&stale.1, &bundled);
    assert_eq!(stale.1.base_sha, main);
    assert_ne!(stale.1.policy_sha256, good.1.policy_sha256);
    assert_eq!(stale.1.verdict, HealthVerdict::Fail, "{stale:?}");
    assert_eq!(
        blocking_rules(&stale.1),
        vec!["deps.violation"],
        "{stale:?}"
    );
    assert_eq!(
        repair_classes(&flow.events).last(),
        Some(&"health:deps.violation".to_string()),
        "{:?}",
        flow.events
    );
    assert_eq!(flow.status, Status::Done, "{:?}", flow.events);
}

/// reviewer の合格の再利用（`ReviewerPassReused`）の持ち越し先 `(target_sha, reviewed_sha)`。
fn reused_bindings(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewerPassReused {
                target_sha,
                reviewed_sha,
                ..
            } => Some((target_sha.clone(), reviewed_sha.clone())),
            _ => None,
        })
        .collect()
}

/// 不変条件 (e)(g): reviewer の合格の再利用（reviewer の合格の後、検査の最中に main が候補の差分と重ならない file
/// だけ進む。ADR 2026-10-27 D2。`target_sync.rs` の再利用の試験と同じ流れ）の経路でも、health は飛ばさず、同期で
/// 変わった head で本物の検査器が走り直す（`reused_from` が無い）。再利用されるのは reviewer の合格だけ:
/// - main が文書だけ進む（ts・rust）: 新しい head の health は Pass で done。
/// - main が base 側の profile で層 `app` の許可を空にする（ts）: reviewer の合格は再利用されても、新しい head の
///   health は fail 級の `deps.violation` で Fail になり、再利用の合格で done にしない（task は failed。repair に
///   回らないことは欠け G4-S1。[`health_generic_reviewer_reuse_health_fail_routes_to_repair`]）。
///
/// 検査の最中に古くなった 1 回目の review は、health の項目を記録しない（同期をやり直す）。前の Pass を写す経路が
/// 無いことは `health_generic_stale_*_reevaluates` が確かめる。
#[tokio::test]
async fn health_generic_reviewer_reuse_reevaluates_health_on_new_head() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let narrow = narrow_app_layer();
    let cases = [
        (
            "ts",
            "docs/notes.md",
            "printf '# Notes\\n'".to_string(),
            true,
        ),
        (
            "rust",
            "docs/notes.md",
            "printf '# Notes\\n'".to_string(),
            true,
        ),
        ("ts", PROFILE, narrow, false),
    ];
    for (kind, file, blob, passes) in cases {
        let mut setup = Setup::new(kind, "good");
        setup.reviewer = true;
        setup.bump = Some((file.to_string(), blob));
        let flow = review(setup, candidate).await;
        let main = git_out(&flow.repo, &["rev-parse", MAIN]);
        assert_ne!(main, flow.fx.base(), "main did not advance");
        let reused = reused_bindings(&flow.events);
        assert_eq!(reused.len(), 1, "{kind}/{file}: {:?}", flow.events);
        let (target, reviewed) = &reused[0];
        assert_eq!(target, &main);
        assert_ne!(reviewed, flow.fx.head(), "the sync moved the head");
        let items = review_health(&flow.events);
        let (pass, evidence) = items
            .first()
            .unwrap_or_else(|| panic!("health was skipped: {:?}", flow.events));
        assert_eq!(
            (&evidence.base_sha, &evidence.head_sha),
            (target, reviewed),
            "the health item is bound to the reused candidate"
        );
        assert_fresh(evidence, &bundled);
        if passes {
            assert!(*pass, "{evidence:?}");
            assert_pass(evidence, &bundled);
            assert!(repair_classes(&flow.events).is_empty(), "{:?}", flow.events);
        } else {
            assert!(!*pass, "{evidence:?}");
            assert_eq!(evidence.verdict, HealthVerdict::Fail, "{evidence:?}");
            assert_eq!(blocking_rules(evidence), vec!["deps.violation"]);
            assert_ne!(flow.status, Status::Done, "{:?}", flow.events);
            continue;
        }
        assert_eq!(
            flow.status,
            Status::Done,
            "{kind}/{file}: {:?}",
            flow.events
        );
    }
}

/// 欠け G4-S1 の assert の文言（[`health_generic_reviewer_reuse_health_fail_routes_to_repair`] の `should_panic`）。
const G4_S1: &str =
    "G4-S1: a health Fail next to a reused reviewer pass did not go to the review repair";

/// 不変条件 (g)（ADR (3) の表: reviewer の合格の再利用の経路も dep-inversion を Fail にして repair に回す）: reviewer の
/// 合格を再利用した review で health が Fail になったら（再利用の後に main が base 側の profile で層 `app` の許可を空にし、
/// 良い変更の `app` → `core` の import が fail 級の `deps.violation` になる。候補が自分で層を逆向きにした場合は、決定的な
/// 項目が落ちるので reviewer run が起きず、再利用する合格が無い）、review の repair（class `health:deps.violation`）に
/// 回り、修復の後に done になる。
///
/// 既知の欠け G4-S1（進捗 `agent-docs/progress/2026-11-10-health-v2-g4/rs-stale.md` の未解決）: 決定的な項目（health）が
/// 落ちると reviewer の項目は「not evaluated: a deterministic check failed」の不合格になり（`review.rs` の ADR-0007 D5）、
/// `classify_review_failure` は修復できない reviewer の不合格と health の不合格の混在を Substantive にして、task を
/// failed（`review_fail`）にする。期待は弱めず、この欠けの assert の文言だけの `should_panic` にしてある。直ったら
/// panic しなくなって落ちるので、そのとき `should_panic` を外す。
#[tokio::test]
#[should_panic(
    expected = "G4-S1: a health Fail next to a reused reviewer pass did not go to the review repair"
)]
async fn health_generic_reviewer_reuse_health_fail_routes_to_repair() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let mut setup = Setup::new("ts", "good");
    setup.reviewer = true;
    setup.bump = Some((PROFILE.into(), narrow_app_layer()));
    let flow = review(setup, candidate).await;
    let reused = reused_bindings(&flow.events);
    assert_eq!(reused.len(), 1, "{:?}", flow.events);
    let items = review_health(&flow.events);
    let (pass, evidence) = items
        .first()
        .unwrap_or_else(|| panic!("health was skipped: {:?}", flow.events));
    assert!(!*pass, "{evidence:?}");
    assert_fresh(evidence, &bundled);
    assert_eq!(evidence.verdict, HealthVerdict::Fail, "{evidence:?}");
    assert_eq!(evidence.checker.as_ref(), Some(&bundled));
    assert_eq!(blocking_rules(evidence), vec!["deps.violation"]);
    assert_eq!(
        repair_classes(&flow.events),
        vec!["health:deps.violation".to_string()],
        "{G4_S1}: {:?}",
        flow.events
    );
    assert_eq!(flow.status, Status::Done, "{G4_S1}: {:?}", flow.events);
}

/// 候補の最初の health の証跡（review の health の項目の 1 つ目）。
fn first_health(flow: &Flow) -> (bool, HealthEvidence) {
    review_health(&flow.events)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the review did not evaluate health: {:?}", flow.events))
}

/// 不変条件 (f): baseline・policy の改変と履歴の細工と候補が持ち込んだ検査器で合格しない。
/// - baseline-rewrite・policy-exclude（dep-inversion に policy の改変を重ねた候補）: base の profile で評価するので
///   層の逆向きは残り、`policy.changed` も fail 級で、health は Fail（review の repair）。
/// - 同じ改変の policy の file だけの候補: health の項目は PolicyOnly（合格）でも、policy の関門（G2 の D6）が人の承認の
///   子 task を作って統合（done）を止める。
/// - history-smuggle: 候補の途中の commit を base にせず、base（main）との差分で層の逆向きが Fail。
/// - 候補が自分の `scripts/dev/repo-health/repo_health.py`（何もせず exit 0）を足しても、同梱の検査器が走って Fail。
#[tokio::test]
async fn health_generic_baseline_weakening_not_pass() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    for kind in ["ts", "rust"] {
        let inverted = tempfile::tempdir().unwrap();
        let inverted_repo = inverted.path().join(kind);
        let dep = build_fixture(kind, "dep-inversion", &inverted_repo);
        let inversion = changed_paths(&inverted_repo, &dep);
        let mut with_policy = dep_inversion_rules(kind);
        with_policy.push("policy.changed");
        with_policy.sort_unstable();
        for variant in ["baseline-rewrite", "policy-exclude"] {
            let flow = review(Setup::new(kind, variant), candidate).await;
            let (pass, evidence) = first_health(&flow);
            assert!(!pass, "{kind}/{variant}: {evidence:?}");
            assert_eq!(evidence.verdict, HealthVerdict::Fail, "{evidence:?}");
            assert_eq!(evidence.checker.as_ref(), Some(&bundled));
            assert_eq!(evidence.base_sha, flow.fx.base());
            assert_eq!(blocking_rules(&evidence), with_policy, "{evidence:?}");
            assert!(
                !repair_classes(&flow.events).is_empty(),
                "{:?}",
                flow.events
            );

            // 同じ改変の policy の file だけ（dep-inversion の差分を除いた path を候補の head から取る）。
            let policy_files: Vec<String> = changed_paths(&flow.repo, &flow.fx)
                .into_iter()
                .filter(|p| !inversion.contains(p))
                .collect();
            assert!(!policy_files.is_empty(), "{kind}/{variant}");
            let files = policy_files.clone();
            let only = review(Setup::new(kind, variant), move |dir, fx| {
                let mut args = vec!["checkout".to_string(), fx.head().to_string(), "--".into()];
                args.extend(files);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                git_out(dir, &args);
                git_out(dir, &["commit", "-q", "-m", "weaken the policy only"]);
            })
            .await;
            let (pass, evidence) = first_health(&only);
            assert!(pass, "{kind}/{variant}: {evidence:?}");
            assert_eq!(evidence.verdict, HealthVerdict::PolicyOnly, "{evidence:?}");
            assert_eq!(evidence.checker.as_ref(), Some(&bundled));
            assert_eq!(only.status, Status::Reviewing, "{:?}", only.events);
            let approvals: Vec<Task> = only
                .store
                .children(only.task.id)
                .unwrap()
                .into_iter()
                .filter(|c| {
                    policy_files
                        .iter()
                        .all(|f| c.objective.contains(f.as_str()))
                })
                .collect();
            assert_eq!(approvals.len(), 1, "{kind}/{variant}: {:?}", only.events);
            assert!(!approvals[0].status.is_terminal(), "{:?}", approvals[0]);
        }

        let smuggled = review(Setup::new(kind, "history-smuggle"), candidate).await;
        let middle = git_out(
            &smuggled.repo,
            &["rev-parse", &format!("{}^", smuggled.fx.head())],
        );
        assert_ne!(middle, smuggled.fx.base(), "the candidate has two commits");
        let (pass, evidence) = first_health(&smuggled);
        assert!(!pass, "{kind}: {evidence:?}");
        assert_eq!(evidence.base_sha, smuggled.fx.base());
        assert_dep_inversion_fail(kind, &evidence, &bundled, smuggled.fx.head());

        let own_checker = review(Setup::new(kind, "dep-inversion"), |dir, fx| {
            candidate(dir, fx);
            let script = dir.join("scripts/dev/repo-health/repo_health.py");
            std::fs::write(&script, "import sys\nsys.exit(0)\n").unwrap();
            git_out(dir, &["add", "-A"]);
            git_out(
                dir,
                &["commit", "-q", "-m", "bring an always-passing checker"],
            );
        })
        .await;
        // 同梱の検査器が候補の差分（持ち込んだ `.py` を含む）を評価する。profile の adapter に python が無いので、
        // 構造の rule は unsupported で verdict は Error（検査器の契約。合格にしない）。候補の検査器（exit 0）が
        // 走っていれば Pass になる。
        let (pass, evidence) = first_health(&own_checker);
        assert!(!pass, "{kind}: {evidence:?}");
        assert!(
            matches!(evidence.verdict, HealthVerdict::Fail | HealthVerdict::Error),
            "{evidence:?}"
        );
        assert_eq!(evidence.checker.as_ref(), Some(&bundled), "{evidence:?}");
        assert_ne!(
            own_checker.status,
            Status::Reviewing,
            "{:?}",
            own_checker.events
        );
    }
}
