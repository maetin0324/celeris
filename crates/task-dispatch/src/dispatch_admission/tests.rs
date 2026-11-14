//! `admit` の順序と組み合わせの試験（ADR 2026-11-14 dispatch-admission-order D6）。
//!
//! 既存の dispatcher の試験は 1 つの試験で 1 つの gate だけを止めるので、隣り合う 2 つの gate の順を入れ替えても合格する。
//! ここでは admit を直接呼び、両方が止める入力で先の gate が勝つこと、後ろの gate の fact を求めないこと
//! （副作用を持つ owner の処理を呼ばせないこと）、並列 WU の 2 本目以降が飛ばす gate の集合、planner run の組み合わせ、
//! 置き場所の gate（write-set・クラスタ・作業場所・WU の用意・disk）の順と止め方を確かめる。

use time::{Duration, OffsetDateTime};

use super::*;
use crate::run_supervisor::RunKey;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + Duration::seconds(secs)
}

fn values() -> AdmissionTaskValues {
    AdmissionTaskValues {
        gate_mode: GateMode::Shadow,
        execution: None,
        tree_child: false,
        route: None,
        tier_source: None,
        tier: Tier::Standard,
        budget: (10, 600),
        planner_tier: Tier::Standard,
        planner_budget: (40, 1200),
        attempts: 0,
    }
}

/// どの gate も止めない答え（1 本目の atomic task）。各試験は止めたい gate の答えだけを変える。
fn open_world() -> AdmissionInput {
    AdmissionInput {
        forced_unit: None,
        knowledge_inputs_ready: Some(true),
        task_runs: Some(0),
        cos_run: Some(false),
        capacity_admits: Some(true),
        just_aborted: Some(false),
        eligible: Some(true),
        disk_critical: Some(false),
        disk_watch_paths: Some("/local".to_string()),
        assigned: Some(true),
        execution_gate_applied: true,
        unit: Some(AdmissionUnit::Atomic),
        decision_self_hold: Some(false),
        design_review_hold: Some(false),
        tree_limit_hold: Some(false),
        task_values: Some(values()),
        retry_backoff: Some(BackoffClock {
            until: at(100),
            now: at(200),
        }),
        infra_backoff: Some(None),
        infra_backoff_cleared: false,
        browser_prereq_needed: Some(false),
        browser_prereq_hold: Some(false),
        place: open_place(),
    }
}

/// 置き場所の gate がどれも止めない答え（Local の作業場所、接続済みのクラスタ、用意できる WU、disk は判定しない）。
fn open_place() -> AdmissionPlace {
    AdmissionPlace {
        run_override_applied: false,
        write_set: Some(WriteSetAdmission::Admit(None)),
        cluster: Some(AdmissionCluster::Local),
        cluster_cooldown: Some(ClusterCooldown {
            until: None,
            now: Instant::now(),
        }),
        cluster_in_use: Some(0),
        cluster_connected: Some(Some(true)),
        cluster_auto_connect: Some(Some(Ok(()))),
        cluster_marked_connected: false,
        task_dir_resolved: Some(true),
        wu_prepare_backoff: Some(None),
        wu_workspace_ready: Some(true),
        disk: Some(None),
        wu_scope_env_ready: Some(true),
    }
}

fn remote() -> AdmissionCluster {
    AdmissionCluster::Remote {
        id: "c1".to_string(),
        host: "h1".to_string(),
        concurrency: 2,
    }
}

fn work_unit(phased: bool) -> AdmissionUnit {
    AdmissionUnit::WorkUnit {
        budget: None,
        phased,
    }
}

fn cluster_stop(unavailable: Option<&str>, waiting: bool, detail: &str) -> AdmissionProposal {
    AdmissionProposal::Stop(AdmissionStop::Cluster {
        unavailable: unavailable.map(str::to_string),
        waiting,
        wait: TaskWait::Cluster {
            cluster_id: "c1".to_string(),
            detail: detail.to_string(),
        },
    })
}

