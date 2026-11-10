---
title: 健全性v2 G3 P-9 event — 依存先の終端からの復帰で、戻した後続と戻さなかった後続を DependentsReopened として記録する
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 event — 依存先の終端からの復帰で、戻した後続と戻さなかった後続を DependentsReopened として記録する

WorkUnit `event`（P-9 (b)）。ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の D7.3 と決定 p9-scope（戻した後続は task id と依存先を記録する）の範囲。基準は `$CELERIS_WU_BASE`。依存先 A が終端から戻った遷移と同じ transaction で、戻した後続（restored）と、依存先の失敗で cancel されたが戻さなかった後続（kept）を A の events に 1 件追記する。受信箱の項目は後続の unit `inbox-read`・`inbox` が持つ。

## 変更
件名「P-9 event: 依存先の終端からの復帰で DependentsReopened を記録する」の commit。
- `crates/task-core/src/model/events.rs`: `Event::DependentsReopened { restored, kept }` と要素の型 `RestoredDependent { task_id, depends_on }`・`KeptDependent { task_id, why: KeptReason }`。`KeptReason` は unit successors が置いた型をそのまま使う。`model.rs` と `lib.rs` の `pub use` から出す。
  - `why` を `#[serde(flatten)]` で平たく並べない。flatten の schema は `oneOf` と `properties` が同居し、web の生成器（`web/scripts/gen-types.mjs`）は `oneOf` を先に見て `task_id` を落とす（試して確かめた）。
- `crates/task-core/src/transition.rs`: 純粋関数 `dependents_reopened(from, verdicts, previous) -> Option<Event>`（store に依存しない）。`verdicts` は後続ごとの（id、`depends_on`、`dependency_reopen_trigger` の判定）。
  - restored は `Restore`、kept は `Kept`。kept は `from` が failed・cancelled のときだけ数える。`Other`（人・CoS の cancel、`parent_cancelled` などの連鎖の中止、cancelled でない後続）はどちらにも入れない。
  - 両方空なら `None`。restored が空で kept が `previous`（A の直前の `DependentsReopened`）の kept と同じなら `None`。
  - 並びは task id 順（後続の読み出しの順に依らず、直前の記録と比べられる）。
- `crates/task-core/src/store/transition.rs`: `cascade_after_transition_tx` の復帰の枝が、後続ごとの判定を集め、`Restore` のときは今までどおり遷移を適用する。枝の最後に A の直前の記録を `last_event_tx(tx, A, "dependents_reopened")` で 1 件だけ読み（unit successors の関数。新しい SQL は無し、events の全走査は無し）、`dependents_reopened` が返した event を `append_event_tx` で A に追記する。`SqliteStore` の method は足していない。
- `crates/task-api/src/query.rs`: `event_type_name` に `dependents_reopened`。
- `docs/api/v1/event.schema.json`・`api-v1.schema.json`（`UPDATE_SCHEMA=1` で再生成）、`docs/api/v1/api.md` の改訂行（2026-11-10）。
- `web/api/generated/{schema.json,types.ts}`（`corepack pnpm@12.6.0 -C web run gen:types`）、`web/api/realtime/event-kinds.ts`・`invalidation-map.ts`（`dependents_reopened: { sets: ["T", "L"] }`。依存先の timeline と受信箱が変わる）。

