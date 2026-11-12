//! `disk_admission` の試験（ADR 2026-11-11-disk-aware-dispatch）。

use super::*;

const GB: u64 = 1_000_000_000;

fn sample(owner: &str, repo: &str, grows: bool, size: Option<u64>) -> Sample {
    Sample {
        owner: owner.into(),
        repo_key: repo.into(),
        grows,
        size_bytes: size,
    }
}

fn busy(tasks: &[&str]) -> BTreeSet<String> {
    tasks.iter().map(|t| t.to_string()).collect()
}

/// 通した run がすべて走っている（dispatcher の `running` にある）とみなす鍵。
fn running(gate: &Gate) -> Vec<(String, Option<String>)> {
    gate.state().admitted.keys().cloned().collect()
}

fn inputs(free: u64, samples: Vec<Sample>) -> Inputs {
    Inputs {
        free_bytes: Some(free),
        reserve_bytes: 30 * GB,
        ratio: 1.0,
        samples,
    }
}

/// 不変条件: 予備は容量に比例し、既定 10% は disk_watch の critical（95%）より手前で止める。
#[test]
fn the_reserve_is_proportional_to_the_filesystem() {
    assert_eq!(min_free_bytes(300 * GB, 10.0), 30 * GB);
    assert_eq!(min_free_bytes(300 * GB, 0.0), 0);
    assert!(min_free_bytes(300 * GB, 10.0) > 300 * GB / 100 * 5);
}

/// 不変条件: repo の見込みは同じ repo の最大、走っている owner だけが増分を持つ。
#[test]
fn expectations_are_the_repo_maximum_and_growth_counts_running_owners() {
    let v = forecast(
        &inputs(
            100 * GB,
            vec![
                sample("task-a", "ap", false, Some(100 * GB)),
                sample("task-b", "ap", true, Some(40 * GB)),
                sample("task-c", "ap", true, None),
                sample("task-d", "other", true, Some(5 * GB)),
            ],
        ),
        None,
    );
    assert_eq!(v.expected_by_repo.get("ap"), Some(&(100 * GB)));
    assert_eq!(v.running_growth_bytes, 60 * GB + 100 * GB);
    assert_eq!(need_bytes(&v, "task-new", "ap"), 100 * GB);
    assert_eq!(need_bytes(&v, "task-b", "ap"), 60 * GB);
    assert_eq!(need_bytes(&v, "task-a", "ap"), 0);
}

fn owner_row(
    owner: &str,
    kind: &str,
    class: &str,
    reason: &str,
    target: bool,
    size: u64,
) -> ScratchOwnerView {
    ScratchOwnerView {
        owner: owner.into(),
        kind: kind.into(),
        class: class.into(),
        reason: reason.into(),
        has_target: target,
        size_bytes: Some(size),
        estimated_bytes: size,
        measured_at: None,
        lease_mtime: None,
        repo_key: Some("ap".into()),
        base_commit: None,
        adopted_from: None,
        work_unit_key: None,
    }
}