/// 求められた予算の上書き（tier と、turn と壁時計）。
fn run_override(asked: &[AdmissionFact]) -> (Option<Tier>, Option<(u32, u64)>) {
    asked
        .iter()
        .find_map(|f| match f {
            AdmissionFact::RunOverride { tier, budget } => Some((*tier, *budget)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("予算の上書きを求めなかった: {asked:?}"))
}

/// dispatch_one の代わりに、admit が求めた fact だけを `world` から 1 つずつ写して admit を呼び直す。
/// 求められた順の fact の列と、最後の止め方か起動を返す。
fn drive(world: &AdmissionInput) -> (Vec<AdmissionFact>, AdmissionProposal) {
    let mut input = AdmissionInput {
        forced_unit: world.forced_unit,
        ..AdmissionInput::default()
    };
    let mut asked = Vec::new();
    loop {
        let fact = match admit(&input) {
            AdmissionProposal::Need(fact) => fact,
            done => return (asked, done),
        };
        assert!(!asked.contains(&fact), "{fact:?} を 2 回求めた");
        asked.push(fact);
        match fact {
            AdmissionFact::KnowledgeInputs => {
                input.knowledge_inputs_ready = world.knowledge_inputs_ready
            }
            AdmissionFact::TaskRuns => input.task_runs = world.task_runs,
            AdmissionFact::CosRun => input.cos_run = world.cos_run,
            AdmissionFact::Capacity { .. } => input.capacity_admits = world.capacity_admits,
            AdmissionFact::JustAborted => input.just_aborted = world.just_aborted,
            AdmissionFact::Eligible => input.eligible = world.eligible,
            AdmissionFact::DiskCritical => input.disk_critical = world.disk_critical,
            AdmissionFact::DiskWatchPaths => {
                input.disk_watch_paths = world.disk_watch_paths.clone()
            }
            AdmissionFact::Assignee => input.assigned = world.assigned,
            AdmissionFact::ExecutionGate => input.execution_gate_applied = true,
            AdmissionFact::Unit => input.unit = world.unit,
            AdmissionFact::DecisionSelfHold => input.decision_self_hold = world.decision_self_hold,
            AdmissionFact::DesignReviewHold => input.design_review_hold = world.design_review_hold,
            AdmissionFact::TreeLimit => input.tree_limit_hold = world.tree_limit_hold,
            AdmissionFact::TaskValues => input.task_values = world.task_values,
            AdmissionFact::RetryBackoff => input.retry_backoff = world.retry_backoff,
            AdmissionFact::InfraBackoff => input.infra_backoff = world.infra_backoff,
            AdmissionFact::ClearInfraBackoff => input.infra_backoff_cleared = true,
            AdmissionFact::BrowserPrereq { .. } => {
                input.browser_prereq_needed = world.browser_prereq_needed
            }
            AdmissionFact::BrowserPrereqHold => {
                input.browser_prereq_hold = world.browser_prereq_hold
            }
            AdmissionFact::RunOverride { .. } => input.place.run_override_applied = true,
            AdmissionFact::WriteSet { .. } => input.place.write_set = world.place.write_set.clone(),
            AdmissionFact::Cluster => input.place.cluster = world.place.cluster.clone(),
            AdmissionFact::ClusterState(step) => {
                let (to, from) = (&mut input.place, &world.place);
                match step {
                    ClusterFact::Cooldown => to.cluster_cooldown = from.cluster_cooldown,
                    ClusterFact::Load => to.cluster_in_use = from.cluster_in_use,
                    ClusterFact::Connected => to.cluster_connected = from.cluster_connected,
                    ClusterFact::AutoConnect => {
                        to.cluster_auto_connect = from.cluster_auto_connect.clone()
                    }
                    ClusterFact::MarkConnected => to.cluster_marked_connected = true,
                }
            }
            AdmissionFact::TaskDir => input.place.task_dir_resolved = world.place.task_dir_resolved,
            AdmissionFact::WorkUnit(step) => {
                let (to, from) = (&mut input.place, &world.place);
                match step {
                    WorkUnitFact::PrepareBackoff => to.wu_prepare_backoff = from.wu_prepare_backoff,
                    WorkUnitFact::Workspace => to.wu_workspace_ready = from.wu_workspace_ready,
                    WorkUnitFact::ScopeEnv => to.wu_scope_env_ready = from.wu_scope_env_ready,
                }
            }
            AdmissionFact::Disk => input.place.disk = world.place.disk.clone(),
        }
    }
}

fn launch_of(proposal: AdmissionProposal) -> AdmissionLaunch {
    match proposal {
        AdmissionProposal::Launch(launch) => launch,
        other => panic!("起動へ進まなかった: {other:?}"),
    }
}

/// 不変条件: どの gate も止めない 1 本目の task では、fact を移行前の dispatch_one と同じ順で 1 回ずつ求める
/// （ADR 2026-11-14 D5 の 1〜16 と、Local の atomic の run の置き場所の gate）。
/// 既存の dispatcher の試験は順を見ないので、gate の入れ替えを検出できない。
#[test]
fn admit_first_pass_asks_facts_in_the_dispatch_order() {
    let mut world = open_world();
    world.task_values = Some(AdmissionTaskValues {
        attempts: 1,
        ..values()
    });
    world.infra_backoff = Some(Some(BackoffClock {
        until: at(100),
        now: at(200),
    }));
    world.browser_prereq_needed = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(
        asked,
        vec![
            AdmissionFact::KnowledgeInputs,
            AdmissionFact::TaskRuns,
            AdmissionFact::CosRun,
            AdmissionFact::Capacity { cos: false },
            AdmissionFact::JustAborted,
            AdmissionFact::Eligible,
            AdmissionFact::DiskCritical,
            AdmissionFact::Assignee,
            AdmissionFact::ExecutionGate,
            AdmissionFact::Unit,
            AdmissionFact::DecisionSelfHold,
            AdmissionFact::DesignReviewHold,
            AdmissionFact::TreeLimit,
            AdmissionFact::TaskValues,
            AdmissionFact::RetryBackoff,
            AdmissionFact::InfraBackoff,
            AdmissionFact::ClearInfraBackoff,
            AdmissionFact::BrowserPrereq { planner_run: false },
            AdmissionFact::BrowserPrereqHold,
            AdmissionFact::RunOverride {
                tier: None,
                budget: None,
            },
            AdmissionFact::WriteSet { planner_run: false },
            AdmissionFact::Cluster,
            AdmissionFact::TaskDir,
            AdmissionFact::Disk,
        ]
    );
    let launch = launch_of(done);
    assert!(!launch.planner_run && !launch.cos_run);
}

/// 不変条件: 隣り合う 2 つの gate が両方止める入力では、今の順の先の gate が勝ち、後ろの gate の fact を求めない。
/// 組: 同じ task の run → 並列度の枠、並列度の枠 → 打ち切ったばかりの run、eligible → disk の critical、
/// disk の critical → 担当（担当の決定は書き戻しか blocked の副作用を持つ）。
/// 既存の試験は 1 つの gate だけを止めるので、この組の入れ替えを検出できない。
#[test]
fn admit_adjacent_capacity_and_host_gates_keep_the_first_stop() {
    let mut world = open_world();
    world.task_runs = Some(1);
    world.capacity_admits = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert_eq!(asked.last(), Some(&AdmissionFact::TaskRuns));

    let mut world = open_world();
    world.capacity_admits = Some(false);
    world.just_aborted = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Wait(TaskWait::Capacity {
            scope: CapacityScope::Global,
            id: None,
        }))
    );
    assert!(!asked.contains(&AdmissionFact::JustAborted));

    let mut world = open_world();
    world.eligible = Some(false);
    world.disk_critical = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert!(!asked.contains(&AdmissionFact::DiskCritical));

    let mut world = open_world();
    world.disk_critical = Some(true);
    world.assigned = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Wait(TaskWait::DiskCritical {
            path: "/local".to_string(),
        }))
    );
    assert!(!asked.contains(&AdmissionFact::Assignee));
}

