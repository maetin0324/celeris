---
title: "decision-log: 判断・実行・成果・消費の最小一貫ログと campaign export（close-out）"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# decision-log（task 全体の close-out）

ADR: `agent-docs/adr/2026-10-15-decision-log-export.md`（付記に実装との突き合わせ）。unit ごとの詳細は同ディレクトリの `log-core.md`・`log-wire.md`・`export-agg.md`・`export-cli.md`。

## 実装済み
- 記録（log-core）: migration 0082、run_id 主キーの冪等 upsert。提案/実行は別欄、run 終了・WU 検証・campaign 合否は別ラベル。
- 配線（log-wire）: dispatcher が events から投影。fallback 時も結果を最初の推奨候補へ帰属しない。retry・再処理・cancel・timeout で二重計上しない。
- 集計・export（export-agg / export-cli）: campaign 単位の token/cache/cost/wall-time、役割別、介入（研究判断と手動引継ぎ・復旧を区別）、クラスタ待機。定価換算（estimated）・実支出・quota を別欄にし、欠測は null と `n_missing`。
- Jev の `classifier_confidence` は `reward`/`propensity` に入れない（別欄）。

## 証拠
- `bash scripts/dev/test-parallel.sh`: exit 0。passed 5168 / failed 0 / ignored 13、`test-parallel: ok`。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- 誤帰属・二重計上・別ラベル・confidence 非同一視: `cargo test -p task-dispatch --lib decision_log`（5）、`task-core` の `decision_log_tests` / `decision_log_export_tests`、`cargo test -p celerisctl --test campaign_export`（byte 一致・秘密なし・欠測 null）。いずれも上の全体試験に含まれ通過。
- 失敗なし。最小修正は不要だった。

## 未実装・未検証
- **実 LLM 測定は未実施**（試験は決定的 fixture のみ。有料測定は始めていない）。
- **本番 host は未変更**（systemd・~/.config・releases・本番 DB・daemon に触れていない。config 変更なし）。「本番への移行」は無い（migration 0082 は次の release 時に自動適用される）。
- campaign_id / arm / routing_mode / Jev classifier 欄の書き手が未接続のため、実運用では campaign_id None の run は export に出ない。CoS・cluster 待機・介入の消費行の自動書き込みも後続。`export_cursor` は None。

## 提案
- campaign 実装時に `campaign_id` の書き手を一本化し、root task id を使うなら log-wire で入れる。
- reviewer run の判断 record と CoS/planner の消費行を追加して、役割別集計を実運用で埋める。
- 実 LLM での小規模測定は人の承認後に別 task で行う。

## live-smoke の検証状況（2026-10-15 22:15Z）
検証済み: 鍵登録（fingerprint）、本番 config（decision_assisted + jev enabled）、jev-wire 込み release 稼働。
未検証: 実 Jev API での分類成功、fell_back_to_fixed の有無と理由、credential 待ち→再開の実経路。観測できた `routing_decided.lane_assist` は `not_attempted`（`real_api_verified: false`）のみ。詳細は live-smoke.md。
