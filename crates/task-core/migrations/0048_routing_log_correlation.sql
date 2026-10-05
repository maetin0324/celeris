-- ADR 2026-10-04-multi-objective-model-routing Phase 2（p2-log-index）: routing の相関欄。
--
-- `llm_proxy_requests`（0022）に、要求・決定・snapshot・run・task・供給元を結ぶ欄を足す（すべて NULL 可）。
-- 既存の行と既存の欄は書き換えない。この版より前に書かれた行は新欄が NULL のまま残る。
--
--   request_id  は既存の `llm_proxy_requests.id`（0022）をそのまま使う（別欄は足さない）。
--   account     は既存の欄（0022）をそのまま使う。
--
--   decision_id  — 経路選択の決定 id（routing decision）。
--   snapshot_id  — 決定に使った SourceState snapshot の id。
--   run_id       — この要求を出した run（`runs` の id）。
--   task_id      — この要求が属する task。task に紐付かない要求は NULL。
--   source_id    — 選ばれた供給元の id（`source` と同じ粒度の安定 id）。
--   model        — 決定で選ばれた具体モデル名（`upstream_model` は上流へ実際に渡した名前）。
ALTER TABLE llm_proxy_requests ADD COLUMN decision_id TEXT;
ALTER TABLE llm_proxy_requests ADD COLUMN snapshot_id TEXT;
ALTER TABLE llm_proxy_requests ADD COLUMN run_id TEXT;
ALTER TABLE llm_proxy_requests ADD COLUMN task_id TEXT;
ALTER TABLE llm_proxy_requests ADD COLUMN source_id TEXT;
ALTER TABLE llm_proxy_requests ADD COLUMN model TEXT;

-- decision から要求を引く（値のある行だけ）。
CREATE INDEX idx_llm_proxy_requests_decision ON llm_proxy_requests (decision_id)
  WHERE decision_id IS NOT NULL;

-- task events から routing の決定（`routing_decided`）を task 単位で引く。
-- store/events.rs の述語と同じ式にする（0047 と同じ書き方）。
CREATE INDEX idx_events_routing_decided ON events(task_id, seq)
  WHERE json_extract(json,'$.type') = 'routing_decided';
