-- ADR 2026-10-14-eval-verify-isolation D3.1: 評価試行を起動した daemon instance（`daemon_instances.instance_id`）。
-- claim（queued → running）と同じ更新で書く。終端・queued に戻しても消さない（記録）。回復は running の行の
-- この持ち主が生きているかで error にするかを決める。migration 前の行は NULL。
ALTER TABLE eval_trials ADD COLUMN instance_id TEXT;
