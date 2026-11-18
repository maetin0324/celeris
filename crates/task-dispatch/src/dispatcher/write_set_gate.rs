//! ADR-0130 D3: 同じ repo で走っている run の expected write-set と候補の expected write-set が
//! 強く重なれば、候補の起動を次 tick に回す（待たせるだけ。失敗・attempts・状態遷移なし）。
//! hint の無い run は予約を作らず、照合もしない（従来どおり）。
//!
//! D3 の配線で決めた細部（ADR 本文への反映は verify 葉で行う）:
//!
//! - **予約の置き場所**: `crate::execution_coordination::ExecutionCoordination` のメモリ上の側表（`RunKey` → repo 鍵と
//!   正規化 prefix）に、run を起こした時点の hint で置く（ADR 2026-11-11 dispatcher-state-ownership D1）。照合は
//!   実行中の run の鍵（`RunSupervisor::run_keys`）に残る run の予約だけを見るので、run の終了・lease
//!   回収・中止で実行中の run の表から外れた時点で予約も効かなくなる。この file は hint から予約を作る
//!   純粋関数（`reservation`）だけを持ち、hint の読み出しと判定の呼び出しは dispatch の入口
//!   （`dispatch_run.rs` の `admit_write_set`。ADR 2026-11-18 dispatcher-store-access-pilot）が行う。task の lease は
//!   1 つの daemon だけが持つ（ADR-0040）ので、この Phase では store の予約表は作らない。
//! - **照合の位置**: `dispatch_one` の retry / infra backoff の後、クラスタ解決・作業場所の用意・
//!   lease・`Trigger::Dispatch` より前。見送りは `Ok(false)` を返すだけ。
//! - **対象の run**: implementation の run（atomic task の run と WU の run）。planner run と CoS の
//!   対話 run は予約も照合もしない。どちらかが hint なしなら重ならない（D1 の従来互換）。
//! - **repo 鍵**: `task.repos` があれば `RepoId`、無い旧 task はローカル作業場所のパス（`path:`）、
//!   リモートは `remote:<cluster>:<path>`。
//! - **同じ task の WU 同士**: 両方が task の有効 hint をそのまま継いだものなら照合しない。unit 固有の
//!   hint を持つ WU は兄弟とも照合する。
//! - **見送りの記録**: event は増やさず `tracing` の log（`write-set overlap; deferring the run`）に残す。
//! - **公平性**: `RunKey` ごとに待機開始 tick と連続回数を持ち、連続 3 回以上見送られた ready task は
//!   待機開始の古い順に候補の先頭へ移す。
//! - **飢餓の防止**（2026-10-03-write-set-no-starvation）: 容量（`max_concurrency`・CoS 枠）が尽きて走査が
//!   途中で切れた tick は、照合されなかった待機記録を「待ち続けた」として引き継ぐ（数え直さない）。連続
//!   回数が閾値に届いた待機は自分の予約を「先取り」として持ち、自分より後から待ち始めた重なる候補を
//!   止める（先に待った方が先に走る）。兄弟 WU の走査は待たされた WU を飛ばして後ろを試す。
//! - **actual の併用**: 走行中の run の既知の actual はこの Phase では照合に使わない（expected だけ）。

use task_core::{Task, WorkspaceSpec};

use crate::execution_coordination::WriteReservation;

/// 候補の run の予約（`ExecutionCoordination::admit_write_set` に渡す）。hint なし・hint が空・読めない hint・
/// repo 鍵なしは `None`（gate に掛けない）。planner run と CoS の対話 run は、入口が hint を読まずに `None` にする。
///
/// `task_hint` は task の有効 hint、`wu_hint` は WU の run なら `Some(WU の expected hint)`、WU の無い run は `None`。
/// WU の hint が task の有効 hint と同じなら「task の hint を継いだ」予約にする（同じ task の WU 同士を照合しない）。
pub(super) fn reservation(
    task: &Task,
    task_hint: Option<Vec<String>>,
    wu_hint: Option<Option<Vec<String>>>,
) -> Option<WriteReservation> {
    let (paths, inherited_from_task) = match wu_hint {
        Some(hint) => {
            let inherited = hint.is_some() && hint == task_hint;
            (hint, inherited)
        }
        None => (task_hint, false),
    };
    let paths = paths.filter(|p| !p.is_empty())?;
    let paths = match task_core::write_set::normalize_write_paths(&paths) {
        Ok(paths) => paths,
        Err(e) => {
            // 保存時に検証済みのはず。読めない hint は従来互換で「hint なし」に倒す。
            tracing::warn!(task_id = %task.id, error = %e, "invalid expected_write_paths; ignoring the hint (ADR-0130 D1)");
            return None;
        }
    };
    let repos = write_set_repo_keys(task);
    if repos.is_empty() {
        return None;
    }
    Some(WriteReservation::new(repos, paths, inherited_from_task))
}

/// task の repo 鍵（ADR-0130 付記）。子 task は親の `repos` を継ぐので同じ鍵になる。
fn write_set_repo_keys(task: &Task) -> Vec<String> {
    if !task.repos.is_empty() {
        let mut keys: Vec<String> = task
            .repos
            .iter()
            .map(|r| format!("repo:{}", r.repo_id))
            .collect();
        keys.sort();
        keys.dedup();
        return keys;
    }
    match &task.workspace {
        WorkspaceSpec::Local { path, .. } => {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            vec![format!("path:{}", path.display())]
        }
        WorkspaceSpec::Remote { cluster, path, .. } => {
            vec![format!("remote:{cluster}:{}", path.display())]
        }
    }
}