/// 不変条件（attempt 3・attempt 4 の review）: repo の見込みは target が pool にある owner だけから決まり（GC の推定と
/// 同じ入力）、増分は run か review の検査が今書いている owner（seed を写している最中で target がまだ無い owner も
/// 大きさ不明として含む）と lease の生きた release-build だけが持つ。まだ始まらない WU・target の消えた lease の記録・
/// p0 でない owner は数えない。上の試験は `Sample` を直接与えるので、走査の行からの絞り込みを確かめない。
#[test]
fn samples_count_present_targets_and_grow_only_where_a_run_writes_now() {
    let rows = vec![
        owner_row("release-abc", "release", "p0", "lease live", true, 60 * GB),
        owner_row("task-gone", "task", "p3", "task done", false, 150 * GB),
        owner_row("task-p3", "task", "seed", "task done", true, 90 * GB),
        owner_row("task-r", "task", "p0", "task running", true, 40 * GB),
        owner_row("task-v", "task", "p0", "task reviewing", true, 70 * GB),
        owner_row(
            "task-w/wu-q",
            "work_unit",
            "p0",
            "work unit queued",
            true,
            10 * GB,
        ),
        owner_row(
            "task-w/wu-r",
            "work_unit",
            "p0",
            "work unit running",
            true,
            20 * GB,
        ),
    ];
    let samples = samples_from_owners(&rows);
    assert!(samples.iter().all(|s| s.owner != "task-gone"));
    // target をまだ写している running の owner（lease だけある）は、大きさ不明の running として数える。
    let copying = samples_from_owners(&[owner_row(
        "task-seeding",
        "task",
        "p0",
        "task running",
        false,
        150 * GB,
    )]);
    assert_eq!(copying.len(), 1);
    assert!(copying[0].grows && copying[0].size_bytes.is_none());
    let v = forecast(&inputs(100 * GB, [samples.clone(), copying].concat()), None);
    let seeding = v.owners.iter().find(|o| o.owner == "task-seeding");
    assert_eq!(seeding.map(|o| o.growth_bytes), Some(90 * GB));
    let v = forecast(&inputs(100 * GB, samples), None);
    assert_eq!(v.expected_by_repo.get("ap"), Some(&(90 * GB)));
    let growing: Vec<&str> = v
        .owners
        .iter()
        .filter(|o| o.growth_bytes > 0)
        .map(|o| o.owner.as_str())
        .collect();
    assert_eq!(
        growing,
        vec!["release-abc", "task-r", "task-v", "task-w/wu-r"]
    );
    // lease の生きた release-build（target 60 GB）は次の build で見込み（90 GB）まで育つ増分 30 GB を持つ。
    assert_eq!(v.running_growth_bytes, (30 + 50 + 20 + 70) * GB);
}

/// 不変条件: 係数は増分と見込みにだけ掛かる（disk 上の量に直す）。
#[test]
fn the_ratio_scales_needs_and_growth() {
    let mut i = inputs(
        100 * GB,
        vec![
            sample("task-a", "ap", false, Some(100 * GB)),
            sample("task-b", "ap", true, Some(60 * GB)),
        ],
    );
    i.ratio = 0.5;
    let v = forecast(&i, None);
    assert_eq!(v.running_growth_bytes, 20 * GB);
    assert_eq!(need_bytes(&v, "task-new", "ap"), 50 * GB);
}

#[test]
fn decide_holds_when_the_forecast_crosses_the_reserve_and_never_blocks_the_only_run() {
    let v = forecast(
        &inputs(
            150 * GB,
            vec![
                sample("task-a", "ap", false, Some(100 * GB)),
                sample("task-b", "ap", true, Some(80 * GB)),
            ],
        ),
        None,
    );
    // 150 − 20 − 0 − 100 = 30 ≥ 30: 通す。2 本目は 30 − 100 < 30: 待つ。
    assert_eq!(
        decide(&v, "task-x", "ap", 0, 1),
        Decision::Admit {
            need_bytes: 100 * GB
        }
    );
    assert!(matches!(
        decide(&v, "task-y", "ap", 100 * GB, 1),
        Decision::Hold { .. }
    ));
    // 走っている run が 0 なら通す。
    assert!(matches!(
        decide(&v, "task-y", "ap", 100 * GB, 0),
        Decision::Admit { .. }
    ));
    // 見込みの無い repo は判定しない。
    assert_eq!(decide(&v, "task-z", "unknown", 0, 1), Decision::Skip);
}

/// 不変条件（D4）: daemon の status の `held` は自分の Gate のメモリだけから来る。別の Gate（同じ pool を見る verify の
/// daemon）が空の pass を回しても変わらない。旧い試験 `held_runs_round_trip_through_the_pool_file` は pool の
/// `admission.json`（廃止した共有の写し）を確かめていたので、この試験に置き換えた。
#[test]
fn the_daemon_status_carries_its_own_held_runs_from_memory() {
    let v = forecast(
        &inputs(10 * GB, vec![sample("task-a", "ap", true, Some(50 * GB))]),
        None,
    );
    assert_eq!(
        v.held, None,
        "a pool scan alone does not know the held runs"
    );
    let mut status = crate::scratch_gc::disabled_status(
        &task_worker::scratch::ScratchSettings::default(),
        SystemTime::UNIX_EPOCH,
    );
    status.admission = v.clone();
    let (live, verify) = (Gate::default(), Gate::default());
    // a の run をこの daemon が走らせている（書き手）。
    live.begin_pass(running(&live), busy(&["a"]));
    assert!(live.hold(&v, "t1", None, "task-t1", "ap", SystemTime::UNIX_EPOCH));
    live.begin_pass(running(&live), busy(&[]));
    verify.begin_pass(running(&verify), busy(&[]));
    let held = live.with_held(Some(&status)).and_then(|s| s.admission.held);
    let owners: Vec<String> = held
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.owner)
        .collect();
    assert_eq!(owners, vec!["task-t1".to_string()]);
    assert_eq!(
        verify
            .with_held(Some(&status))
            .and_then(|s| s.admission.held),
        Some(Vec::new())
    );
    assert_eq!(live.with_held(None), None);
    // 次の pass で判定に来なければ消える。
    live.begin_pass(running(&live), busy(&[]));
    assert!(live.held_views().is_empty());
}

