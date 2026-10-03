# 定期実行（cron job）の API（/api/v1/cron-jobs）

ADR-0131 D5。schema は `docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1 cargo test -p task-api` で再生成）。共通の約束は `docs/celeris-api-v1.md`。

job は DB（`cron_jobs`・`cron_job_runs`）に置く。発火は daemon の tick が時刻を見て雛形から通常の task を作るだけ（LLM 呼び出しなし）。API の手動実行も同じ関数（`task_ops::cron_jobs::run_now`）を通る。`{id}` は job の ULID か `name` のどちらでも引け、無ければ 404 `cron_job_not_found`。書き込み系は `POST /tasks` と同じ通常の認証（人の操作）。式・タイムゾーン・雛形の誤りは 422 `validation`、未知の欄・不正な JSON は 400。

- `GET /cron-jobs` → 200 `CronJobList`（`items[]` は `CronJobView` = `CronJob` の欄〈`id`・`name`・`enabled`・`schedule`・`timezone`・`overlap`・`catch_up`・`template`・`next_fire_at`・`created_at`・`updated_at`〉+ 最後の履歴 `last_run`。`name` 昇順）。
- `POST /cron-jobs` → 201 `CronJobView`。本文は `CronJobCreateBody`（`name`、`schedule`〈5 欄の cron 式か `@daily` 等〉、`timezone`〈IANA 名〉、`overlap?`〈`skip` 既定 / `queue`〉、`catch_up?`〈`latest` 既定 / `skip`〉、`enabled?`〈既定 `true`〉、`template`〈`CronTaskTemplate`: `title`〈`{date}` は発火日〉・`objective`・`acceptance`〈`POST /tasks` と同じ形〉・`assignee?`・`harness?`・`lane?`・`project?`・`repos?`・`priority?`〉）。`name` の重複は 409 `cron_job_name_in_use`。
- `GET /cron-jobs/{id}` → 200 `CronJobView`（雛形を含む）。
- `PATCH /cron-jobs/{id}` → 200 `CronJobView`。本文は `CronJobPatchBody`（書いた欄だけ。`template` は丸ごと置き換え。`enabled` は pause / resume で変える）。schedule / timezone を変えたら `next_fire_at` を今から計算し直す。
- `DELETE /cron-jobs/{id}` → 204（履歴も消える。作った task は残る）。
- `POST /cron-jobs/{id}/pause` → 200 `CronJobView`（`enabled = false`、`next_fire_at = null`、溜まっている `queued` は `skipped_overlap` に閉じる）。`POST /cron-jobs/{id}/resume` → 200（一時停止中に過ぎた時刻は取りこぼしにせず、次回を今から計算）。
- `POST /cron-jobs/{id}/run` → 200 `CronRunResult`（`job_id`・`job_name`・`runs[]`〈この実行の履歴、`trigger = manual`〉・`task_id?`）。前回の task が終わっていなければ D2 の規則（`skip` → `skipped_overlap`、`queue` → `queued`）。同じ時刻の記録の衝突は 409 `cron_run_conflict`。
- `GET /cron-jobs/{id}/runs?limit=` → 200 `CronJobRunList`（`job_id`・`items[]` = `CronJobRun`〈`scheduled_for`・`trigger`・`outcome`〈`created` / `queued` / `skipped_overlap` / `skipped_missed` / `error`〉・`task_id?`・`detail?`・`recorded_at`〉。新しい順、`limit` 既定 50・上限 500、0 は 400）。
