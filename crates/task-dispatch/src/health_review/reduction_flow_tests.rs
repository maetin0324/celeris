//! ADR 2026-11-16-reduction-goal-delivery: 行数の削減を目的とする task の目的（削除の候補・基点の commit・削減の目標・
//! 禁止事項・不変条件）と差分が planner・WorkUnit・final reviewer に届き、本体の純増・移動だけ・測定の欠損は合格に
//! ならない。通常の機能追加には純減を求めない。
//!
//! - 本物の dispatcher が planner → WU → 統合 → final review を回す。案件の repo（一時 git repo）は
//!   `health_review_scenario` の stub の health 検査器（verdict pass）に、repo の本物の
//!   `scripts/dev/source-size-report.py` と toml を足したもの。
//! - 受け入れ条件の size check は CLAUDE.md の書き方（base の commit の script と toml を `git show` で取り出し、base と
//!   HEAD を同じ分類規則で数える）で、dispatcher が task の作業ツリーで実行する。
//! - adapter はどの run の prompt も本物の `claude_code::build_prompt` で組んで記録する。reviewer は全 criterion を合格、
//!   health の finding を 0 件と書く（甘い reviewer。それでも純減でない変更は合格にならないことを示す）。
//! - 外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の `health_review_scenario_`・`health_review_flow_` は削減の目的を持たない task だけを回し、WU の prompt に
//! task の目的の 1,500 文字より後ろが届くか、size check の不合格で reviewer の合格が覆らないかを見ない。

use super::*;

use super::health_review_scenario::{NORMAL_PATH, done, project_repo};
use crate::review_decision::REVIEWER_NOT_EVALUATED_REASON;

const MOVED_PATH: &str = "crates/foo/src/moved.rs";
/// base の本体の行数（`NORMAL_PATH` の関数の数）と、WU が足す・消す・動かす行数。
const BASE_FNS: usize = 40;
const DELTA: usize = 10;
const CANDIDATE: &str = "削除の候補 C9: crates/foo/src/lib.rs の f30〜f39（呼び元の無い 1 行転送）";
const TARGET: &str = "削減の目標: production 全言語の純減 10 行以上";
const FORBIDDEN: &str = "禁止事項: lease と auth の検査を弱めない";
const INVARIANT: &str = "不変条件: events-append-only を守る";
const MOVE_RULE: &str =
    "file の移動・名前の変更・試験や生成物や別言語への付け替え・何も消さない分割だけなら";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Change {
    /// 本体に `DELTA` 行を足す。
    Grow,
    /// 本体から `DELTA` 行を消す。
    Shrink,
    /// 本体の `DELTA` 行を別の file に動かす（純増 0）。
    Move,
}

impl Change {
    /// base の内容から決める WU の後の file の内容（何度当てても同じ。repair の再実行でも commit が空にならない）。
    fn files(self) -> Vec<(&'static str, String)> {
        let fns = |range: std::ops::Range<usize>| -> String {
            range.map(|i| format!("pub fn f{i}() {{}}\n")).collect()
        };
        match self {
            Change::Grow => vec![(NORMAL_PATH, fns(0..BASE_FNS + DELTA))],
            Change::Shrink => vec![(NORMAL_PATH, fns(0..BASE_FNS - DELTA))],
            Change::Move => vec![
                (NORMAL_PATH, fns(0..BASE_FNS - DELTA)),
                (MOVED_PATH, fns(BASE_FNS - DELTA..BASE_FNS)),
            ],
        }
    }
}

/// size check（CLAUDE.md の書き方）。base と HEAD の production 全言語の差が `min` 未満、または測定が落ちたら exit≠0。
fn size_check(base: &str, min: usize) -> String {
    format!(
        "set -e; B={base}; d=$(mktemp -d); trap 'rm -rf \"$d\"' EXIT; \
         git show $B:scripts/dev/source-size-report.py >\"$d/s.py\"; \
         git show $B:scripts/dev/source-size-report.toml >\"$d/s.toml\"; \
         for r in $B HEAD; do python3 \"$d/s.py\" --exclusive-classes --config \"$d/s.toml\" --rev $r \
         --format json --out \"$d/$r.json\"; done; \
         python3 -c 'import json,sys; b,h=(json.load(open(p))[\"class_totals\"][\"production\"] for p in sys.argv[1:3]); \
         print(\"production net reduction\", b-h); sys.exit(0 if b-h>=int(sys.argv[3]) else 1)' \
         \"$d/$B.json\" \"$d/HEAD.json\" {min}"
    )
}

