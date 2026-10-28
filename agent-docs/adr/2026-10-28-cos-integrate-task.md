# ADR 2026-10-28: 機械的に解けない統合の衝突を、CoS が integrate task として依頼する

- 状態: 実装済み（2026-10-28）
- tasks: [01M60AMT97YPAFYRT9H0E3F1Z5]
- 関係: ADR 2026-10-02-parallel-integration-auto-resolve（D4 統合の依頼）、ADR 2026-10-27-done-task-auto-reintegrate（D4 衝突）、
  ADR 2026-10-09-cos-operations-all-mutations（登録表）、ADR 2026-10-05-cos-chat-home（CoS 受信箱の一次対応）、
  ADR 2026-10-27-event-principals（主体）、ADR 2026-10-27-external-operator

## 問題

配送（origin `delivery`）の統合の依頼（`IntegrationRequested`）は、auto_resolve で解けない衝突（`NeedsHuman`・上限）と
done 後の自動の再取り込みの衝突（ADR 2026-10-27-done-task-auto-reintegrate D4）で記録される。受信箱の選択肢は
「統合した」「見送る」「再試行する」の 3 つで、どれも回答を記録するだけである。衝突を解く手段は人の手 merge しかない。

- 2026-10-28 時点で `integration_answered` 85 件のうち 67 件を外部 operator が答え、13 件を手で no-ff merge した。
- CoS は受信箱の一次対応で統合の依頼を受けても `native_action_required`（409）で答えられず、escalate するしかない。
- 手 merge は build・試験をしない。C3 の stagnation 試験が削除済みの欄を使ったまま main に入り、main が compile できなくなった。

## 決定

### D1. integrate task: 衝突を解くのは worker、取り込みを決めるのは検査と reviewer

新しい操作 `POST /api/v1/tasks/{id}/integration-requests/{request}/integrate-task`（body `{"note": 任意}`）は、
元の task の未回答の統合の依頼 `{request}`（`IntegrationRequest::id_for`）について、**通常の root task を 1 件起票する**。

- 案件・リポジトリは元の task と同じ。genre・role・skills も元の task から継ぐ。担当とモデルの段は celeris が通常どおり決める。
- worktree は通常どおり既定ブランチの先端から切る。objective は worker に「`git merge --no-ff <source_sha>`（元の task の head）を
  取り込み、衝突を両側の意図を保って解き、`[commands] check` を通して commit する」ことを指示し、依頼の markdown
  （両側の commit・衝突 file）を写す。
- 受け入れ条件（決定的）:
  1. `git merge-base --is-ancestor <source_sha> HEAD && git merge-base --is-ancestor <target_sha> HEAD`（両側を取り込んだこと）。
  2. 元の task の先頭リポジトリの `workspace.toml` `[commands] check` の各行（agent-platform では test-affected・clippy・fmt）。
     reviewer は ADR 2026-10-23-required-checks-bound-to-candidate の必須検査を候補 SHA で再実行する。
  3. reviewer の判定（`Check::Reviewer`）。worker の自己申告では done にならない。
- 取り込みは通常の配送（review 前同期 → 必須検査 → reviewer → `merge --ff-only`）。integrate task の branch は main を基点に
  元の head を merge したものなので、main に入ると元の head も main の祖先になる。CoS も操作も git を直接触らない。

**却下した案。** (a) 元の task を reopen して IntegrationRepair WU を足す: done の task の状態と配送の記録を巻き戻すことになり、
reviewer の合格の束縛（`(repo, target_sha, reviewed_sha)`）と配送の Blocked の扱いが複雑になる。(b) CoS に
`/integrations/{repo}/merge`（no-ff の手 merge）を許す: 検査を通らない取り込みで、今回の事故の経路そのもの。

### D2. 結び: 元の task に event を追記する

- 起票と同じ transaction で、元の task に `Event::IntegrateTaskRequested { request_id, integrate_task_id, by }` を追記する
  （`by` は ADR 2026-10-27-event-principals の主体。CoS は `cos:<thread>`、人は `human:<name>`）。新しい task の
  `Created.created_by` も同じ主体。migration は要らない（events は追記専用）。
- 1 つの依頼に結べる integrate task は 1 件。進行中（未決着）なら 409 `integrate_task_exists`、失敗して人に回した後も
  409 `integrate_task_escalated`（2 回目は人が決める）。origin が `delivery` でない依頼（段の統合 `phase:*` は target が親の
  branch）は 422 `integrate_task_unsupported_origin`。元の task に案件・リポジトリが無ければ 422 `repository_required`。

### D3. 決着: 決定的な sweep が元の依頼を閉じる、失敗は人に回す

`task_ops::integrate_task::settle` が、stranded sweep（300 秒、single-flight）の中で reintegrate の後に走る。LLM は呼ばない。

- integrate task が `done` で、元の task のリポジトリで `source_sha` が既定ブランチ（依頼の `target_branch`）の祖先なら、
  元の依頼を `IntegrationAnswered { answer: "integrated", by: "daemon" }` で閉じ、
  `Event::IntegrateTaskSettled { outcome: "integrated" }` を元の task に追記する。元の task の取り込み状態は既存の
  stranded sweep が `integrated` にする。
- integrate task が `failed` / `cancelled` なら、`IntegrateTaskSettled { outcome: "escalated" }` を追記する。依頼は閉じない。
- `done` でもまだ main に入っていなければ待つ（integrate task 自身の配送・統合の依頼が受信箱に出る）。

### D4. 受信箱と CoS

- 統合の依頼（origin `delivery`、結び無し）の受信箱項目に選択肢 `integrate_task`（「integrate task に解かせる」）と
  native `POST …/integrate-task` を足す。人の `/inbox/items/{i}/answer` も CoS の `/cos/inbox/{i}/resolve`（outcome=answer）も
  同じ操作関数に入る。
- 結んだ integrate task が進行中の間、元の依頼は受信箱に出さない（integrate task が扱っている）。
- `escalated` の依頼は受信箱に戻り、題に「integrate task が失敗」、本文に task id と状態を書き、`integrate_task` の選択肢を出さない。
  `human_required_reason` はこの項目に理由を返すので、CoS は answer できず escalate する（人に回す）。
- CoS の登録表: tasks 領域の ALLOWED に `integration.integrate_task` を足す（外部効果なし。task の起票・結びの event・
  `cos_operations` 行・監査 envelope が 1 transaction）。skill `cos-operator` の操作表と `cos-inbox-triage` の判断に、
  統合の依頼は integrate task で答え、失敗した件は escalate することを書く。

### D5. event・API の付帯

- `Event::IntegrateTaskRequested` / `IntegrateTaskSettled` の kind 名（`query.rs`）・web の `event-kinds.ts` /
  `invalidation-map.ts`・schema の再生成。`docs/api/v1/api.md` に endpoint を足す。
- 消す旧経路は無い。統合の依頼の 3 つの回答（integrated/declined/retry）は人の記録として残す。手 merge の
  `/integrations/{repo}/merge` は人だけの操作（EXCLUDED）のまま。

## 本番への影響

- skill の変更（`config/skills/cos-operator`・`cos-inbox-triage`）は本番 KB の `skills/` に人が取り込むまで CoS に届かない。
- config 欄の追加・削除は無い。

## Consequences

- 機械的に解けない衝突も、CoS が integrate task を起票すれば、worker が解き、決定的な検査と reviewer が判定してから main に入る。
- 失敗した件だけが人に回り、受信箱に task id と状態が残る。
- integrate task の分だけ worker と reviewer の費用がかかる。1 依頼 1 件に限るので連鎖はしない。