/// 不変条件: CoS の対話 run は CoS の枠で数え、止めたら scope が CoS の待ちを記録する（ADR-0089）。
/// 担当が決まらなければ execution gate を当てず、WU の gate も求めない。
/// 既存の試験は CoS の枠の満杯と担当の不在を別々に見るので、枠の引数と担当の後の gate の省き方を固定しない。
#[test]
fn admit_counts_cos_runs_and_stops_before_the_execution_gate_without_assignee() {
    let mut world = open_world();
    world.cos_run = Some(true);
    world.capacity_admits = Some(false);
    let (asked, done) = drive(&world);
    assert!(asked.contains(&AdmissionFact::Capacity { cos: true }));
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Wait(TaskWait::Capacity {
            scope: CapacityScope::Cos,
            id: None,
        }))
    );

    let mut world = open_world();
    world.assigned = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert_eq!(asked.last(), Some(&AdmissionFact::Assignee));
}

/// 不変条件: 決定の待ちが止めたら設計 review の待ち（設計 review の run を起こしうる）を求めない。木の上限は WU の gate の
/// 種類より先に見る。統合・最終 review の gate は木の上限を通った後で止め方になる。
/// 既存の試験は決定の待ちと設計 review の待ちを同時に立てないので、この順の入れ替えを検出できない。
#[test]
fn admit_holds_and_tree_limit_come_before_the_unit_kind() {
    let mut world = open_world();
    world.decision_self_hold = Some(true);
    world.design_review_hold = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert!(!asked.contains(&AdmissionFact::DesignReviewHold));
    assert!(!asked.contains(&AdmissionFact::TreeLimit));

    let mut world = open_world();
    world.design_review_hold = Some(true);
    world.tree_limit_hold = Some(true);
    let (asked, _) = drive(&world);
    assert_eq!(asked.last(), Some(&AdmissionFact::DesignReviewHold));

    let mut world = open_world();
    world.unit = Some(AdmissionUnit::Planner { replan: true });
    world.tree_limit_hold = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert!(!asked.contains(&AdmissionFact::TaskValues));

    for (unit, how) in [
        (
            AdmissionUnit::StartIntegration,
            AdmissionStop::StartIntegration,
        ),
        (AdmissionUnit::FinalReview, AdmissionStop::StartFinalReview),
        (AdmissionUnit::Skip, AdmissionStop::Quiet),
    ] {
        let mut world = open_world();
        world.unit = Some(unit);
        let (asked, done) = drive(&world);
        assert_eq!(done, AdmissionProposal::Stop(how));
        assert_eq!(asked.last(), Some(&AdmissionFact::TreeLimit));
    }
}

