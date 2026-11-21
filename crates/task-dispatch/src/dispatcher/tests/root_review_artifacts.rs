//! ADR 2026-11-21-root-review-work-unit-artifacts: 最終 review の `ArtifactExists` が名前の照合に使う成果物の候補
//! （`crate::review::artifact_candidates`）。
//!
//! 守る不変条件:
//! - 計画を持つ task の最終 review は、同じ task の WorkUnit の run が出した `ArtifactProduced` を名前で見つける
//!   （本番 2026-11-20 task 01M7MZTKBPZPQE4XN451F8X84F seq 2075: `final-report.md` は WU `finish` の run が
//!   `wu/finish/artifacts/final-report.md` に出したが、review は `artifacts/final-report.md` を探して落ちた）。
//! - 候補は対象 task の events と WorkUnit の行だけから作る。他の task（子・兄弟・他の案件）の成果物は候補にしない。
//! - workspace の外を指す path（絶対 path・`..`・外への symlink）の候補は `review_task` が使わない。
//! - 計画の無い task の候補と判定は変わらない（review する run の成果物だけ）。
//!
//! 既存の試験で検出できない理由: `review/tests.rs` の `artifact_exists_*` は `review_task` に `produced` を直接渡すので、
//! dispatcher がどの run の成果物を `produced` に入れるかを確かめない。dispatcher の試験
//! （`tick_and_dispatch.rs` の `a_standalone_task_keeps_the_plain_artifacts_dir` など）は計画の無い task だけで、WU の run
//! の成果物を root の review が探す経路を通らない。

use super::*;

use task_worker::LocalWorkspace;

const REVIEWED_RUN: &str = "run-root-reviewed";

fn artifact(name: &str, path: &str) -> ArtifactRef {
    ArtifactRef {
        name: name.to_string(),
        path: path.to_string(),
        sha256: String::new(),
        kind: "md".to_string(),
        declared: false,
    }
}

/// `keys` の WorkUnit を持つ計画を採用し、WorkUnit の行を返す（本物の `adopt_plan` で行を作る）。
fn adopt_units(store: &SqliteStore, task_id: TaskId, keys: &[&str]) -> Vec<task_core::WorkUnitRow> {
    let spec = task_core::ExecutionPlanSpec {
        stages: Vec::new(),
        units: Vec::new(),
        decisions: Vec::new(),
        schema: task_core::EXECUTION_PLAN_SCHEMA.to_string(),
        rationale: "root review artifacts".to_string(),
        work_units: keys.iter().map(|k| wu_spec(k, &[])).collect(),
        phases: Vec::new(),
        children: Vec::new(),
        design_brief: None,
    };
    task_ops::execution::adopt_plan(
        store,
        task_id,
        spec,
        task_core::PlanOrigin::Fixture,
        None,
        task_core::ExecutionLimits::default(),
        OffsetDateTime::now_utc(),
    )
    .expect("adopt plan");
    store.work_units_for(task_id).unwrap()
}

