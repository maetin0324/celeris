# routing の未使用定義を撤去し、特徴量 hash の計算を集約する

- 日付: 2026-11-15
- 状態: 採用（R6 routing の指定範囲）
- tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]

## 決定

`route_record` を `{schema, template, brief, aux}` の直列化と `sha256:` + 小文字 hex の owner とし、通常の特徴量生成と dispatcher の評価記録が同じ関数を呼ぶ。欄順、null、空配列、日本語の UTF-8 を含めて既存の byte 列を維持する。schema と template の版は変えない。

参照の無い `route_record::{RouteDecision, DecisionSubject}`、`model_router::cost::ScoreBreakdown`、`LlmSourceRef::to_key`、`task_ops::routing_audit::task_routing_audit` を撤去する。現行の `route_select::trace::RouteDecision` と直行経路の同名型、API、履歴の投影は維持する。

効かない `[execution].cheap_local_first` を撤去する。既存の `deny_unknown_fields` により指定された設定は読み込みエラーとする。互換分岐を置かず、本番設定の確認・削除は人の手順として進捗に書く。

## 検証

共通化前の両実装に固定入力の試験を置き、同じ期待値を確認してから共通関数への呼び出しに置き換える。従来の同一性試験だけでは、両経路の hash が同時に変わる退行を検出できないため、固定の期待値を追加する。設定には true / false の両方の拒否試験を置く。既存の設定試験だけでは、この無効な欄の再導入を検出できない。

検索と各項目の判定は `agent-docs/progress/2026-11-15-health-v2-r6/routing.md` に記録する。DB、events、研究用の crate・凍結 fixture は変更しない。
