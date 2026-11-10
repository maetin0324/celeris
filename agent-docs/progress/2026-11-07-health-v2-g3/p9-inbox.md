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
- record: ADR (7) の付記「unit p9-inbox」、closeout の付記 (c) の訂正、この file、[p9](p9.md) の「未解決」への 1 行ずつの追記。
- [test-names](p9-inbox-test-names.md): task の受け入れ条件 0 は 6 件の試験を関数名で grep する。それに合わせて試験を改名・分割した（製品のコードは変えていない）。旧名 → 新名の対応は次のとおり。
  - task-core: `dependency_reopen_keeps_dependent_with_retry_successor` → `dependency_reopen_keeps_retried_dependent`。
  - task-core: `dependency_reopen_retry_skips_dependent_with_successor` → `dependency_reopen_retry_skips_retried_dependent`。
  - task-core: `dependency_reopen_skips_unchanged_record` → `dependency_reopen_rereview_does_not_repeat_event`。`dependency_reopen_records_restored_and_kept` の後半（done の X の部分）もこちらに移した。
  - task-ops: `dependents_reopened_inbox_item_lifecycle` → `dependency_reopen_inbox_lists_restored_and_kept`・`dependency_reopen_inbox_item_resolves_when_dependents_move`・`dependency_reopen_kept_only_event_has_no_inbox_item` の 3 件に分けた。
- names-record（この unit）: ADR (7) の付記「unit p9-inbox」の試験の行と、この file の受け入れの節を新しい名前にした。[successors](p9-inbox-successors.md)・[event](p9-inbox-event.md)・[inbox](p9-inbox-inbox.md) の進捗に、改名を知らせる 1 行ずつを足した。コードは変えていない。

## 変更（commit の件名）
- 「P-9 successors: retry の後継を持つ後続を reopen と retry の両経路で戻さない」
- 「P-9 event: 依存先の終端からの復帰で DependentsReopened を記録する」
- 「integrate wu/event (phase core)」
- 「P-9 inbox-read: 最新行の読み出しを種類で一般化し、dependents_reopened の部分 index を足す」
- 「P-9 inbox: 受信箱の種類 dependents_reopened を足す」
- 「integrate wu/inbox (phase inbox)」
- 各 unit の「進捗に提出時の検査の結果を書く」
- 「P-9 record: ADR (7) の付記と closeout の付記 (c) を実装に合わせ、p9-inbox の進捗を書く」（コードは変えていない）
- 「integrate wu/record (phase close)」
- 「P-9 test-names: 受け入れ条件 0 の 6 件の試験を指定の名前にする」（試験の名前と分け方だけ）
- 「P-9 names-record: ADR (7) の付記と p9-inbox の進捗の試験名を新しい名前に合わせる」（この unit。文書だけ）

## 受け入れの 6 件の試験と守る不変条件
task の受け入れ条件 0 の 6 件の内容と、それを確かめる試験（1 件に 1 つ）。名前が `dependency_reopen_` で始まる試験は 14 件ある。task-core の 11 件（unit p9 の 5 件と、この task の 6 件）と task-ops の 3 件。
1. A の reopen で retry の後継を持つ B は戻らず、kept に retried と後継が入る。
   - `dependency_reopen_keeps_retried_dependent`（task-core）: B は cancelled のまま、理由は `dependency_failed` だけ。B' は draft のままで、event も増えない。後継の無い C は ready に戻る。A の記録は 1 件で、restored が C（依存先 `[A]`）、kept が B（`retried`、後継 B'）。
   - 不変条件: 後継を持つ後続は戻さない（D7.2）。
2. A の retry で B は付け替えず cancelled のまま、B' は A' に付け替わる。
   - `dependency_reopen_retry_skips_retried_dependent`（task-core）: 付け替わるのは B' と C だけ。B は cancelled のまま、`depends_on` は A のまま。B' は draft のまま A' に付け替わる。後継の無い C は A' に付け替わって draft に戻る（理由 `retried`）。
   - 不変条件: retry の付け替えと reopen の復帰は同じ `retry_successors_tx` と同じ述語を使う（D7.2）。
3. 項目に restored と kept と理由が出る。
   - `dependency_reopen_inbox_lists_restored_and_kept`（task-ops）: A の項目の本文に、戻した B の id と依存先 A、kept の C（retry の後継 C2）と D（ほかの依存先 E）がある。選択肢は `cancel:<B>` だけで、native は `POST /api/v1/tasks/<B>/cancel`。`cancel_target` は戻していない D を返さない。
   - 不変条件: 決定 p9-scope の「戻した後続は項目に task id と依存先を書く」。cancel できるのは項目が挙げた後続だけ。
