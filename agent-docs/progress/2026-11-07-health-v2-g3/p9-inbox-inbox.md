---
title: 健全性v2 G3 P-9 inbox — 受信箱の種類 dependents_reopened（戻した後続がまだ動いていない間だけ出し、cancel を答えにする）
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 inbox — 受信箱の種類 dependents_reopened

WorkUnit `inbox`（P-9 (c)）。ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の D7.3 と決定 p9-scope の「受信箱にも記録する。戻した後続は項目に task id と依存先を書き、人が重複を cancel できるように」を実装した。正本は各依存先 A の最新の `Event::DependentsReopened`（unit `inbox-read` の `TaskStore::latest_event_rows(LatestEventKind::DependentsReopened)`）。基準は `$CELERIS_WU_BASE`。

## 変更
件名「P-9 inbox: 受信箱の種類 dependents_reopened を足す」の commit。
- `crates/task-ops/src/inbox/dependents_reopened.rs`（新規。`health_repeat.rs` と同じ形）: `attention_items` が `AttentionItem::DependentsReopened { task, pending, kept, seq, at }` を導く。
  - `pending` は `restored` のうち、今 draft・ready で、最後の `Transitioned` の理由が `dependency_reopened` の後続（`PendingDependent { task, depends_on }`）。最後の遷移を読むのは、今 draft・ready の後続だけ。
  - `pending` が空なら項目を出さない。kept だけの event と、戻した後続がすべて動き出したか終端になった event がこれに当たる。
  - `focus`（P4-C の範囲つきの組み立て）では、A か `restored` の後続が範囲にある event だけを見る。
- `crates/task-ops/src/inbox.rs`: variant `AttentionItem::DependentsReopened`、`attention_at` の行、`build_attention` の呼び出し 1 行、`pub mod dependents_reopened`。
  - `attention_task` は `None` にした。A が done になった後も、戻した後続が動き出すまでは重複を cancel できる。A の状態で隠すと（`TerminalTask`）、その間に項目が消える。
- `crates/task-ops/src/human_inbox.rs`: `InboxKind::DependentsReopened`（`"dependents_reopened"`。`ALL` は 26）、`route_attention` と `attention` の行、`pub mod dependents_reopened`。
- `crates/task-ops/src/human_inbox/dependents_reopened.rs`（新規）: 項目の組み立て。
  - id は `dependents_reopened-<A の id>-<event の seq>`。
  - 本文には、まだ動いていない後続の title・task id・依存先と、kept の理由を書く。kept の理由は「retry の後継 <id> があるので戻さない」か「ほかの依存先 <id> が failed・cancelled のまま。それが戻ると戻る」。
  - 選択肢は、まだ動いていない後続ごとの `cancel:<task id>`。native は、後続が 1 件のときだけ `POST /api/v1/tasks/{id}/cancel`（`draft_accept` と同じ）。
  - `cancel_target(item, option)` が選択肢の key を後続の id にし、項目の `blocking.tasks`（まだ動いていない後続）に無い id を `None` にする。
- `crates/task-ops/src/human_inbox/item_task.rs`: `TASK_KEYED` に `dependents_reopened`（12 件）。
- `crates/task-api/src/inbox_notifications.rs`: `delegated_request` の網羅の match に `InboxKind::DependentsReopened` を足した。`cancel_target` で後続を引いて `POST /api/v1/tasks/{id}/cancel` に写す。引けない id は 400。
- `docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1`）、`web/api/generated/{schema.json,types.ts}`（`gen:types`）を作り直した。`docs/api/v1/api.md` の改訂の行と `attention[]` の説明に `dependents_reopened` を足した。
- `web/features/inbox/inbox-model.ts`: `INBOX_KINDS`・`KIND_LABELS`（「依存先の復帰で戻した後続」）に行を足した。`isDestructive` は key の `:` より前の操作名で見分けるので、`cancel:<id>` は確認を挟む破壊的な選択になる。
- `docs/architecture-map.md`: 受信箱の行に、新しい 2 つの module と試験を足した。

## 実装前 brief
1. 根本原因: unit `event` が A の events に `DependentsReopened` を書くが、受信箱はこれを読まない。そのため、決定 p9-scope の「戻した後続を受信箱に出し、人が重複を cancel できる」が無い。
2. 責務 owner: 導出は `task_ops::inbox`（attention の導出。`health_repeat` と同じ置き場）。項目の共通形は `task_ops::human_inbox`。answer の写しは `task_api::inbox_notifications::delegated_request`。
3. 既存の再利用先: `latest_event_rows`（unit `inbox-read`）、`health_repeat` の module の形、`draft_accept` の native（1 件のときだけ）、`Builder::finish` と `root_of`、web の `isDestructive`。
4. 選択肢:
   - 選択肢の形について、(a) 1 つの `cancel` と payload の task id、(b) 後続ごとの `cancel:<id>`、を考えた。(b) を採った。目的の「選択肢は戻した後続ごとの cancel」に合い、`validate_answer` の「項目に無い選択肢は 400」がそのまま効く。
   - 選択肢の key を解く処理は `human_inbox::dependents_reopened::cancel_target` の 1 か所に置き、task-api はそれを呼ぶ。key の書式を 2 か所に持たない。
