-- ADR 2026-10-05-cos-chat-home 付記（resume 時の差分配送）: この session に配送済みの thread seq の水位。
-- delivered_through_seq: run が done で終わったときに、その run が session に渡した入力（と同じ run の返事）までの seq
--   へ上げる（下げない）。0 = 不明（既存行・まだ done の run が無い）で、resume でも summary＋未要約履歴の全文を渡す。
--   summary_through_seq（checkpoint の水位）とは独立。
ALTER TABLE node_sessions ADD COLUMN delivered_through_seq INTEGER NOT NULL DEFAULT 0;