/// 不変条件（attempt 5 の review の指摘）: 通した run の見込みは pass をまたいで、走査に書いている owner として出るまで
/// 数え続ける。lease を書く前に終わった run（走っていない）の分は次の pass で消える。target を今書いている run が
/// 無ければ 1 本は通すが、通した run が走査に出る前でもそれを書いている run として数える。旧い試験は
/// `decide` に通した量を手で渡すか、次の tick の前に通した run の sample を probe に足していたので、lease が現れる
/// までの抜けを検出できなかった。
#[test]
fn admitted_runs_count_across_passes_until_the_scan_shows_them() {
    let old = sample("task-old", "ap", false, Some(100 * GB));
    let v = forecast(&inputs(250 * GB, vec![old.clone()]), None);
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&[]));
    // 書いている run 0 本: a は通す。b は 250 − 100 − 100 = 50 ≥ 30 で通す。c は 250 − 200 − 100 < 30 で待つ。
    assert!(!gate.hold(&v, "a", None, "task-a", "ap", SystemTime::UNIX_EPOCH));
    assert!(!gate.hold(&v, "b", None, "task-b", "ap", SystemTime::UNIX_EPOCH));
    assert!(gate.hold(&v, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    // 次の pass: a・b の lease はまだ走査に無い。c は待ち続ける。
    for _ in 0..3 {
        gate.begin_pass(running(&gate), busy(&[]));
        assert_eq!(gate.admitted_bytes(), 200 * GB);
        assert!(gate.hold(&v, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    }
    assert_eq!(gate.notes(), 1);
    // a が seed を写し始めて走査に出る（大きさ不明の書いている owner。増分 100）: 通した分からは外れ、二重に数えない。
    let seeding = Sample {
        owner: "task-a".into(),
        repo_key: "ap".into(),
        grows: true,
        size_bytes: None,
    };
    let v2 = forecast(&inputs(250 * GB, vec![old.clone(), seeding]), None);
    gate.begin_pass(running(&gate), busy(&[]));
    assert!(gate.hold(&v2, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    assert_eq!(gate.admitted_bytes(), 100 * GB);
    // b が lease を書く前に終わった（走っていない）: 次の pass で消え、c は 250 − 100 − 0 − 100 = 50 ≥ 30 で通る。
    gate.begin_pass(
        running(&gate).into_iter().filter(|(t, _)| t != "b"),
        busy(&["a"]),
    );
    assert!(!gate.hold(&v2, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    assert!(gate.held_views().is_empty());
    // 走査に書いている owner が無く、通した run も無ければ、予備を割っても 1 本は通す。
    let empty = Gate::default();
    let low = forecast(&inputs(5 * GB, vec![old]), None);
    assert!(!empty.hold(&low, "d", None, "task-d", "ap", SystemTime::UNIX_EPOCH));
    assert!(empty.hold(&low, "e", None, "task-e", "ap", SystemTime::UNIX_EPOCH));
}
/// 不変条件（attempt 6 の review の指摘）: lease の生きた release-build は増分を持つが、「1 本は通す」の数えには入らない。
/// task の run が 0 本なら、空きが予備を割っていても 1 本は通す（release の lease は 48 時間生き、待っても空きは戻らない）。
/// 旧い試験は release の owner を含む走査で 0 本の規則を見ていなかった。
#[test]
fn a_live_release_build_does_not_stall_the_first_task_run() {
    let rel = Sample {
        owner: "release-abc".into(),
        repo_key: "ap".into(),
        grows: true,
        size_bytes: Some(90 * GB),
    };
    let seed = sample("task-p3", "ap", false, Some(90 * GB));
    let v = forecast(&inputs(50 * GB, vec![rel, seed]), None);
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&[]));
    // 空き 50 GB は予備を割るが、task の run は 0 本なので 1 本は通す。2 本目は待つ。
    assert!(!gate.hold(&v, "t1", None, "task-t1", "ap", SystemTime::UNIX_EPOCH));
    assert!(gate.hold(&v, "t2", None, "task-t2", "ap", SystemTime::UNIX_EPOCH));
}

/// 不変条件（attempt 7 の review の指摘）: 「1 本は通す」の書き手は、増分を持つ owner のうち、この daemon が run・
/// review・WU の検査を走らせている task のものだけ。`reviewing` で子・人の承認を待つ親（走査では増分を持つ）は
/// 書き手に数えず、子の run は予備を割っていても通る（数えると親は子を、子は親を待って詰まる）。同じ親が review の
/// 検査を走らせている間は書き手に数え、子は待つ。旧い試験は書き手を走査の増分だけから数え、待っている親を検出
/// できなかった。
#[test]
fn a_parent_waiting_in_review_is_not_a_writer_but_a_running_review_is() {
    let parent = sample("task-p", "ap", true, Some(40 * GB));
    let old = sample("task-old", "ap", false, Some(100 * GB));
    // 空き 69 GB − 親の増分 60 − 子の見込み 100 < 予備 30。
    let v = forecast(&inputs(69 * GB, vec![parent, old]), None);
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&["other"]));
    assert!(!gate.hold(&v, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&["p"]));
    assert!(gate.hold(&v, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
    // 親の WU の owner（task-p/wu-w）も親の task の書き手として数える。
    let wu = sample("task-q/wu-w", "ap", true, Some(40 * GB));
    let v = forecast(&inputs(69 * GB, vec![wu]), None);
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&["q"]));
    assert!(gate.hold(&v, "c", None, "task-c", "ap", SystemTime::UNIX_EPOCH));
}

/// 不変条件（attempt 7 の review の副次の指摘）: 通した run の見込みは、worker と同じ owner（task の worktree を
/// 共有する WU なら `task-<id>`）で走査の owner と突き合わせる。続きの run はその owner の target の増分だけで判定し、
/// 走査に出たら通した分から外れる（二重に数えない）。
#[test]
fn a_shared_worktree_work_unit_is_judged_by_the_task_owner() {
    let task_owner = task_worker::scratch::Owner::of_run("t", None).to_string();
    assert_eq!(task_owner, "task-t");
    assert_eq!(
        task_worker::scratch::Owner::of_run("t", Some("w")).to_string(),
        "task-t/wu-w"
    );
    let v = forecast(
        &inputs(
            45 * GB,
            vec![
                sample("task-old", "ap", false, Some(100 * GB)),
                sample("task-t", "ap", false, Some(92 * GB)),
            ],
        ),
        None,
    );
    let gate = Gate::default();
    gate.begin_pass(running(&gate), busy(&["x"]));
    // 走査に書き手を 1 本置く（x）。phase のある WU w の run（鍵は (t, w)）は task-t の target の増分 8 GB で判定:
    // 45 − 0 − 8 ≥ 30 で通る。WU の owner（task-t/wu-w）で判定すると 100 GB 全体になって待つ。
    let x = sample("task-x", "other", true, Some(GB));
    let v = ScratchAdmissionView {
        owners: [v.owners.clone(), forecast(&inputs(0, vec![x]), None).owners].concat(),
        ..v
    };
    assert!(!gate.hold(
        &v,
        "t",
        Some("w"),
        &task_owner,
        "ap",
        SystemTime::UNIX_EPOCH
    ));
    assert_eq!(gate.admitted_bytes(), 8 * GB);
    // task-t が走査に増分を持つ owner として出たら、通した分から外れる。
    let mut seen = v.clone();
    if let Some(o) = seen.owners.iter_mut().find(|o| o.owner == "task-t") {
        o.grows = true;
    }
    assert!(gate.hold(&seen, "u", None, "task-u", "ap", SystemTime::UNIX_EPOCH));
    assert_eq!(gate.admitted_bytes(), 0);
}