4. 戻した後続が動くと項目が消える。
   - `dependency_reopen_inbox_item_resolves_when_dependents_move`（task-ops）: 人が戻した後続 Y を cancel する（終端）と X の項目が消える。B が running になると A の項目が消え、B が `infra_requeue` で ready に戻っても出ない。
   - 不変条件: ADR-0133 の「答えると消える」。
5. kept だけの event は項目を出さない。
   - `dependency_reopen_kept_only_event_has_no_inbox_item`（task-ops）: restored が空で kept が 1 件の K の event では項目が出ない。同じ store の A には項目が出る。
   - 不変条件: 人が答えることの無い項目を出さない（ADR (7) の付記「unit p9-inbox」）。
6. 人が cancel した後続を持つ done の A の Rereview では event を書かず、同じ kept の復帰を繰り返しても event は 1 件。
   - `dependency_reopen_rereview_does_not_repeat_event`（task-core）: done の X の rereview の直後も、その後の reopen の後も、人が cancel した後続と、ほかの依存先 F の失敗で cancel された Y があっても記録を書かない。kept だけを持つ A を rereview → 不合格 → rereview と 2 回戻しても記録は 1 件（2 回目は遷移の 1 件だけ追記）。人が cancel した後続だけを持つ Z の reopen は記録を書かない。
   - 不変条件: kept に人・CoS の cancel と連鎖の中止を入れない。kept は A の終端が failed・cancelled のときだけ数える。同じ記録を重ねない（D7.3 を狭める付記）。

補う試験:
- `dependency_reopen_records_restored_and_kept`（task-core）: A の reopen で A の記録が 1 件。restored は C、kept は B（`retried`）と D（`other_dependency`）。人が cancel した M と `parent_cancelled` の P は kept に無く、draft の後継 B' も入らない。
- `dependency_reopen_restores_when_successor_drops_dependency`（task-core）: B' の `depends_on` から A を外すと、B は戻って restored に出る。
- `dependency_reopen_trigger_distinguishes_kept_reasons`（task-core）: 純粋関数が `Restore`・`Kept(Retried)`・`Kept(OtherDependency)`・`Other` を区別する。
- `dependents_reopened_latest_rows_use_partial_index`（task-core）: 読み出しが events を部分 index でだけ読む。
- `dependents_reopened_answer_cancels_restored_dependent`（task-api）: 答えの `cancel:<B>` は B だけを cancel し、項目に無い id は 400。
- web の `inbox-model.test.ts` の「dependents_reopened の後続ごとの cancel を破壊的な選択として見分ける」。

## 証拠（提出時の HEAD で実行）
- 受け入れ条件 0 のコマンド（6 件の `fn` の grep と `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/dependency_reopen_/)'`）→ exit 0、14 passed（task-core 11 件・task-ops 3 件）。
- `cargo nextest run -p task-core -p task-ops -p task-api --no-tests=fail -E 'test(/dependency_reopen_|dependents_reopened_/)'` → exit 0、16 passed（補う試験の `dependents_reopened_` 2 件を含む）。
- ADR [2026-11-08-health-enforcement-loop](../../adr/2026-11-08-health-enforcement-loop.md) の旧名 4 件の grep → 一致なし（exit 1）。新しい名前 6 件は (7) の付記「unit p9-inbox」の試験の行に 1 回ずつある。

## 提出時の検査
提出時の HEAD（unit names-record の commit の後）で実行した。unit names-record の変更は文書 5 file だけ（範囲 check `sh "$CELERIS_WU_SCOPE_PATHS"` の出力は ADR・この file・successors・event・inbox の進捗の 5 件）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。
- `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（413 files）。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。
- `python3 scripts/dev/check-human-text.py` → exit 0（0 findings）。`python3 scripts/dev/check-architecture-map.py` → OK（990 件）。
- `bash scripts/dev/test-affected.sh` → exit 0。「試験対象の変更なし（docs のみ等）」で nextest は走らない。先頭の check-health は verdict=pass、fail 級 0、worsened 0。new の 2 件は unit successors 以降の進捗と同じ warning の候補（unit names-record が変えていない file）。`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決
- 計画を持つ後続（木の親）が戻っても、その子 task（`parent_cancelled`）と work unit は戻らない（[p9](p9.md) の「未解決」の 3 点目のまま。この task の範囲の外）。
- 旧い名前 `latest_delivery_skipped_rows` が 2 か所に残る（[inbox-read](p9-inbox-inbox-read.md) の「未解決」）。migration 0037 の注釈は、既存の migration を変えない決まりなのでそのまま残す。ADR-0121 の付記は、unit record の目的の file に入っていないので変えていない（下の「提案」）。

## 提案
- ADR [0121-root-delivery-without-assignee](../../adr/0121-root-delivery-without-assignee.md) の付記に、「読み出しは `TaskStore::latest_event_rows(LatestEventKind::DeliverySkipped)` に一般化した（ADR 2026-11-08-health-enforcement-loop D7.3、migration 0122）」の 1 行を足す。
