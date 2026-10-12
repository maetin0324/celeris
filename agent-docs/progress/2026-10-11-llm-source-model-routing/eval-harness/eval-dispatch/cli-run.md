---
title: eval-dispatch cli-run（評価試行投入 CLI）
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
status: done
updated: 2026-10-12
---

# eval-dispatch cli-run

`celeris-routing-eval run` を追加し、凍結 manifest の問題と `candidates.rs` の候補・k に従って評価試行を API へ投入する。12 問候補は `subset_12` の問題だけを選ぶ。check command は API の採点用フィールドだけに含め、worker objective と標準出力の要約には含めない。

- options: `--api`・`--manifest`・`--candidates` 必須。`--token-file`・`--repo`・`--wait`・`--poll-interval-ms`・`--out` 任意。
- `--wait` は batch の GET を間隔 poll し、全 trial が passed/failed/error/cancelled になるまで待つ。summary JSON は id・problem・trial 番号・status のみ。
- reqwest の blocking/json/rustls 機能を routing-eval crate のみに追加。CLI help に run options を記載。

検査:
- 前 run の check は `cli-run.md` 不在で失敗していた（`test -s .../cli-run.md`: exit 1）。この進捗記録を追加し、同じ check を再実行する。
- `cargo test -p routing-eval run::tests`: exit 0（候補展開・subset/k・localhost POST/GET --wait・summary の check command 非表示を確認）。
- `cargo fmt --check -p routing-eval`: exit 0。
- 指定された progress check: exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"`: exit 0（変更範囲は routing-eval CLI と本 progress）。
