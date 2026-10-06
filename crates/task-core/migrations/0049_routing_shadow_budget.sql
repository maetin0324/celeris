-- ADR 2026-10-04-multi-objective-model-routing §7.1・Phase 4（p4-shadow-eval）: 実行 shadow の日次上限の共有予約。
--
-- 実行 shadow（候補モデルで実際に生成する。既定 off）は、開始前に output 上限込みの最悪消費を
-- この表へ予約する。UTC 日（`day` = YYYY-MM-DD）ごとの消費は、その日の行の `charged_*` の和と行数
-- （= request 数）で決まる。予約・確定は BEGIN IMMEDIATE の中で和を読み直してから書くので、
-- 再起動・daemon handoff・2 instance が同じ DB を使っても上限を越えない。
--
--   state            — reserved（実行中または確定されずに取り残された）/ completed / failed / timed_out
--   reserved_*       — 予約時の最悪消費（tokens と effective 費用の micro USD）
--   charged_*        — 日次上限に数える消費。reserved の間は予約と同じ。completed は実測、
--                      failed/timed_out は実測と予約の大きい方
--   owner            — 予約した instance（監査用）
--
-- 新しい表だけを足す（既存の表・event は変えない）。本文・prompt・credential は持たない。
CREATE TABLE routing_shadow_reservations (
  id                 TEXT PRIMARY KEY,
  day                TEXT NOT NULL,
  shadow_id          TEXT NOT NULL,
  owner              TEXT NOT NULL,
  state              TEXT NOT NULL
                     CHECK (state IN ('reserved', 'completed', 'failed', 'timed_out')),
  reserved_tokens    INTEGER NOT NULL CHECK (reserved_tokens >= 0),
  reserved_micro_usd INTEGER NOT NULL CHECK (reserved_micro_usd >= 0),
  charged_tokens     INTEGER NOT NULL CHECK (charged_tokens >= 0),
  charged_micro_usd  INTEGER NOT NULL CHECK (charged_micro_usd >= 0),
  reserved_at        TEXT NOT NULL,
  settled_at         TEXT
);

CREATE INDEX idx_routing_shadow_reservations_day ON routing_shadow_reservations (day);