/// 削減の依頼の目的。指示は 1,500 文字の背景の後ろに置く（長い依頼の目的と同じ形）。
fn reduction_objective(base: &str) -> String {
    format!(
        "{}\n\n## 削減の指示\n- {CANDIDATE}\n- 基点の commit: {base}\n- {TARGET}\n- {FORBIDDEN}\n- {INVARIANT}\n",
        "前回の削減は本体の行数を減らせなかった。".repeat(80)
    )
}

/// 計画（leaf 1 つ。write path は本体の 2 file）。leaf の目的は削減の指示を写さない（WU には task の目的の全文が届く）。
fn plan() -> String {
    let mut unit = super::tree::leaf("a", "s1", &[]);
    unit["context"] = serde_json::json!({"paths": [NORMAL_PATH, MOVED_PATH]});
    super::tree::v3_plan(vec![super::tree::stage("s1", false)], vec![unit])
}

/// どの run の prompt も本物の `build_prompt` で組んで記録し、planner・WU・reviewer の役を演じる。
struct ReductionAdapter {
    change: Change,
    runs: StdMutex<Vec<(task_worker::RunContext, String)>>,
}

impl ReductionAdapter {
    fn prompts(&self, pred: impl Fn(&task_worker::RunContext) -> bool) -> Vec<String> {
        let runs = self.runs.lock().unwrap();
        runs.iter()
            .filter(|(c, _)| pred(c))
            .map(|(_, p)| p.clone())
            .collect()
    }
}

#[async_trait]
impl WorkerAdapter for ReductionAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let prompt = task_worker::claude_code::build_prompt(
            &req.task,
            &req.context,
            run_id,
            &req.artifacts_rel(),
        );
        self.runs
            .lock()
            .unwrap()
            .push((req.context.clone(), prompt));
        std::fs::create_dir_all(&req.artifacts_dir).unwrap();
        if req.context.execution_planner.is_some() {
            std::fs::write(req.artifacts_dir.join("execution-plan.json"), plan()).unwrap();
            return Ok(done("planned"));
        }
        if let Some(review) = &req.context.review {
            let verdicts: Vec<_> = (review.criteria.iter())
                .map(|c| serde_json::json!({"criterion": c, "pass": true, "reason": "tests pass"}))
                .collect();
            let json = serde_json::json!({"verdicts": verdicts, "health": {"findings": []}});
            std::fs::write(
                req.artifacts_dir.join(crate::review::REVIEW_FILE_NAME),
                json.to_string(),
            )
            .unwrap();
            return Ok(done("reviewed"));
        }
        if req.context.work_unit.is_some() {
            let cwd = req
                .work_dir
                .clone()
                .unwrap_or_else(|| req.workspace.clone());
            let nested = cwd.join("repos").join("code");
            let tree = if nested.join(".git").exists() {
                nested
            } else {
                cwd
            };
            for (path, body) in self.change.files() {
                let file = tree.join(path);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(&file, body).unwrap();
            }
            if !git_out(&tree, &["status", "--porcelain"]).is_empty() {
                git_out(&tree, &["add", "-A"]);
                git_out(&tree, &["commit", "-q", "-m", "reduce"]);
            }
        }
        Ok(done("ok"))
    }
}

struct Flow {
    store: Arc<dyn TaskStore>,
    task: Task,
    adapter: Arc<ReductionAdapter>,
    base: String,
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
}

