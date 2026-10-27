# 案件の状態の要約（project brief）を構造化して永続化し、secretary_summary を消す

- 日付: 2026-10-27
- 状態: 採用・実装済み
- task: 01M5XXF7QH5BSQJTVW7ETJ3Y6T
- 関連: [ADR-0033](0033-organization-projects-and-reports.md)（`projects.secretary_summary`）、
  [ADR 2026-10-09-cos-operations-all-mutations](2026-10-09-cos-operations-all-mutations.md)（CoS op の台帳）、
  [ADR 2026-10-22-cos-fixed-threads](2026-10-22-cos-fixed-threads.md)（統括 CoS と案件 CoS の固定スレッド）

## 背景

案件ごとの「今の状態」（目的・現状・未解決・次の手・待っている相手・根拠）が永続化されていない。統括 CoS は全 task 木を
読むか operator の会話の記憶に頼るしかなかった。`projects.secretary_summary`（ADR-0033、migration 0006）は列だけあって
書き込む経路が無く、常に NULL だった。`GET /projects` には件数・最終活動時刻・人待ちの時刻も無かった。

## 決定

- D1. `projects.secretary_summary` を消す（migration 0095 で `DROP COLUMN`、`Project.secretary_summary` も消す）。
  非 NULL・非空の値が残っていた行は、消す前に D2 の表へ `state` として写す（`objective` は依頼文、`updated_by` は
  `migration:0095`）。データは失わない。
- D2. 案件の状態の要約は新しい表 `project_briefs`（案件ごとに 1 行、`project_id` が主キー）に持つ。型は
  `task_core::ProjectBrief`。
  - `objective`（目的、必須）・`state`（現状、必須）・`open_issues`（未解決）・`next_actions`（次の手）・
    `waiting_on`（待っている相手。`kind` は `human` / `task` / `external`、`target` は任意、`note` は必須。
    `kind = task` の `target` はこの案件の task id でなければならない）・`evidence_refs`（根拠への参照。
    `task:<id>`・`run:<id>`・文書の path・URL など自由な文字列）・`updated_at`・`updated_by`。
  - 書き込みは全置換（PUT）。差分更新はしない（要約は毎回書き直すもので、部分更新は古い欄を残す）。
  - 上限: `objective` 2,000 字、`state` 4,000 字、各一覧 20 件・各 500 字。空・上限超えは 422。
- D3. 書き込みの経路は `PUT /api/v1/projects/{id}/status`（action `project.status_put`）。
  - CoS（`/cos/operations`）からは **その案件の案件 CoS スレッドの run だけ**が書ける（統括 CoS・他案件の CoS・
    旧スレッドは 403 `project_brief_forbidden`、`rejected` で記録）。`updated_by` は `cos:<thread id>`。
    統括 CoS は要約を読むだけにし、書く責任を案件 CoS に寄せる（同じ案件を 2 者が書き直して食い違わないように）。
  - 人は管理系（admin token）の直接の `PUT` で書ける。`updated_by` は `human`。
  - 案件が無ければ 404。
- D4. `GET /projects` の各項目に、案件（従来の欄はそのまま平らに）に加えて次を足す:
  `brief`（D2。無ければ省略）、`task_counts`（task の状態ごとの件数）、`last_activity_at`（案件の task の
  `updated_at` の最大と案件の `updated_at` の遅い方）、`human_waits`（その案件の受信箱の項目数）と
  `oldest_human_wait_at`（その最古の `created_at`。無ければ省略）。人待ちは `GET /inbox/items` と同じ
  `human_inbox` から数える（別の定義を作らない）。`GET /projects/{id}` にも `brief` を足す。
- D5. CoS chat run の文脈（`CosChatContext.projects`）に案件の要約を渡す。
  - 統括 CoS: アーカイブしていない全案件。1 件ごとに id・題名・状態・`objective`・`state`（400 字で切る）・
    待ち（`waiting_on` の `kind` と `note`）・未終端 task の件数・最終活動時刻・要約の更新時刻だけ。詳細は
    `GET /api/v1/projects/{id}` で必要な案件だけ読む。
  - 案件 CoS: 自分の案件の 1 件だけ（同じ形）。状態が変わったら `project.status_put` で書き直す。
  - 旧スレッドには渡さない。
  - 件数・時刻は store から決定的に求める（dispatcher に LLM 呼び出しは入れない）。人待ちの時刻は受信箱の組み立てを
    dispatcher に持ち込まないため渡さない（必要なら `GET /projects` で読む）。

## 結果

- 旧い仕組み（`secretary_summary` の列・欄・GUI の参照・celerisctl の表示）は同じ変更で消した。
- 本番への移行: migration 0095 が起動時に当たる（手作業なし）。