/// WorkUnit `unit` の run `run_id` が走り、成果物 `produced` を出したことを `task_id` の events に残す
/// （dispatcher の `WorkUnitTransitioned{to: running}` と `ArtifactProduced` と同じ形）。
fn unit_run_produced(
    store: &SqliteStore,
    task_id: TaskId,
    unit: &task_core::WorkUnitRow,
    run_id: &str,
    produced: &[ArtifactRef],
) {
    store
        .append_event(
            task_id,
            &Event::WorkUnitTransitioned {
                work_unit_id: unit.id.clone(),
                key: unit.key.clone(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Running,
                reason: "dispatch".to_string(),
                run_id: Some(run_id.to_string()),
            },
        )
        .unwrap();
    for a in produced {
        store
            .append_event(
                task_id,
                &Event::ArtifactProduced {
                    run_id: run_id.to_string(),
                    artifact: a.clone(),
                },
            )
            .unwrap();
    }
}

fn run_produced(store: &SqliteStore, task_id: TaskId, run_id: &str, produced: &[ArtifactRef]) {
    for a in produced {
        store
            .append_event(
                task_id,
                &Event::ArtifactProduced {
                    run_id: run_id.to_string(),
                    artifact: a.clone(),
                },
            )
            .unwrap();
    }
}

/// dispatcher の最終 review と同じ読み方（対象 task の events と WorkUnit の行）で候補を作る。
fn candidates(store: &SqliteStore, task_id: TaskId) -> Vec<ArtifactRef> {
    crate::review::artifact_candidates(
        &store.events_for(task_id).unwrap(),
        &store.work_units_for(task_id).unwrap(),
        REVIEWED_RUN,
    )
}

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// `task` の最初の受け入れ条件（`ArtifactExists`）の判定。`review_spawn` と同じく、workspace は task の dir、
/// 成果物の dir は `<dir>/artifacts`。
async fn artifact_verdict(task: &Task, dir: &std::path::Path, produced: &[ArtifactRef]) -> Verdict {
    let ws = LocalWorkspace::new(dir);
    let outcome = review_task(
        task,
        &ws,
        dir,
        &dir.join("artifacts"),
        produced,
        Duration::from_secs(5),
        ReviewExtras::default(),
    )
    .await;
    outcome.verdicts.into_iter().next().unwrap()
}

/// (1) WU の run だけが出した成果物（`wu/<key>/artifacts/<name>`）を、root の最終 review の `ArtifactExists` が見つける。
/// 本番（task 01M7MZTKBPZPQE4XN451F8X84F）と同じ形: WU `finish` の 1 本目の run が成果物を出して question で止まり、
/// 2 本目の run（review する run）は成果物を出さずに done になった。修正前は review する run の成果物だけが候補で、
/// `artifacts/final-report.md` を探して落ちた（本番の seq 2075 と同じ理由）。
#[tokio::test]
async fn root_review_artifact_finds_work_unit_run_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(
        dir.path(),
        Check::ArtifactExists {
            name: "final-report.md".into(),
        },
        0,
    );
    store.insert(&task).unwrap();
    let units = adopt_units(&store, task.id, &["measure", "finish"]);
    let finish = units.iter().find(|u| u.key == "finish").unwrap();
    write(dir.path(), "wu/finish/artifacts/final-report.md", "# final");
    unit_run_produced(
        &store,
        task.id,
        finish,
        "run-wu-finish-1",
        &[artifact(
            "final-report.md",
            "wu/finish/artifacts/final-report.md",
        )],
    );
    unit_run_produced(&store, task.id, finish, REVIEWED_RUN, &[]);

    let produced = candidates(&store, task.id);
    let v = artifact_verdict(&task, dir.path(), &produced).await;
    assert!(v.pass, "{}", v.reason);
    assert!(
        v.reason
            .starts_with("path=wu/finish/artifacts/final-report.md sha256="),
        "{}",
        v.reason
    );
}

/// 名前が重なれば、review する run の成果物 → WU の run の新しい順で選ぶ。
#[tokio::test]
async fn root_review_artifact_prefers_reviewed_run_then_newest_work_unit() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(
        dir.path(),
        Check::ArtifactExists {
            name: "report.md".into(),
        },
        0,
    );
    store.insert(&task).unwrap();
    let units = adopt_units(&store, task.id, &["early", "late"]);
    for key in ["early", "late"] {
        write(dir.path(), &format!("wu/{key}/artifacts/report.md"), key);
    }
    unit_run_produced(
        &store,
        task.id,
        &units[0],
        "run-wu-early",
        &[artifact("report.md", "wu/early/artifacts/report.md")],
    );
    unit_run_produced(
        &store,
        task.id,
        &units[1],
        "run-wu-late",
        &[artifact("report.md", "wu/late/artifacts/report.md")],
    );
    let v = artifact_verdict(&task, dir.path(), &candidates(&store, task.id)).await;
    assert!(
        v.reason.starts_with("path=wu/late/artifacts/report.md "),
        "{}",
        v.reason
    );

    write(dir.path(), "artifacts/report.md", "root");
    run_produced(
        &store,
        task.id,
        REVIEWED_RUN,
        &[artifact("report.md", "artifacts/report.md")],
    );
    // review する run の後に WU の run がもう一度出しても、review する run の分が勝つ。
    unit_run_produced(
        &store,
        task.id,
        &units[0],
        "run-wu-early-2",
        &[artifact("report.md", "wu/early/artifacts/report.md")],
    );
    let v = artifact_verdict(&task, dir.path(), &candidates(&store, task.id)).await;
    assert!(
        v.reason.starts_with("path=artifacts/report.md "),
        "{}",
        v.reason
    );
}

