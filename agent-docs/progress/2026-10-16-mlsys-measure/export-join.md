---
unit: export-join
task: 01M51X7MZPYQXVWV2WDG2N70C1
status: done
completed: 2026-10-16
---
# export-join — export の結合と接続確認

## 実装済み
- `decision_log_export.rs`: 新 record `trial`（run 1 行に全欄を結合）。campaign_id・case_id・arm・repetition・manifest_hash・routing_setting、推奨候補（proposed_first / proposed_candidates）、実行候補、fallback 理由、分類結果（kind/model/snapshot/fallback）、prompt・質問 schema・provider model の版、run_end・wu_verdict・`grader_verdict`（campaign_verdict）、消費（`run_consumption` と、run に帰属しない直接行 = Jev 分類を task の最初の run にだけ載せる `task_level_consumption`）。
- decision / run / outcome / usage record にも識別欄（case・arm・repetition・manifest_hash・routing_setting）を付与。usage は run → decision → task の順に提案を引く。
- summary: `total` と by_role/by_retry/by_arm は直接帰属のみ。共有オーバーヘッドは `shared_overhead` に別欄。`by_attribution_rule`、`elapsed_ms`（store が metering から埋める）、`run_wall_sum_ms`（別欄）。定価換算・実支出・クラスタ・壁時計は従来どおり別欄。
- 接続確認: `celerisctl campaign export <id> --out <dir> --verify-connected`（`task_core::decision_log_export::verify_connected`）。空 export、試行の必須欄（case_id/arm/repetition/manifest_hash/proposed_first/exec_model/run_end/grader_verdict）の欠測、grader `unscored`、run の消費が観測 0、直接消費行ゼロのいずれでも stderr に理由を出し非 0。出力ファイルは書く。

## mock 検証済み（temp DB、実 API・実クラスタなし）
- `crates/celerisctl/tests/campaign_connected.rs`: 正常（全結合欄・共有オーバーヘッド別欄・Jev 費用の帰属）／空（別 campaign 含む）／欠測（case_id・grader・消費未計上）の 3 試験。

## 実経路検証済み
- なし。**実経路検証は未実施**。

## 未実施・人が実行する手順
実 campaign（実 API・実クラスタを使う本測定）の後に、本番 DB を読むだけで確認する:
1. `celerisctl --db <本番 DB の写し> campaign export <campaign-id> --out $TMPDIR/exp --verify-connected; echo $?`（本番 DB に直接書かない。写しは `celerisctl db backup` 相当で作る）
2. exit 0 を確認。非 0 なら stderr の `not connected:` の行（run ごとの欠測欄）を見る。
3. `summary.json` の `shared_overhead`（CoS）、`elapsed_ms` と `run_wall_sum_ms`、`total.cash_cost_usd`（実支出は取れなければ null）を目で確認。
- Jev の分類費用は `task_level_consumption` に出る。Jev token 数・CoS 実 turn の配線は metering.md の未実施のまま。
- `decision_log_interventions` の書き手は未着手のまま（持ち越し）。

## 提案
- 本測定 runner は campaign 終了時に `--verify-connected` を必ず走らせ、非 0 なら結果を採用しない。
