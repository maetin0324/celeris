# labels-db: DB outcome labels

D3(b) の `quota_used`・`retries_after`・`repaired` を `outcome_labels_for_run` に追加。quota は run_id 一致の最後の `quota_estimated` event の値付き window のみを出力。retries_after は同一 task/WU の対象 run より後の worker run 数、repaired は後続 repair WU の有無。対象 run の started_at が無ければ後二欄を null＋missing。

試験: `route_outcome_retries_after_and_repaired_task_and_wu`、`route_outcome_quota_used_from_quota_estimated`。変更は ADR 実装付記、task-features の label 表、field-audit に記録。