/// (2) 他の task（同じ案件の兄弟・子）と他の案件の task の WorkUnit の run が同じ名前の成果物を出しても拾わない。
/// 対象 task の events に他の task の WorkUnit の id を指す遷移が紛れても、その run の成果物は候補にしない。
#[tokio::test]
async fn root_review_artifact_ignores_other_tasks_and_projects() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let mine_project = titled_project("対象の案件");
    let other_project = titled_project("他の案件");
    store.project_create(&mine_project).unwrap();
    store.project_create(&other_project).unwrap();

    let mut task = new_task(
        dir.path(),
        Check::ArtifactExists {
            name: "final-report.md".into(),
        },
        0,
    );
    task.project_id = Some(mine_project.id);
    store.insert(&task).unwrap();
    adopt_units(&store, task.id, &["finish"]);

    // 同じ workspace を継ぐ子（tree の子 task）、同じ案件の兄弟、他の案件の task。どれも WU `finish` が同名の成果物を出す。
    let mut child = new_task(dir.path(), Check::Human, 0);
    child.parent_id = Some(task.id);
    child.project_id = Some(mine_project.id);
    let mut sibling = new_task(dir.path(), Check::Human, 0);
    sibling.project_id = Some(mine_project.id);
    let mut foreign = new_task(dir.path(), Check::Human, 0);
    foreign.project_id = Some(other_project.id);
    let mut foreign_unit = None;
    for (i, other) in [&child, &sibling, &foreign].into_iter().enumerate() {
        store.insert(other).unwrap();
        let units = adopt_units(&store, other.id, &["finish"]);
        let path = "wu/finish/artifacts/final-report.md";
        write(dir.path(), path, "not mine");
        unit_run_produced(
            &store,
            other.id,
            &units[0],
            &format!("run-other-{i}"),
            &[artifact("final-report.md", path)],
        );
        foreign_unit = Some(units[0].clone());
    }
    // 対象 task の events に他の案件の WorkUnit を指す遷移とその run の成果物があっても、対象 task の WorkUnit ではない。
    unit_run_produced(
        &store,
        task.id,
        &foreign_unit.unwrap(),
        "run-foreign-in-mine",
        &[artifact(
            "final-report.md",
            "wu/finish/artifacts/final-report.md",
        )],
    );

    let produced = candidates(&store, task.id);
    assert!(produced.is_empty(), "{produced:?}");
    let v = artifact_verdict(&task, dir.path(), &produced).await;
    assert!(!v.pass, "{}", v.reason);
    assert_eq!(
        v.reason,
        "artifact \"final-report.md\" not found at artifacts/final-report.md"
    );
}