/// 不変条件: 再試行の backoff は infra の backoff より先に見る。infra の backoff の期限の前は止め、browser の前提
/// （止めたら task を遷移させる）を求めない。期限を過ぎていれば解除を求めてから進む。attempts が 0 なら再試行の
/// backoff を求めない。
/// 既存の試験は再試行と infra の backoff を同時に立てないので、この順と解除の位置を検出できない。
#[test]
fn admit_retry_backoff_wins_over_infra_backoff_and_both_precede_browser() {
    let mut world = open_world();
    world.task_values = Some(AdmissionTaskValues {
        attempts: 2,
        ..values()
    });
    world.retry_backoff = Some(BackoffClock {
        until: at(300),
        now: at(200),
    });
    world.infra_backoff = Some(Some(BackoffClock {
        until: at(400),
        now: at(200),
    }));
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Backoff {
            until: at(300),
            cause: BackoffCause::Retry,
        })
    );
    assert!(!asked.contains(&AdmissionFact::InfraBackoff));

    world.retry_backoff = Some(BackoffClock {
        until: at(100),
        now: at(200),
    });
    world.browser_prereq_needed = Some(true);
    world.browser_prereq_hold = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Backoff {
            until: at(400),
            cause: BackoffCause::Infra,
        })
    );
    assert!(!asked.contains(&AdmissionFact::ClearInfraBackoff));
    assert!(
        !asked
            .iter()
            .any(|f| matches!(f, AdmissionFact::BrowserPrereq { .. }))
    );

    let mut world = open_world();
    world.retry_backoff = Some(BackoffClock {
        until: at(300),
        now: at(200),
    });
    world.browser_prereq_needed = Some(true);
    world.browser_prereq_hold = Some(true);
    let (asked, done) = drive(&world);
    assert!(!asked.contains(&AdmissionFact::RetryBackoff));
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert_eq!(asked.last(), Some(&AdmissionFact::BrowserPrereqHold));
}

