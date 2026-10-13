-- ADR 2026-10-11-llm-source-model-routing-evaluation D4: 旧い実行 shadow の UTC 日次上限の共有予約
-- （0049 の routing_shadow_reservations）を消す。shadow の仕組みごと撤去し、予約を書く経路も読む
-- API も無い。過去の routing_shadow_recorded event（events 表）は残し、書き換えない。
DROP INDEX IF EXISTS idx_routing_shadow_reservations_day;
DROP TABLE IF EXISTS routing_shadow_reservations;
