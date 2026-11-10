---
title: 健全性v2 G3 P-9 — retry の後継を持つ後続を両経路で戻さず、依存先の reopen で戻した後続を受信箱に出す
tasks: [01M71K2HHSJ20836QAMXJ0YMEW]
status: done
updated: 2026-11-10
completed: 2026-11-10
---
# 健全性v2 G3 P-9 — retry の後継を持つ後続を両経路で戻さず、依存先の reopen で戻した後続を受信箱に出す

完了日: 2026-11-10

task 01M71K2HHSJ20836QAMXJ0YMEW の記録。G3 の final review（reviewer 01M71G34Y5DVAMXYK493P24P4V）の差し戻し (1) を直した。人の決定 p9-scope（後続を戻し、受信箱にも記録する。戻した後続は項目に task id と依存先を書き、人が重複を cancel できるようにする）と、ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の D7.2・D7.3 を満たす。前の記録は [p9](p9.md)。基準は `git merge-base HEAD main`（WorkUnit は `$CELERIS_WU_BASE`）。

依存先を A、その後続を B、B の retry の後継を B' と書く。

## unit ごとの記録
- [successors](p9-inbox-successors.md)（P-9 (a)）: `retry_successors_tx` と共有の述語 `cancelled_by_dependency`。後継を持つ B は A の reopen でも A の retry でも戻さない。
- [event](p9-inbox-event.md)（P-9 (b)）: A の終端からの復帰で、A の events に `DependentsReopened { restored, kept }` を同じ transaction で 1 件追記する。
- [inbox-read](p9-inbox-inbox-read.md)（P-9 (c) 前段）: 最新行の読み出しを `TaskStore::latest_event_rows(LatestEventKind)` に一般化し、migration 0122 で `dependents_reopened` の部分 index を足した。
- [inbox](p9-inbox-inbox.md)（P-9 (c)）: 受信箱の種類 `dependents_reopened`。戻した後続がまだ動いていない間だけ出し、答えは戻した後続の cancel。
- record（この unit）: ADR (7) の付記「unit p9-inbox」、closeout の付記 (c) の訂正、この file、[p9](p9.md) の「未解決」への 1 行ずつの追記。

## 変更（commit の件名）
- 「P-9 successors: retry の後継を持つ後続を reopen と retry の両経路で戻さない」
- 「P-9 event: 依存先の終端からの復帰で DependentsReopened を記録する」
- 「integrate wu/event (phase core)」
- 「P-9 inbox-read: 最新行の読み出しを種類で一般化し、dependents_reopened の部分 index を足す」
- 「P-9 inbox: 受信箱の種類 dependents_reopened を足す」
- 「integrate wu/inbox (phase inbox)」
- 各 unit の「進捗に提出時の検査の結果を書く」
- 「P-9 record: ADR (7) の付記と closeout の付記 (c) を実装に合わせ、p9-inbox の進捗を書く」（この unit。コードは変えていない）

## 受け入れの 6 件の試験と守る不変条件
task の受け入れ条件 0 の 6 件の内容と、それを確かめる試験。`dependency_reopen_` は 11 件で、unit p9 の 5 件にこの task が 6 件を足した（`dependency_reopen_keeps_dependent_with_retry_successor`・`dependency_reopen_retry_skips_dependent_with_successor`・`dependency_reopen_restores_when_successor_drops_dependency`・`dependency_reopen_trigger_distinguishes_kept_reasons`・`dependency_reopen_records_restored_and_kept`・`dependency_reopen_skips_unchanged_record`）。受信箱の 3 件の内容は、1 つの試験 `dependents_reopened_inbox_item_lifecycle` が順に確かめる。
1. A の reopen で retry の後継を持つ B は戻らず、kept に retried と後継が入る。
   - `dependency_reopen_keeps_dependent_with_retry_successor`（task-core）: B は cancelled のまま、理由は `dependency_failed` だけ。B' は draft のまま。後継の無い C は ready に戻る。
   - `dependency_reopen_records_restored_and_kept`（task-core）: A の記録の kept に B が `retried`（後継 B'）で入る。
   - 不変条件: 後継を持つ後続は戻さない（D7.2）。