## 実装前 brief
1. 根本原因: 依存先の終端からの復帰（D7.1）は後続を戻すが、何を戻し何を戻さなかったかを残さない。人は戻った後続と手で起票した後継の重複に気づけず、受信箱（D7.3）の正本も無い。
2. 責務 owner: 記録の内容の判定は `crate::transition`（`dependency_reopen_trigger` と同じ場所の純粋関数）、追記は依存の cascade の場所 `store/transition.rs` の `cascade_after_transition_tx`。
3. 既存の再利用先: `dependency_reopen_trigger`（判定と kept の理由）、`last_event_tx`（最新の 1 件を種類で読む）、`append_event_tx`（同じ transaction の続き番号で追記）、`KeptReason`。
4. 選択肢: (a) store の枝で restored・kept を組み立てて判定する。(b) 判定を純粋関数に寄せ、store は判定の結果を集めて渡すだけにする。(b) を採った（store の行を増やさず、判定を store から切り離す）。kept の要素は (i) `flatten` で平たく並べる、(ii) `why` に入れ子にする。web の生成型が (i) で `task_id` を落とすので (ii) にした。
5. 新しい状態・分岐・fallback・port: event の種類 `DependentsReopened` と要素の型 2 つ。fallback・切替 flag・新しい port は無し。
6. 撤去対象: 無し（新しい記録の追加。旧い記録の仕組みは無い）。
7. 不変条件: 復帰と同じ transaction で A に 1 件だけ追記する。kept に人・CoS の cancel と連鎖の中止は入らない。done からの復帰では kept を数えない。空の記録と同じ内容の重ね書きは無い。既存の event は変わらない（下の試験）。

## 試験（守る不変条件と、既存の試験で検出できない理由）
既存の `dependency_reopen_` の試験は後続の状態と後続の events だけを見ており、A の側に記録が無い・余計な後続が入る・同じ記録が重なる不具合を検出できない。
- `dependency_reopen_records_restored_and_kept`（新規）: A の reopen（failed → ready）で A に記録が 1 件。restored は C（依存先 `[A]`）、kept は後継 B' を持つ B（`retried`）と、ほかの依存先 E が failed の D（`other_dependency`）。人が cancel した M と `parent_cancelled` の P は kept に無く、draft の後継 B' も入らない。B'・B・D・M・P の events（cancel と `Retried`）は変わらない。done の X の rereview・reopen では、ほかの依存先の失敗で cancel された Y と人が cancel した後続があっても記録を書かない。
- `dependency_reopen_skips_unchanged_record`（新規）: kept だけを持つ A を rereview → 不合格 → rereview と 2 回戻しても記録は 1 件（2 回目は遷移の 1 件だけ追記）。人が cancel した後続だけを持つ Z の reopen は記録を書かない。
- `dependency_reopen_restores_when_successor_drops_dependency`（確かめを追加）: 戻った B が A の記録の restored に依存先 `[A]` と一緒に入り、kept は空。
- `dependency_reopen_restores_cancelled_dependents_transitively`（期待を変更）: B の復帰が C を戻すので、B の events には遷移の後に restored C（依存先 `[B]`）の記録が続く。旧い期待「B に 1 件だけ追記する」は D7.3 で契約が変わった（戻した後続の遷移も同じ枝を通り、推移的に戻した後続も記録する）。既存の event が変わらないこと、C には遷移の 1 件だけが足されることは引き続き確かめる。A の記録が B だけを持つことも足した。

付記（2026-11-10）: unit [test-names](p9-inbox-test-names.md) が、上の 2 件目を `dependency_reopen_rereview_does_not_repeat_event` に改名し、1 件目の done の X の部分をそこへ移した（1 件目の名前は変えていない）。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-core --no-tests=fail -E 'test(/dependency_reopen_/)'`: exit 0、11 passed。
- `cargo test -p task-api --lib schema`・`cargo nextest run -p task-core -E 'test(event_row_schema_matches_committed)'`: 合格（schema の差分ゼロ）。
- web: `tsc -b` 合格、`vitest run api/realtime` 3 file・61 件合格。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: exit 0。先頭の check-health は verdict=pass、`size.responsibility-growth` は pass（worsened 0）、fail 級 0。new の 2 件は unit successors の進捗と同じ warning の候補（この unit が変えていない file）。nextest は 8173 passed・23 skipped、web は tsc -b 合格、vitest 104 file・711 件合格。
- 健全性の値（`size.responsibility-growth` と同じ数え方。base は `git merge-base HEAD main`）: store の module の行は base 20026、unit successors の後 19720、この unit の後 19727（+7）。`SqliteStore` の method は base 732、この unit の後 730（増えていない）。

## 未解決
- 無し。

## 提案
- 無し。
