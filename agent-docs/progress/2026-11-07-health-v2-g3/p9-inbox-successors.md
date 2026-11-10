---
title: 健全性v2 G3 P-9 successors — retry の後継を持つ後続を reopen と retry の両経路で戻さない
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 successors — retry の後継を持つ後続を reopen と retry の両経路で戻さない

WorkUnit `successors`（P-9 (a)）。G3 の final review の差し戻し (1) と、ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の D7.2・決定 p9-scope の範囲。基準は `$CELERIS_WU_BASE`。依存先 A の失敗で cancel された後続 B を人が retry して後継 B' を作っていたら、A の reopen（復帰）でも A の retry（付け替え）でも B を戻さない。kept の記録（`DependentsReopened`）は後続の unit `event`、受信箱は unit `inbox` が持つ。

## 変更
件名「P-9 successors: retry の後継を持つ後続を reopen と retry の両経路で戻さない」の commit。
- `crates/task-core/src/model/events.rs`: kept の理由の型 `KeptReason`（`Retried { successor }`・`OtherDependency { unresolved }`、serde は `reason` の tag と snake_case、`JsonSchema`）。`model.rs` の `pub use events::{…}` と `lib.rs` の `pub use model::{…}` から出す。unit `event` の `DependentsReopened` はこの型を使う。
- `crates/task-core/src/transition.rs`: 判定の型 `DependencyCancel<T>`（`Other`・`Restore(T)`・`Kept(KeptReason)`）。
  - `cancelled_by_dependency(status, last, successor)` は `Restore(cancel の前の状態)`、`Kept(Retried { successor })`、`Other`（依存先の失敗の cancel でない）を返す。
  - `dependency_reopen_trigger(status, last, successor, others)` は `others` を（id, 状態）で受け、`Restore(DependencyReopened)`、`Kept(Retried)`、`Kept(OtherDependency { unresolved })`、`Other` を返す。後継の判定は未解決の依存先より先に行う。
- `crates/task-core/src/store/transition.rs`:
  - module の関数 `last_event_tx(tx, task_id, kind) -> Option<Event>` を足した。1 つの task の最新の 1 件を種類で読む SQL はここだけで、`UNIQUE(task_id, seq)` の index を seq の降順に辿る。比べ方は `instr`（`kind` の `_` は LIKE の wildcard にならない）。`last_transitioned_tx` は `last_event_tx(…, "transitioned")` の結果を取り出すだけにした（呼び出し元は変えない）。
  - module の関数 `retry_successors_tx(tx, dependents) -> HashMap<元, 後継>` を足した。後続ごとに最新の `Event::Retried` を `last_event_tx(…, "retried")` で 1 件読む。events の全走査はしない。migration は足さない。
  - `cascade_after_transition_tx` の復帰の枝が `retry_successors_tx` と `dependency_reopen_trigger` を使い、`Restore` のときだけ戻す。
  - store の重複を 3 つ畳んで行を相殺した。(1) `task_status_tx` と `apply_transition_tx` の task の読み出しを `get_locked` にした。(2) `apply_transition_tx` の遷移の event と extra_events の INSERT の 2 つの写しを `append_event_tx` にした。seq は同じ transaction の `MAX(seq)+1`、ts は今の時刻で、今までと同じ行になる。派生の 3 つ（`close_run_row_for_event_tx`・`apply_tree_event_tx`・`cluster_job::apply_event_tx`）は `Transitioned` を扱わない。(3) 条件が同じ 3 つの `if !from_terminal && to_terminal` を 1 つにまとめた（中の順は変えない）。
- `crates/task-core/src/store/tasks.rs`: `retry_task_tx` の付け替えが、同じ `retry_successors_tx` と `cancelled_by_dependency` を同じ入力で使う。cancelled の後続は `Restore` のときだけ付け替える。後継（draft・ready・blocked）は今までどおり付け替わる。
- 試験: `crates/task-core/src/store/dependency_reopen_tests.rs` に 3 件、`crates/task-core/src/transition/tests.rs` に純粋関数の 1 件（下）。

