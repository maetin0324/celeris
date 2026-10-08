-- ADR 2026-10-05-cos-chat-home 付記（rollover の会計）: context 占有と累積の課金相当入力を approx_tokens と分ける。
-- last_context_tokens: 最後に観測した 1 API 呼び出しの input+cache_read+cache_creation（NULL = 未観測。既存行・
--   占有を報告しない harness は approx_tokens へ fallback）。
-- billed_input_tokens: run ごとの input+cache_read+cache_creation の累積（課金相当の入力。判定には使わない）。
ALTER TABLE node_sessions ADD COLUMN last_context_tokens INTEGER;
ALTER TABLE node_sessions ADD COLUMN billed_input_tokens INTEGER NOT NULL DEFAULT 0;