impl Flow {
    fn events(&self) -> Vec<Event> {
        (self.store.events_for(self.task.id).unwrap().into_iter())
            .map(|(_, e)| e)
            .collect()
    }
    fn status(&self) -> Status {
        self.store.get(self.task.id).unwrap().unwrap().status
    }
    /// criterion `idx` の `ReviewVerdict` の (pass, reason) の列。
    fn verdicts(&self, idx: usize) -> Vec<(bool, String)> {
        (self.events().into_iter())
            .filter_map(|e| match e {
                Event::ReviewVerdict {
                    criterion_idx,
                    pass,
                    reason,
                    ..
                } if criterion_idx == idx => Some((pass, reason)),
                _ => None,
            })
            .collect()
    }
    /// size check（criterion 0）の exit の列。
    fn size_exits(&self) -> Vec<Option<i32>> {
        (self.events().into_iter())
            .filter_map(|e| match e {
                Event::ReviewCommandChecked {
                    criterion_idx: 0,
                    exit,
                    ..
                } => Some(exit),
                _ => None,
            })
            .collect()
    }
    fn final_review_prompts(&self) -> Vec<String> {
        self.adapter.prompts(|c| c.review.is_some())
    }
}

/// 案件の repo を作り、`objective`・`acceptance_cmd`（`None` なら削減の size check）の task を止まるまで回す。
/// `measure_base` は size check の基点（`None` なら本体を置いた commit）。
async fn run(change: Change, reduction: bool, measure_base: Option<&str>) -> Flow {
    let repo = project_repo(true);
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/dev");
    let dev = repo.path().join("scripts/dev");
    for name in ["source-size-report.py", "source-size-report.toml"] {
        std::fs::copy(src.join(name), dev.join(name)).unwrap();
    }
    let lib = repo.path().join(NORMAL_PATH);
    std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
    let body: String = (0..BASE_FNS)
        .map(|i| format!("pub fn f{i}() {{}}\n"))
        .collect();
    std::fs::write(&lib, body).unwrap();
    git_out(repo.path(), &["add", "-A"]);
    git_out(repo.path(), &["commit", "-q", "-m", "base"]);
    let base = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let (project, repos) =
        project_with_repos(&store, &[("code", repo.path(), task_core::RepoKind::Git)]);
    let mut task = compound_task(repo.path());
    task.project_id = Some(project);
    task.repos = repos.iter().map(RepoRef::of).collect();
    if reduction {
        task.objective = reduction_objective(&base);
        task.acceptance[0] = task_core::Criterion {
            text: "production 全言語の純減が 10 行以上（size check）".into(),
            check: Check::Command {
                cmd: size_check(measure_base.unwrap_or(&base), DELTA),
                expect_exit: 0,
            },
        };
    } else {
        task.objective = "Add the export functions f40..f49.".into();
    }
    task.acceptance.push(task_core::Criterion {
        text: "the change keeps one source of truth".into(),
        check: Check::Reviewer,
    });
    store.insert(&task).unwrap();
    let adapter = Arc::new(ReductionAdapter {
        change,
        runs: StdMutex::new(Vec::new()),
    });
    let mut d = worktree_dispatcher(store.clone(), adapter.clone(), root.path(), None);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.limits = super::tree::tree_limits();
    run_until_idle(&mut d, 400).await;
    Flow {
        store,
        task,
        adapter,
        base,
        _repo: repo,
        _root: root,
    }
}

/// size check が `exit` で落ち、reviewer の合格と health の pass があっても review は不合格で、task は done にならない。
/// `exit` は純減の不足なら 1（比較の python）、測定の欠損なら 128（`git show`）。
fn assert_size_check_fails(flow: &Flow, exit: i32) {
    let exits = flow.size_exits();
    assert!(
        !exits.is_empty() && exits.iter().all(|e| *e == Some(exit)),
        "{exits:?}"
    );
    let size = flow.verdicts(0);
    assert!(
        !size.is_empty() && size.iter().all(|(pass, _)| !pass),
        "{size:?}"
    );
    let reviewer = flow.verdicts(1);
    assert!(
        !reviewer.is_empty()
            && reviewer
                .iter()
                .all(|(pass, reason)| !pass && reason == REVIEWER_NOT_EVALUATED_REASON),
        "the reviewer does not overturn a failed size check: {reviewer:?}"
    );
    assert!(
        flow.final_review_prompts().is_empty(),
        "no reviewer run after a failed size check"
    );
    assert_ne!(flow.status(), Status::Done);
}

/// (1) 本体の純増: health の stub は verdict pass、reviewer は全 criterion を合格と書くが、size check が落ちて review は
/// 不合格になる。
#[tokio::test]
async fn reduction_flow_growth_with_health_pass_fails_review() {
    let flow = run(Change::Grow, true, None).await;
    assert_size_check_fails(&flow, 1);
}

