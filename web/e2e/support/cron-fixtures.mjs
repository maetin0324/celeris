// 定期実行の画面監査用。実際の task 作成・スケジューラは起動しない。
export const cronJobFixture = {
  id: "daily",
  name: "知識ベースの日次整理",
  enabled: true,
  schedule: "0 9 * * *",
  timezone: "UTC",
  catch_up: "skip",
  overlap: "skip",
  created_at: "2026-10-01T00:00:00Z",
  updated_at: "2026-10-11T00:00:00Z",
  next_fire_at: "2026-10-12T09:00:00Z",
  template: { title: "知識ベースの整理", objective: "候補を整理する", genre: "knowledge-curation" },
  last_run: {
    id: "CR1",
    job_id: "daily",
    trigger: "schedule",
    outcome: "created",
    task_id: "T1",
    scheduled_for: "2026-10-11T09:00:00Z",
    recorded_at: "2026-10-11T09:00:01Z",
  },
};
export const cronFixtures = {
  "/api/v1/cron-jobs": { items: [cronJobFixture] },
  "/api/v1/cron-jobs/daily": cronJobFixture,
  "/api/v1/cron-jobs/daily/runs": { job_id: "daily", items: [cronJobFixture.last_run] },
};
