//! `admit` の順序と組み合わせの試験（ADR 2026-11-14 dispatch-admission-order D6）。
//!
//! 既存の dispatcher の試験は 1 つの試験で 1 つの gate だけを止めるので、隣り合う 2 つの gate の順を入れ替えても合格する。
//! ここでは admit を直接呼び、両方が止める入力で先の gate が勝つこと、後ろの gate の fact を求めないこと
//! （副作用を持つ owner の処理を呼ばせないこと）、並列 WU の 2 本目以降が飛ばす gate の集合、planner run の組み合わせを確かめる。

use time::{Duration, OffsetDateTime};

use super::*;

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
    }
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
/// （ADR 2026-11-14 D5 の 1〜16。attempts が 0 なので再試行の backoff は求めない）。
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
        ]
    );
    let launch = launch_of(done);
    assert!(!launch.planner_run && !launch.cos_run);
    assert_eq!((launch.tier, launch.budget), (None, None));
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
    world.forced_unit = Some(AdmissionUnit::WorkUnit(Some(wu_budget)));
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
        ]
    );
    let launch = launch_of(done);
    assert!(!launch.cos_run && !launch.planner_run);
    assert_eq!(launch.budget, Some((50, 1800)));

    let mut world = open_world();
    world.forced_unit = Some(AdmissionUnit::WorkUnit(None));
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
            AdmissionUnit::WorkUnit(None),
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
    let launch = launch_of(done);
    assert_eq!(launch.tier, Some(Tier::Cheap));
    assert_eq!(launch.budget, Some((40, 1200)));
    assert!(!launch.planner_human_frontier);

    world.task_values = Some(AdmissionTaskValues {
        gate_mode: GateMode::On,
        execution: Some((ExecutionMode::Compound, GateSource::Policy)),
        tier_source: Some(TierSource::Human),
        tier: Tier::Frontier,
        planner_tier: Tier::Cheap,
        ..values()
    });
    let launch = launch_of(drive(&world).1);
    assert!(launch.planner_human_frontier);
    assert_eq!(launch.tier, Some(Tier::Frontier));

    let mut world = open_world();
    world.unit = Some(AdmissionUnit::WorkUnit(Some(WorkUnitBudget {
        max_turns: None,
        max_wall_secs: Some(90),
    })));
    world.task_values = Some(AdmissionTaskValues {
        budget: (45, 600),
        ..values()
    });
    let launch = launch_of(drive(&world).1);
    assert_eq!((launch.tier, launch.budget), (None, Some((45, 90))));

    world.unit = Some(AdmissionUnit::WorkUnit(None));
    let launch = launch_of(drive(&world).1);
    assert_eq!(launch.budget, Some((45, 1800)));
}