2. A の retry で B は付け替えず cancelled のまま、B' は A' に付け替わる。
   - `dependency_reopen_retry_skips_dependent_with_successor`（task-core）: B の `depends_on` は A のまま。B' は draft のまま A' に付け替わる。後継の無い C は A' に付け替わって draft に戻る。
   - 不変条件: retry の付け替えと reopen の復帰は同じ `retry_successors_tx` と同じ述語を使う（D7.2）。
3. 項目に restored と kept と理由が出る。
   - `dependents_reopened_inbox_item_lifecycle`（task-ops）: A の項目の本文に、戻した B の id と依存先 A、kept の C（retry の後継 C2）と D（ほかの依存先 E）がある。選択肢は `cancel:<B>` だけで、native は `POST /api/v1/tasks/<B>/cancel`。
   - 不変条件: 決定 p9-scope の「戻した後続は項目に task id と依存先を書く」。cancel できるのは項目が挙げた後続だけ。
4. 戻した後続が動くと項目が消える。
   - `dependents_reopened_inbox_item_lifecycle`: B が running になると A の項目が消え、B が `infra_requeue` で ready に戻っても出ない。人が戻した後続 Y を cancel する（終端）と X の項目が消える。
   - 不変条件: ADR-0133 の「答えると消える」。
5. kept だけの event は項目を出さない。
   - `dependents_reopened_inbox_item_lifecycle`: restored が空で kept が 1 件の K の event では項目が出ない。
   - 不変条件: 人が答えることの無い項目を出さない（ADR (7) の付記「unit p9-inbox」）。
6. 人が cancel した後続を持つ done の A の Rereview では event を書かず、同じ kept の復帰を繰り返しても event は 1 件。
   - `dependency_reopen_records_restored_and_kept`（task-core）: done の X の rereview・reopen では、人が cancel した後続とほかの依存先の失敗で cancel された Y があっても記録を書かない。
   - `dependency_reopen_skips_unchanged_record`（task-core）: kept だけを持つ A を rereview → 不合格 → rereview と 2 回戻しても記録は 1 件。人が cancel した後続だけを持つ Z の reopen は記録を書かない。
   - 不変条件: kept に人・CoS の cancel と連鎖の中止を入れない。kept は A の終端が failed・cancelled のときだけ数える。同じ記録を重ねない（D7.3 を狭める付記）。

補う試験: `dependency_reopen_restores_when_successor_drops_dependency`（B' の `depends_on` から A を外すと B は戻って restored に出る）、`dependency_reopen_trigger_distinguishes_kept_reasons`（純粋関数が `Restore`・`Kept(Retried)`・`Kept(OtherDependency)`・`Other` を区別する）、`dependents_reopened_latest_rows_use_partial_index`（読み出しが events を部分 index でだけ読む）、`dependents_reopened_answer_cancels_restored_dependent`（task-api。答えの `cancel:<B>` は B だけを cancel し、項目に無い id は 400）、web の `inbox-model.test.ts` の「dependents_reopened の後続ごとの cancel を破壊的な選択として見分ける」。

## 証拠（統合後の HEAD で実行。提出時の HEAD で実行）
- `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/dependency_reopen_|dependents_reopened/)'` → exit 0、14 passed（`dependency_reopen_` 11 件、`dependents_reopened_` 3 件）。
- `corepack pnpm@12.6.0 -C web exec vitest run features/inbox` → exit 0、1 file・6 passed。
- 文書の検査・`bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は下の「提出時の検査」に書く。

## 提出時の検査

## 未解決
- 計画を持つ後続（木の親）が戻っても、その子 task（`parent_cancelled`）と work unit は戻らない（[p9](p9.md) の「未解決」の 3 点目のまま。この task の範囲の外）。
- 旧い名前 `latest_delivery_skipped_rows` が 2 か所に残る（[inbox-read](p9-inbox-inbox-read.md) の「未解決」）。migration 0037 の注釈は、既存の migration を変えない決まりなのでそのまま残す。ADR-0121 の付記は、この unit の目的の file に入っていないので変えていない（下の「提案」）。

## 提案
- ADR [0121-root-delivery-without-assignee](../../adr/0121-root-delivery-without-assignee.md) の付記に、「読み出しは `TaskStore::latest_event_rows(LatestEventKind::DeliverySkipped)` に一般化した（ADR 2026-11-08-health-enforcement-loop D7.3、migration 0122）」の 1 行を足す。
