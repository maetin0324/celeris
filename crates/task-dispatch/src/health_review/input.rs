//! ADR 2026-11-07-health-review-integration D4: final review の `ReviewRequest.health`（base→candidate の diff 統計・
//! 変更 path の owner 地図の行・repo health の new findings の要約）を candidate SHA ごとに 1 回作る処理の置き場
//! （後続の health-input の unit）。WU の check・WU の review・tick では作らない（D4.4）。
