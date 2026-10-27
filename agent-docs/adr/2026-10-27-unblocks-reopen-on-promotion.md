# 修正 task と待っている task を `unblocks` で結び、修正を含む release の昇格で待っている task を reopen する

- 日付: 2026-10-27
- 状態: 採用・実装済み
- task: 01M5XXF7QMD919KZ07G80MESYF
- 関連: [ADR-0041](0041-self-improvement-loop-hardening.md) D3（`promoted.json`）、
  [ADR-0044](0044-task-management.md) D2（`reopen`・コメント）、
  [2026-10-04-release-notes](2026-10-04-release-notes.md)（昇格の要約 `included.tasks`）

## 背景

「修正 task が done → その commit を含む release → 昇格 → 待っていた task を reopen」の流れを結ぶものが無く、
operator が毎回手でつないだ（2026-10-26 の 01M5WC8PPW は、修正 01M5WCJ9ZM・01M5WMNDA8・01M5WTKE9M の昇格ごとに
operator が reopen した）。`promote.sh` が書く `promoted.json` の `included.tasks` は含まれる task の一覧だけで、
「どの task がその修正を待っているか」の記録はどこにも無い。

## 決定

- D1. 結びは**修正 task の作成時**に書く。`POST /api/v1/tasks` の本文（`NewTaskBody`）に `unblocks: [task_id]`
  （最大 8 件、重複不可、存在する task だけ。無い id は 422）を足した。CoS の `task.create` は同じ本文を同じ関数
  （`create_task_op`）で通すので、API と CoS op の両方で書ける。結びは表 `task_unblocks`（migration 0096。
  `(fix_task_id, waiting_task_id)` が主キー）に task と**同じ transaction**で書く。
  - 待っている task の側には書かない（待っている側は既に失敗して終端にいることが多く、修正 task を起こすのは
    後から。作成時に書く方が自然で、待っている task の編集を要らない）。
- D2. 昇格の判定は daemon の周期処理（`active` だけ、300 秒ごと、`stranded_sweep` と同じ間隔・単発実行の規律）が
  `[selfdeploy] releases_dir` の各 `<sha12>/promoted.json` を読んで行う。修正 task が「release に入った」とは、
  その `included.tasks[].task_id` に**修正 task 自身かその祖先**の id があること（子 task は親の配送で入るため）。
  昇格した release はすべて見る（2 つの release が 1 周期の間に昇格しても取りこぼさない）。決定的（JSON と DB だけ。
  git も LLM も使わない）。
  - `included` が `null`（preview が取れなかった昇格）や `complete: false` で一覧から漏れた修正は検出しない。
    そのときは従来どおり人が reopen する（推測で git の祖先判定を足さない）。
- D3. **reopen を選ぶ**（CoS への通知ではない）。結びは人か CoS が「この修正が入ったら再開する」と明示した意思で、
  operator が手で行っていたのも reopen だった。通知にすると「通知を読んで reopen する」という同じ手作業が残る。
  - 待っている task が `failed` なら、system コメント（「修正 task <id> を含む release <sha12> が昇格したので
    再開した」）と `Trigger::Reopen` を**同じ transaction**で書く（`TaskStore::comment_add` の遷移付き）。コメントは
    次の run の前置きに載るので、再開した run は理由を知る。
  - `blocked` なら状態は変えず、同じ文の system コメントだけを残す（`blocked` は phase gate・計画の承認・browser・
    cluster job など人や外部の待ちで、昇格を理由に勝手に外すと人の gate を飛ばすため）。
  - それ以外（`ready`・`running`・`reviewing`・`done`・`cancelled`・`draft`。人が既に reopen した等）は何もしない。
- D4. 一度だけ。結びの行に `fired_release`・`fired_action`（`reopened` / `commented` / `skipped`）・`fired_at` を書き、
  書いた行は二度と見ない（`fired_at IS NULL` の行だけが対象。書き込みは `WHERE fired_at IS NULL` の条件付き更新）。
  二度目の昇格（修正を含む後の release）や再起動後の周期でも同じ結びで二度 reopen しない。reopen の後に印の書き込みが
  失敗しても、次の周期では待っている task が `ready` 以降なので `skipped` になり、二度目の reopen は起きない。
- D5. 旧い仕組みは無い（operator の手作業の置き換え）。消すコード経路・config 欄は無い。

## 影響

- API: `NewTaskBody.unblocks`（任意）を足した（`docs/api/v1/api-v1.schema.json`・web の生成型を再生成）。
- DB: migration 0096（`task_unblocks`、SCHEMA_VERSION 96）。追加だけでデータは移さない。
- 本番 config の変更は不要（`[selfdeploy] releases_dir` を読むだけ）。