/// 不変条件: 並列 WU の 2 本目以降は、同じ task の run・CoS の対話 run か・担当・execution gate・WU の gate の選択・
/// 決定と設計 review の待ち・再試行と infra の backoff・browser の前提を求めず、それらが止める値でも起動へ進む。
/// 知識整理の入力・並列度の枠（CoS でないとして数える）・打ち切ったばかりの run・eligible・disk の critical・木の上限は求める。
/// 既存の試験は 2 本目以降で 1 本目だけの gate が止める値を立てないので、飛ばす gate の集合の変化を検出できない。
#[test]
fn admit_second_parallel_unit_skips_only_the_first_pass_gates() {
    let wu_budget = WorkUnitBudget {
        max_turns: Some(50),
        max_wall_secs: None,
    };
    let mut world = open_world();
    world.forced_unit = Some(AdmissionUnit::WorkUnit {
        budget: Some(wu_budget),
        phased: false,
    });
    world.task_runs = Some(3);
    world.cos_run = Some(true);
    world.assigned = Some(false);
    world.unit = Some(AdmissionUnit::Skip);
    world.decision_self_hold = Some(true);
    world.design_review_hold = Some(true);
    world.task_values = Some(AdmissionTaskValues {
        attempts: 4,
        ..values()
    });
    world.retry_backoff = Some(BackoffClock {
        until: at(900),
        now: at(200),
    });
    world.infra_backoff = Some(Some(BackoffClock {
        until: at(900),
        now: at(200),
    }));
    world.browser_prereq_needed = Some(true);
    world.browser_prereq_hold = Some(true);
    let (asked, done) = drive(&world);
    assert_eq!(
        asked,
        vec![
            AdmissionFact::KnowledgeInputs,
            AdmissionFact::Capacity { cos: false },
            AdmissionFact::JustAborted,
            AdmissionFact::Eligible,
            AdmissionFact::DiskCritical,
            AdmissionFact::TreeLimit,
            AdmissionFact::TaskValues,
            AdmissionFact::RunOverride {
                tier: None,
                budget: Some((50, 1800)),
            },
            AdmissionFact::WriteSet { planner_run: false },
            AdmissionFact::Cluster,
            AdmissionFact::TaskDir,
            AdmissionFact::WorkUnit(WorkUnitFact::PrepareBackoff),
            AdmissionFact::Disk,
            AdmissionFact::WorkUnit(WorkUnitFact::ScopeEnv),
        ]
    );
    let launch = launch_of(done);
    assert!(!launch.cos_run && !launch.planner_run);

    let mut world = open_world();
    world.forced_unit = Some(work_unit(false));
    world.disk_critical = Some(true);
    let (_, done) = drive(&world);
    assert!(matches!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Wait(TaskWait::DiskCritical { .. }))
    ));
    world.disk_critical = Some(false);
    world.tree_limit_hold = Some(true);
    let (_, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
}

fn planner_case(
    unit: AdmissionUnit,
    gate_mode: GateMode,
    execution: Option<(ExecutionMode, GateSource)>,
    tree_child: bool,
    route: Option<(Route, bool)>,
) -> AdmissionLaunch {
    let mut world = open_world();
    world.unit = Some(unit);
    world.task_values = Some(AdmissionTaskValues {
        gate_mode,
        execution,
        tree_child,
        route,
        ..values()
    });
    launch_of(drive(&world).1)
}

/// 不変条件: planner run は、replan か、計画の無い run で直行経路でなく gate の判定が compound で、`[execution] gate` が
/// on・shadow で人が明示・木の子 task のどれかのときだけ（ADR-0072 D14/D17・ADR-0074 F3・ADR-0079 D4 (1)・ADR-0124 D3）。
/// 既存の試験は組み合わせを 1 つずつしか作らないので、条件の 1 つを落としても他の試験が通る組がある。
#[test]
fn admit_planner_run_combination_matches_the_dispatch_rules() {
    let compound_human = Some((ExecutionMode::Compound, GateSource::Human));
    let compound_policy = Some((ExecutionMode::Compound, GateSource::Policy));
    let atomic_human = Some((ExecutionMode::Atomic, GateSource::Human));
    let a = AdmissionUnit::Atomic;
    let cases = [
        // (unit, gate, 判定, 木の子, route, planner run か)
        (a, GateMode::On, compound_policy, false, None, true),
        (a, GateMode::On, atomic_human, false, None, false),
        (a, GateMode::On, None, false, None, false),
        (a, GateMode::Shadow, compound_human, false, None, true),
        (a, GateMode::Shadow, compound_policy, false, None, false),
        (a, GateMode::Off, compound_human, false, None, false),
        (a, GateMode::Off, compound_policy, true, None, true),
        (a, GateMode::Off, atomic_human, true, None, false),
        (
            a,
            GateMode::On,
            compound_policy,
            false,
            Some((Route::Direct, false)),
            false,
        ),
        (
            a,
            GateMode::On,
            compound_policy,
            false,
            Some((Route::Direct, true)),
            true,
        ),
        (
            a,
            GateMode::On,
            compound_policy,
            false,
            Some((Route::Planned, false)),
            true,
        ),
        (
            work_unit(false),
            GateMode::On,
            compound_policy,
            true,
            None,
            false,
        ),
        (
            AdmissionUnit::Planner { replan: true },
            GateMode::Off,
            None,
            false,
            Some((Route::Direct, false)),
            true,
        ),
        (
            AdmissionUnit::Planner { replan: false },
            GateMode::Off,
            None,
            false,
            None,
            false,
        ),
        (
            AdmissionUnit::Planner { replan: false },
            GateMode::On,
            compound_policy,
            false,
            None,
            true,
        ),
    ];
    for (unit, gate, execution, tree_child, route, want) in cases {
        let launch = planner_case(unit, gate, execution, tree_child, route);
        assert_eq!(
            launch.planner_run, want,
            "{unit:?} {gate:?} {execution:?} tree_child={tree_child} {route:?}"
        );
        assert_eq!(
            launch.replan,
            unit == AdmissionUnit::Planner { replan: true }
        );
        assert_eq!(launch.direct_route, route == Some((Route::Direct, false)));
    }
}