/// (2) 真正の削減: size check が合格し、reviewer の合格で task は done になる。(6) planner・WU・reviewer の prompt に
/// 削減の指示（1,500 文字より後ろ）と規約 11 が届き、reviewer には size check の結果と差分（target・candidate の SHA
/// と消した行）が届く。
#[tokio::test]
async fn reduction_flow_real_reduction_passes_and_delivers_the_goal() {
    let flow = run(Change::Shrink, true, None).await;
    assert_eq!(flow.status(), Status::Done, "{:#?}", flow.events());
    assert_eq!(flow.size_exits().last(), Some(&Some(0)));
    assert!(flow.verdicts(0).iter().all(|(pass, _)| *pass));
    assert!(flow.verdicts(1).last().is_some_and(|(pass, _)| *pass));

    let objective = reduction_objective(&flow.base);
    let late = objective.find(FORBIDDEN).unwrap();
    assert!(
        objective[..late].chars().count() > 1500,
        "the instructions sit after the first 1,500 characters"
    );
    let instructions = [
        CANDIDATE, &flow.base, TARGET, FORBIDDEN, INVARIANT, MOVE_RULE,
    ];
    let planner = flow.adapter.prompts(|c| c.execution_planner.is_some());
    let unit = flow.adapter.prompts(|c| c.work_unit.is_some());
    let reviewer = flow.final_review_prompts();
    for (role, prompts) in [
        ("planner", &planner),
        ("work unit", &unit),
        ("reviewer", &reviewer),
    ] {
        assert!(!prompts.is_empty(), "no {role} run");
        for prompt in prompts {
            for needle in instructions {
                assert!(
                    prompt.contains(needle),
                    "the {role} prompt lacks {needle:?}"
                );
            }
        }
    }
    let cmd = size_check(&flow.base, DELTA);
    let review = (flow.adapter.runs.lock().unwrap().iter())
        .find_map(|(c, _)| c.review.clone())
        .unwrap();
    let health = review.health.expect("the final review carries the diff");
    let repo = &health.repos[0];
    assert!(
        repo.diff
            .paths
            .iter()
            .any(|p| p.path == NORMAL_PATH && p.deletions == DELTA as u64 && p.insertions == 0),
        "{:?}",
        repo.diff
    );
    for prompt in &reviewer {
        assert!(
            prompt.contains(&format!("pass=true command `{cmd}`")),
            "{prompt}"
        );
        assert!(prompt.contains(&format!("- target_sha: `{}`", repo.target_sha)));
        assert!(prompt.contains(&format!("- reviewed_sha: `{}`", repo.reviewed_sha)));
        assert!(prompt.contains(&format!("`{NORMAL_PATH}`（+0・-{DELTA}）")));
    }
}

/// (3) 移動だけ: 本体の 10 行を別の file に動かしても production 全言語の純減は 0 で、size check が落ちる。
#[tokio::test]
async fn reduction_flow_move_only_fails_review() {
    let flow = run(Change::Move, true, None).await;
    assert_size_check_fails(&flow, 1);
}

/// (4) 測定の欠損: 基点の commit に測定の script が無い（`git show` が落ちる）と、削減が真正でも size check は合格に
/// ならず、reviewer の合格に届かない（fail closed）。
#[tokio::test]
async fn reduction_flow_missing_measurement_fails_closed() {
    let probe = project_repo(true);
    let root_commit = git_out(probe.path(), &["rev-list", "--max-parents=0", "HEAD"]);
    let flow = run(Change::Shrink, true, Some(&root_commit)).await;
    assert_size_check_fails(&flow, 128);
}

/// (5) 通常の機能追加: 削減を求めない task は本体が増えても従来どおり done になる（純減を求めない）。
#[tokio::test]
async fn reduction_flow_feature_task_is_not_held_to_net_reduction() {
    let flow = run(Change::Grow, false, None).await;
    assert_eq!(flow.status(), Status::Done, "{:#?}", flow.events());
    assert!(flow.verdicts(1).last().is_some_and(|(pass, _)| *pass));
    assert!(
        flow.final_review_prompts()
            .iter()
            .all(|p| !p.contains(CANDIDATE))
    );
}
