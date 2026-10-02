-- ADR-0117 付記: 受信箱（build_attention）の latest_delivery_skipped_rows が events 全件を
-- full scan していた。delivery_skipped だけを引く部分 index を作る。WHERE 式は
-- store/events.rs の latest_delivery_skipped_rows_impl と字句まで同じにすること（でなければ
-- SQLite がこの index を使わない）。
CREATE INDEX idx_events_delivery_skipped ON events(task_id, seq)
  WHERE json_extract(json,'$.type')='delivery_skipped';