/// 不変条件: planner run は `[execution.planner]` の tier と予算で走り、人が `tier:frontier` を明示した task は
/// frontier で走る。WU の run は WU の予算、書かなければ `max(task の予算, 30 turn・1800 秒)`。atomic の run は上書きしない。
/// browser の前提の判定には planner run かを渡す。
/// 既存の試験は planner の予算と WU の予算を別の試験で見るので、上書きの取り違えを 1 か所で固定しない。
#[test]
fn admit_overrides_the_run_budget_by_run_kind() {
    let mut world = open_world();
    world.task_values = Some(AdmissionTaskValues {
        gate_mode: GateMode::On,
        execution: Some((ExecutionMode::Compound, GateSource::Policy)),
        planner_tier: Tier::Cheap,
        ..values()
    });
    world.browser_prereq_needed = Some(false);
    let (asked, done) = drive(&world);
    assert!(asked.contains(&AdmissionFact::BrowserPrereq { planner_run: true }));
    assert_eq!(run_override(&asked), (Some(Tier::Cheap), Some((40, 1200))));
    assert!(!launch_of(done).planner_human_frontier);

    world.task_values = Some(AdmissionTaskValues {
        gate_mode: GateMode::On,
        execution: Some((ExecutionMode::Compound, GateSource::Policy)),
        tier_source: Some(TierSource::Human),
        tier: Tier::Frontier,
        planner_tier: Tier::Cheap,
        ..values()
    });
    let (asked, done) = drive(&world);
    assert!(launch_of(done).planner_human_frontier);
    assert_eq!(run_override(&asked).0, Some(Tier::Frontier));

    let mut world = open_world();
    world.unit = Some(AdmissionUnit::WorkUnit {
        budget: Some(WorkUnitBudget {
            max_turns: None,
            max_wall_secs: Some(90),
        }),
        phased: false,
    });
    world.task_values = Some(AdmissionTaskValues {
        budget: (45, 600),
        ..values()
    });
    let (asked, done) = drive(&world);
    launch_of(done);
    assert_eq!(run_override(&asked), (None, Some((45, 90))));

    world.unit = Some(work_unit(false));
    let (asked, done) = drive(&world);
    launch_of(done);
    assert_eq!(run_override(&asked).1, Some((45, 1800)));
}

/// 不変条件: 置き場所の gate は、予算の上書き → write-set → クラスタの解決 → （remote なら）cooldown → 並列度 →
/// 接続の確認 → 自動接続 → 接続済みにする → 作業場所の dir → WU の用意の backoff → （phase のある WU なら）WU の
/// worktree → disk の見込み → （Local の WU なら）scope env の順に求める。remote の WU は scope env を、phase の無い WU は
/// WU の worktree を、planner run は disk の見込みを求めない。
/// 既存の dispatcher の試験は 1 つの試験で 1 つの gate の結果だけを見るので、求める順と、どの run の種類で gate を
/// 省くかの入れ替えを検出できない。
#[test]
fn admit_place_facts_follow_the_dispatch_order() {
    let mut world = open_world();
    world.unit = Some(work_unit(true));
    world.place.cluster = Some(remote());
    world.place.cluster_connected = Some(None);
    let (asked, done) = drive(&world);
    let at = asked
        .iter()
        .position(|f| matches!(f, AdmissionFact::RunOverride { .. }))
        .unwrap_or_else(|| panic!("予算の上書きを求めなかった: {asked:?}"));
    let state = AdmissionFact::ClusterState;
    let wu = AdmissionFact::WorkUnit;
    assert_eq!(
        asked[at + 1..],
        [
            AdmissionFact::WriteSet { planner_run: false },
            AdmissionFact::Cluster,
            state(ClusterFact::Cooldown),
            state(ClusterFact::Load),
            state(ClusterFact::Connected),
            state(ClusterFact::AutoConnect),
            state(ClusterFact::MarkConnected),
            AdmissionFact::TaskDir,
            wu(WorkUnitFact::PrepareBackoff),
            wu(WorkUnitFact::Workspace),
            AdmissionFact::Disk,
        ]
    );
    launch_of(done);

    world.place.cluster = Some(AdmissionCluster::Local);
    world.unit = Some(work_unit(false));
    let (asked, _) = drive(&world);
    assert!(
        !asked
            .iter()
            .any(|f| matches!(f, AdmissionFact::ClusterState(_)))
    );
    assert!(!asked.contains(&wu(WorkUnitFact::Workspace)));
    assert_eq!(asked.last(), Some(&wu(WorkUnitFact::ScopeEnv)));

    let mut world = open_world();
    world.task_values = Some(AdmissionTaskValues {
        gate_mode: GateMode::On,
        execution: Some((ExecutionMode::Compound, GateSource::Policy)),
        ..values()
    });
    world.place.disk = Some(Some(Admission::Wait(TaskWait::DiskCritical {
        path: "/local".to_string(),
    })));
    let (asked, done) = drive(&world);
    assert!(asked.contains(&AdmissionFact::WriteSet { planner_run: true }));
    assert!(!asked.contains(&AdmissionFact::Disk));
    assert!(launch_of(done).planner_run);
}