5. 新しい状態・分岐・fallback・port:
   - 受信箱の種類 1 つと attention の variant 1 つ。表・config の欄は無し。fallback・切替 flag は無し。
   - web の `isDestructive` は、`<操作>:<対象>` の key を操作名で見分ける。
6. 撤去対象: 無し（新しい種類の追加で、置き換える仕組みは無い）。
7. 不変条件と試験:
   - 戻した後続がまだ動いていない間だけ出る。kept だけでは出ない。答えで cancel できるのは項目が挙げた後続だけ。
   - 確かめる試験は下の 3 件。

## 試験（守る不変条件と、既存の試験で検出できない理由）
- `dependents_reopened_inbox_item_lifecycle`（`crates/task-ops/src/inbox/dependents_reopened/tests.rs`）:
  - 守る不変条件: A の reopen で項目が出て、本文に戻した後続 B の id・依存先 A と、kept の理由（C は retry の後継 C2、D はほかの依存先 E）がある。選択肢は `cancel:<B>`。
  - kept だけの event（K）では項目が出ない。X の項目は、人が後続 Y を cancel すると（終端）消える。A の項目は、B が running になると消える。B が `infra_requeue` で ready に戻っても出ない。
  - 既存の試験で検出できない理由: task-core の `dependency_reopen_` は A の events の記録までしか見ない。`human_inbox_every_inbox_kind_*` は作った `AttentionItem` を写すだけで、event からの導出を通らない。
  - 変異の確かめ: 最後の遷移の判定を「Transitioned があれば真」に変えると、この試験が落ちた（「動いた後の ready は戻した後続ではない」）。元に戻した。
- `dependents_reopened_answer_cancels_restored_dependent`（`crates/task-api/tests/inbox_notifications.rs`）:
  - 守る不変条件: answer の `cancel:<B>` は `POST /api/v1/tasks/<B>/cancel` に写り（`delegated_to`）、B だけが cancelled になる。
  - 項目が挙げていない id（無関係の task・依存先 A 自身・cancel 済みの B）は 400 で、状態は変わらない。最後の後続 C を cancel すると `removed: true` になる。
  - 既存の試験で検出できない理由: 既存の answer の試験は質問・決定・統合依頼だけを通り、選択肢の key に task id を持つ種類の写しを通らない。
- web `inbox-model.test.ts` の「dependents_reopened の後続ごとの cancel を破壊的な選択として見分ける」:
  - 守る不変条件: `cancel:<id>` は破壊的な選択。`keep:<id>` は破壊的でない。種類 `dependents_reopened` と表示名がある。この種類は専用画面への誘導にしない。
  - 既存の試験で検出できない理由: 既存の試験は key そのものが操作名の選択しか見ない。
- 既存の試験の更新: `human_inbox/tests.rs` の `variant_index`（25）と全種類の土台に `DependentsReopened` を足した（`human_inbox_every_inbox_kind_appears_in_common_form` と `human_inbox_all_contains_every_variant_once` が新しい種類も通る）。
- ADR-0133 の読み出し回数・lock 回数の試験は変えていない。
  - 受信箱の組み立ての読み取りは 1 回増える（`latest_event_rows` の 1 回）。加えて、まだ動いていない後続 1 件につき 1 回増える。
  - `inbox_blocked_events_are_read_once_per_build` は 11 件と 1 件の差を見るので、定数の 1 回は差に出ない。`request_lock_counts` の上限（task 1 件あたり 2 回）にも収まる。どちらも提出時の test-affected で合格した。

## 証拠（提出時の HEAD で実行）
- `cargo nextest run -p task-ops --no-tests=fail -E 'test(/dependents_reopened|human_inbox_/)'`: exit 0、25 passed。
- `cargo nextest run -p task-api --no-tests=fail -E 'test(/dependents_reopened|schema/)'`: exit 0。新しい試験 1 件と schema の試験 4 件が合格した。
- `corepack pnpm@12.6.0 -C web exec tsc -b`: exit 0。`vitest run features/inbox`: 6 passed。
- `python3 scripts/dev/check-architecture-map.py`: OK（990 件）。`sh scripts/dev/check-doc-links.sh`: ok。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: 結果は下の追記。

## 未解決
- 無し。

## 提案
- unit `record` が ADR (7) の付記に書く: `attention_task` を `None` にした理由（A が done になっても、戻した後続が動き出すまでは重複を cancel できる）。選択肢の key `cancel:<task id>` の書式。
