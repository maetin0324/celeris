-- Task-scoped browser restrictions. A missing row denies browser execution.
CREATE TABLE browser_task_policies (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    policy_json TEXT NOT NULL
);
