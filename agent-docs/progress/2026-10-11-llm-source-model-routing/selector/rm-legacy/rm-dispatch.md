---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
---
# rm-dispatch: dispatcher 旧経路の撤去

dispatcher の shadow・enforce 補助 module と候補 trace module を削除し、関連する公開 exports、保持 field、daemon の profile injection を除去した。旧候補 trace は実行時の選択に使われず、selector の route 選択は既存 `RoutePolicy` 経路に残る。

`DispatchRoutingSettings` の context safety margin と escalation は現行の context 作成・再試行判断が参照し、設定側の `rm-config` がまだ存在するため、dispatcher facade に保持している。`RoutingRecord` の互換監査欄は後続 `rm-router` が型を撤去するまで残るが、dispatcher は空の既定値を渡す。選択済み source/model は `WorkerStarted` に記録され、候補の選択は行わない。