/// 不変条件: 予算の上書きを当ててから write-set の gate を求め、write-set の保留はクラスタの解決（設定に無いクラスタの
/// unroutable・担当が道具を持たないときの blocked）より先に勝つ。クラスタの解決で止めたら cooldown 以降と作業場所の dir
/// （root を固定する）を求めない。
/// 既存の試験は write-set の保留とクラスタの不備を同時に立てないので、この順の入れ替えを検出できない。
#[test]
fn admit_write_set_hold_wins_over_the_cluster_resolution() {
    let mut world = open_world();
    let blocker = RunKey {
        task: task_core::TaskId::new(),
        work_unit: None,
    };
    world.place.write_set = Some(WriteSetAdmission::Hold { blocker });
    world.place.cluster = Some(AdmissionCluster::NotConfigured);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::Quiet));
    assert_eq!(
        asked.last(),
        Some(&AdmissionFact::WriteSet { planner_run: false })
    );
    assert!(matches!(
        asked[asked.len() - 2],
        AdmissionFact::RunOverride { .. }
    ));

    world.place.write_set = Some(WriteSetAdmission::Admit(None));
    let (asked, done) = drive(&world);
    let wait = TaskWait::Unroutable {
        detail: "no such cluster in the config".to_string(),
    };
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::ClusterNotConfigured { wait })
    );
    assert_eq!(asked.last(), Some(&AdmissionFact::Cluster));

    let cluster = "c9".to_string();
    world.place.cluster = Some(AdmissionCluster::AssigneeLacksTool {
        cluster: cluster.clone(),
    });
    world.place.task_dir_resolved = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::ClusterToolMissing { cluster })
    );
    assert!(!asked.contains(&AdmissionFact::TaskDir));
}

/// 不変条件: remote のクラスタは cooldown → 並列度 → 接続の順に見て、先に止めた gate の待ちを返す。cooldown と接続の
/// 失敗はクラスタの待ちに数え（`note_cluster_waiting`）、並列度は数えない。接続の確認が通れば自動接続を試さず、
/// 期限を過ぎた cooldown は止めない。
/// 既存の試験は cooldown・並列度・接続を別々の試験で 1 つずつ止めるので、この順と待ちの数え方の入れ替えを検出できない。
#[test]
fn admit_cluster_cooldown_beats_concurrency_and_connection() {
    let now = Instant::now();
    let later = now + std::time::Duration::from_secs(60);
    let mut world = open_world();
    world.place.cluster = Some(remote());
    world.place.cluster_cooldown = Some(ClusterCooldown {
        until: Some(later),
        now,
    });
    world.place.cluster_in_use = Some(2);
    world.place.cluster_connected = Some(Some(false));
    let (asked, done) = drive(&world);
    assert_eq!(done, cluster_stop(None, true, "cooldown"));
    assert_eq!(
        asked.last(),
        Some(&AdmissionFact::ClusterState(ClusterFact::Cooldown))
    );

    world.place.cluster_cooldown = Some(ClusterCooldown {
        until: Some(now),
        now: later,
    });
    let (asked, done) = drive(&world);
    assert_eq!(done, cluster_stop(None, false, "concurrency full"));
    assert_eq!(
        asked.last(),
        Some(&AdmissionFact::ClusterState(ClusterFact::Load))
    );

    world.place.cluster_in_use = Some(1);
    world.place.cluster_connected = Some(Some(true));
    world.place.cluster_auto_connect = Some(None);
    let (asked, done) = drive(&world);
    launch_of(done);
    assert!(!asked.contains(&AdmissionFact::ClusterState(ClusterFact::AutoConnect)));
}

