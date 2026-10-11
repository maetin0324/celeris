# dispatch-ctx — 今回の routing 判断を特徴量へ渡す

task-features の retry で、dispatch の `record_route_features` に今回の `RouteFeatureContext` を渡した。lane は quota 調整後の `task.worker_hint.tier`、harness は選択済み `adapter_id`、`task_features_v69` はこの dispatch の `LaneDecision.features` を記録する。抽出に失敗した場合は従来どおり warning のみで dispatch を続ける。

`route_dispatch_records_feature_reference_and_snapshot` は初回 dispatch の保存行を確認し、lane=`cheap`、harness=`acp`、9 軸 features の存在、欠測欄にこれらがないこと、brief の性質節が文として含まれることを固定する。

検証: `cargo test -p task-dispatch route_dispatch` は 1 test passed。計画 check の前回失敗原因は、この unit 固有の進捗ファイルが未作成で先頭の `test -s ... &&` が短絡していたこと。ファイル作成後に同じ check を再実行する。