/// (3) WU の run の成果物でも review する run の成果物でも、path が workspace の外（絶対 path・`..`・空・外への symlink）
/// を指すものは `ArtifactExists` が使わない（規則は成果物を出す側の `task_worker::artifact::resolve`）。外を指す候補が
/// 後ろにあっても、前にある workspace の中の同名の候補を使い、中の候補が無ければ既定の path（`artifacts/<name>`）で判定する。
/// 修正前は review する run の絶対 path がそのまま使われ、workspace の外の file で合格していた。
#[tokio::test]
async fn root_review_artifact_ignores_paths_outside_workspace() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("task");
    std::fs::create_dir_all(&dir).unwrap();
    let outside = root.path().join("outside/final-report.md");
    write(root.path(), "outside/final-report.md", "outside");
    std::fs::create_dir_all(dir.join("wu/finish/artifacts")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("wu/finish/artifacts/link.md")).unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(
        &dir,
        Check::ArtifactExists {
            name: "final-report.md".into(),
        },
        0,
    );
    store.insert(&task).unwrap();
    let units = adopt_units(&store, task.id, &["finish"]);
    unit_run_produced(
        &store,
        task.id,
        &units[0],
        "run-wu-finish",
        &[
            artifact("final-report.md", outside.to_str().unwrap()),
            artifact("final-report.md", "../outside/final-report.md"),
            artifact(
                "final-report.md",
                "wu/finish/../../../outside/final-report.md",
            ),
            artifact("final-report.md", ""),
            artifact("final-report.md", "wu/finish/artifacts/link.md"),
        ],
    );
    // review する run 自身の記録でも同じ（成果物を出す側の `artifact::resolve` はこの path を受け付けない）。
    run_produced(
        &store,
        task.id,
        REVIEWED_RUN,
        &[artifact("final-report.md", outside.to_str().unwrap())],
    );

    let escaping = candidates(&store, task.id);
    let v = artifact_verdict(&task, &dir, &escaping).await;
    assert!(!v.pass, "{}", v.reason);
    assert_eq!(
        v.reason,
        "artifact \"final-report.md\" not found at artifacts/final-report.md"
    );
    // 外を指す 6 件は候補に入っていて（組み立ては path を見ない）、`review_task` が使わなかった。
    assert_eq!(escaping.len(), 6, "{escaping:?}");

    // workspace の中の同名の候補が前にあれば、後ろの外を指す候補に隠されずにそれを使う。
    write(&dir, "wu/finish/artifacts/final-report.md", "inside");
    let mut with_inside = vec![artifact(
        "final-report.md",
        "wu/finish/artifacts/final-report.md",
    )];
    with_inside.extend(escaping);
    let v = artifact_verdict(&task, &dir, &with_inside).await;
    assert!(v.pass, "{}", v.reason);
    assert!(
        v.reason
            .starts_with("path=wu/finish/artifacts/final-report.md sha256="),
        "{}",
        v.reason
    );
}

/// (4) 計画の無い task は従来どおり: 候補は review する run の成果物だけ（前の attempt の run の成果物は入らない）で、
/// 判定は `artifacts_for_run` を渡したときと同じ。
#[tokio::test]
async fn root_review_artifact_unplanned_task_keeps_reviewed_run_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let task = new_task(
        dir.path(),
        Check::ArtifactExists {
            name: "report.md".into(),
        },
        0,
    );
    store.insert(&task).unwrap();
    write(dir.path(), "artifacts/old/report.md", "old attempt");
    run_produced(
        &store,
        task.id,
        "run-previous-attempt",
        &[artifact("report.md", "artifacts/old/report.md")],
    );
    write(dir.path(), "artifacts/sub/report.md", "reviewed");
    run_produced(
        &store,
        task.id,
        REVIEWED_RUN,
        &[artifact("report.md", "artifacts/sub/report.md")],
    );

    let events = store.events_for(task.id).unwrap();
    let produced = candidates(&store, task.id);
    assert_eq!(
        produced,
        task_ops::derive::artifacts_for_run(&events, REVIEWED_RUN)
    );
    let v = artifact_verdict(&task, dir.path(), &produced).await;
    assert!(v.pass, "{}", v.reason);
    assert!(
        v.reason.starts_with("path=artifacts/sub/report.md "),
        "{}",
        v.reason
    );

    // 前の attempt の成果物しか無い名前は、従来どおり既定の path で判定する。
    let mut only_old = task.clone();
    only_old.acceptance[0].check = Check::ArtifactExists {
        name: "old.md".into(),
    };
    run_produced(
        &store,
        task.id,
        "run-previous-attempt",
        &[artifact("old.md", "artifacts/old/report.md")],
    );
    let v = artifact_verdict(&only_old, dir.path(), &candidates(&store, task.id)).await;
    assert!(!v.pass, "{}", v.reason);
    assert_eq!(
        v.reason,
        "artifact \"old.md\" not found at artifacts/old.md"
    );
}