/// 不変条件: 接続が無いクラスタは自動接続を 1 回試し、失敗の詳細か接続のフックが無いことを理由にクラスタを使えないと
/// 記録して待つ。そのとき作業場所の dir（root を固定する）を求めない。自動接続できたら接続済みにしてから進む。
/// 既存の試験は自動接続の成功と失敗を別々に見るので、止め方の取り違えと、止めた後に作業場所を用意する入れ替えを検出できない。
#[test]
fn admit_cluster_connection_failure_stops_before_the_workspace() {
    let mut world = open_world();
    world.place.cluster = Some(remote());
    world.place.cluster_connected = Some(None);
    world.place.cluster_auto_connect = Some(Some(Err("boom".to_string())));
    let (asked, done) = drive(&world);
    let detail = "auto-connect failed: boom";
    assert_eq!(done, cluster_stop(Some(detail), true, detail));
    assert!(!asked.contains(&AdmissionFact::TaskDir));
    assert!(!asked.contains(&AdmissionFact::ClusterState(ClusterFact::MarkConnected)));

    world.place.cluster_auto_connect = Some(None);
    let (_, done) = drive(&world);
    let reason = "no ssh ControlMaster connection to c1 (host h1)";
    assert_eq!(
        done,
        cluster_stop(Some(reason), true, "no ssh ControlMaster connection")
    );

    world.place.cluster_auto_connect = Some(Some(Ok(())));
    world.place.task_dir_resolved = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::NoTaskDir));
    let marked = asked
        .iter()
        .position(|f| *f == AdmissionFact::ClusterState(ClusterFact::MarkConnected));
    let dir = asked.iter().position(|f| *f == AdmissionFact::TaskDir);
    assert!(marked.is_some() && marked < dir, "{asked:?}");
}

/// 不変条件: WU の run では、作業場所の dir → 用意の backoff → WU の worktree → disk の見込み → scope env の順に先の
/// gate が勝つ。用意の backoff の中は worktree を用意せず、worktree か scope env を用意できなければ WU の用意の失敗
/// （2 本目以降かを持つ）で止め、disk の見込みの待ちは scope env より先に勝つ。
/// 既存の試験は worktree の失敗・disk の待ち・scope env の失敗を同時に立てないので、この順の入れ替えを検出できない。
#[test]
fn admit_workspace_and_disk_stops_keep_the_dispatch_order() {
    let disk_wait = TaskWait::DiskForecast {
        need_bytes: 1,
        detail: "reserve".to_string(),
    };
    let mut world = open_world();
    world.unit = Some(work_unit(true));
    world.place.task_dir_resolved = Some(false);
    world.place.wu_prepare_backoff = Some(Some((2, at(500))));
    let (_, done) = drive(&world);
    assert_eq!(done, AdmissionProposal::Stop(AdmissionStop::NoTaskDir));

    world.place.task_dir_resolved = Some(true);
    world.place.wu_workspace_ready = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::WuPrepareBackoff {
            failures: 2,
            retry_at: at(500),
        })
    );
    assert!(!asked.contains(&AdmissionFact::WorkUnit(WorkUnitFact::Workspace)));

    world.place.wu_prepare_backoff = Some(None);
    world.place.disk = Some(Some(Admission::Wait(disk_wait.clone())));
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::WuPrepareFailed { forced: false })
    );
    assert!(!asked.contains(&AdmissionFact::Disk));
    world.forced_unit = world.unit;
    let (_, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::WuPrepareFailed { forced: true })
    );

    world.forced_unit = None;
    world.place.wu_workspace_ready = Some(true);
    world.place.wu_scope_env_ready = Some(false);
    let (asked, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::Wait(disk_wait))
    );
    assert!(!asked.contains(&AdmissionFact::WorkUnit(WorkUnitFact::ScopeEnv)));

    world.place.disk = Some(None);
    let (_, done) = drive(&world);
    assert_eq!(
        done,
        AdmissionProposal::Stop(AdmissionStop::WuPrepareFailed { forced: false })
    );

    world.place.cluster = Some(remote());
    let (asked, done) = drive(&world);
    launch_of(done);
    assert!(!asked.contains(&AdmissionFact::WorkUnit(WorkUnitFact::ScopeEnv)));
}
