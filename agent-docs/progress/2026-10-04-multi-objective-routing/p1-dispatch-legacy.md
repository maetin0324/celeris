# Phase 1 dispatcher legacy policy 接続

`task-dispatch` は `StaticPolicy` の設定行を lane と adapter 指定で絞り、設定順の model/deployment 候補として `task_core::model_router::optimizer::legacy_rank` に渡す。model identity は `tier_models` の binding、実効 adapter の lane model、従来の model の順で導く。kernel の allowlist は既存 `select_provider_for` に渡し、候補順位の出自は `RoutingDecided.record.optimizer` に任意 trace として残す。

provider の空き・cooldown、cheap local-first、プールの account score、CoS、sticky は既存 selector と帳簿が決定する。`LaneResolution.selection.reason` の語彙と優先関係は変えない。列挙できない外部 `ProviderPolicy` 実装は従来の選択を維持し、trace を省略する。

`routing_dispatch_legacy_equivalence_matrix` は kernel 接続経路と旧 trait surface の対照経路を lane、worker/reviewer/CoS、local 優先、sticky、供給の空き・満杯・不通で比較する。provider・account・model と selection reason を比較する。別試験で `tier_models` の model identity が trace に入り、選択を変えないことも確認する。

検証: `cargo test -p task-dispatch` は unit 675 件と integration 4 件が成功。`cargo clippy -p task-dispatch --all-targets -- -D warnings` も成功。