## 実装前 brief
1. 根本原因: 共有の述語 `cancelled_by_dependency` の入力が後続の状態と最後の遷移だけで、retry の後継の有無を受けない。症状として、A の reopen で B が戻って B と B' が同じ仕事を持ち、A の retry で B が A' に付け替わって draft に戻る。
2. 責務 owner: 述語は `crate::transition`（純粋関数）。後継の読み出しは store の `store/transition.rs`（依存の cascade の場所）。
3. 既存の再利用先: `dependents_of_tx`（後継 B' も A の後続に入る）、`last_transitioned_tx` の SQL（`last_event_tx` に一般化）、`get_locked`、`append_event_tx`。
4. 選択肢: (a) `Event::Retried` を元の task の側にも書く（新しい event か migration が要る）。(b) 後続の events から `Retried` を task_id の index で 1 件ずつ読む。(b) を採った（決定 p9-scope の範囲、migration なし）。
5. 新しい状態・分岐・fallback・port: 判定の型 `DependencyCancel<T>` と `KeptReason`。fallback・切替 flag・新しい port は無し。
6. 撤去対象: `cancelled_by_dependency` の `Option<Status>` と `dependency_reopen_trigger` の `Option<Trigger>` の戻り値（型を置き換えた）。`last_transitioned_tx` の独自の SQL（`last_event_tx` に移した）。`apply_transition_tx` の INSERT の写し 2 つと task の読み出しの写し。
7. 不変条件: 後継を持つ後続は reopen でも retry でも戻らず、付け替わらない。後継が A の後続でなくなれば、元の後続は戻る。既存の 5 件の挙動は変わらない（下の試験）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
既存の `dependency_reopen_` の 5 件はどれも後継のある後続を作らないので、後継のある B が A の reopen・retry で戻る重複を検出できない。
- `dependency_reopen_keeps_dependent_with_retry_successor`: A の reopen で、後継 B' を持つ B は cancelled のまま（理由は `dependency_failed` だけ）。判定は `Kept(Retried { successor: B' })`。B' は draft のままで、event も増えない。後継の無い C は ready に戻る。
- `dependency_reopen_retry_skips_dependent_with_successor`: A の retry（A → A'）で、付け替わるのは B' と C だけ。B は cancelled のまま、`depends_on` は A のまま。B' は draft のまま A' に付け替わる。C は A' に付け替わって draft に戻る（理由 `retried`）。
- `dependency_reopen_restores_when_successor_drops_dependency`: 人が B' の `depends_on` から A を外す（`update_task`）と、判定は `Restore(DependencyReopened { draft: false })` になる。A の reopen で B は ready に戻り、B' は触られない。
- `dependency_reopen_trigger_distinguishes_kept_reasons`（純粋関数）: `Restore`（draft の前は draft）、`Kept(Retried)`（未解決の依存先があっても後継を先に見る）、`Kept(OtherDependency { unresolved })`（failed と見つからない依存先の id を順に入れる）、`Other`（人の cancel、遷移の記録が無い、cancelled でない）を区別する。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-core --no-tests=fail -E 'test(/dependency_reopen_|retry/)'`: exit 0、26 passed（dependency_reopen_ の 9 件を含む）。
- 健全性の値（base → head。`size.responsibility-growth` と同じ数え方）: store の module の行 19736 → 19720、`SqliteStore` の method 730 → 730。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: 結果は下の追記。

## 未解決
- `cargo clippy -p task-core --all-targets -- -D warnings` は、base から変わっていない `crates/task-core/src/chat/store_tests.rs` の 1903 行・1975 行の `useless_conversion` で落ちる。repo の gate（`--all-targets` なし）は合格する。この unit の範囲の外なので直していない。

## 提案
- 無し。
