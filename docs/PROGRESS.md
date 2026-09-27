# PROGRESS — taskd

現在地: **Phase 119、Phase E6、Phase F4b まで記録済み。F5-1 dogfood の3成果・再レビュー対応を実装、全体ゲート成功**。

詳細な履歴と証跡は下記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## 目次

- [Phase 1–50（Phase 0 の初期記録を含む）](progress/phase-001-050.md)
- [Phase 51–100](progress/phase-051-100.md)
- [Phase 101–150](progress/phase-101-150.md)
- [Phase E](progress/phase-E.md)
- [Phase F](progress/phase-F.md)

各 Phase の詳細・証跡・申し送りは上記の分割ファイルを参照。既存の `docs/PROGRESS.md` 参照はこの目次を入口として維持する。

## Phase F5-1 dogfood（再レビュー対応）

worker・review の完了を JoinHandle で明示同期し、実時間の待機回数に依存しない検証へ変更。
[実装と検証の記録](progress/phase-F.md#f5-1-review-repair)を参照。

## release c51837427ac5 の昇格（F4b + planner 修正、schema 28、2026-09-27 15:31Z）と F5-1（やり直し）の結果

- F5-1（やり直し）01M3HG7VV6A2HRHTNXWPDS9051: done（atomic に倒れたまま、13:18〜15:31Z）。planner 2 run（Opus 1 分、Sonnet 7 分）は Plan Mode で成果物を書けず失敗。worker 4 run のうち 3 run が `cheap/mechanical-verifiable-reversible` で gpt-6-luna（F1 の規則表が atomic でも Task の features から cheap を選んだ）、retry のエスカレーションで 1 run が frontier（gpt-6-astra）。continuation 1。E6（全 run standard）と比べて lane の分布は変わった。
- 昇格: in-flight 0 で停止→起動、本番 `c51837427ac5`（schema 28）。本番に F4b（reached / Go、案件 replan、children、案件ページ DAG）と planner の permission_mode 修正が入った。
- 3 回目の F5-1 を投入（planner が成果物を書ける版で compound の計画が採用されるかを確認）。
