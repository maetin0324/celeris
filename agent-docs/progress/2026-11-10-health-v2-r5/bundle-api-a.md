---
title: R5 bundle-api-a — task-api の CoS・案件試験の束ね
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
status: blocked
updated: 2026-11-10
---
# R5 bundle-api-a — task-api の CoS・案件試験の束ね

## 実装前 brief
1. 根本原因: 同じ fixture の試験が別 target となり link 回数を増やしている。
2. 責務 owner: crates/task-api/tests の target 構成。architecture-map の task-api HTTP API・CoS チャットの行に対応する。
3. 既存の再利用先: common/mod.rs と inventory.json の担当21 target・4束を使う。
4. 選択肢: 採用済み ADR の capability group・fixture による4束（6・6・6・3）を採る。全件を1 binary にまとめない。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: 担当 target の旧 path と file ごとの mod common 宣言。試験本体・common の本体は変えない。
7. 不変条件: 試験関数・case・不変条件の試験を維持する。keep/r6 と数字付き file は元の path に残す。cases.py、classify.py、nextest list、各束の nextest と in-process 実行で確かめる。

## 変更
- `phase 1: bundle task-api CoS and project integration targets` — 担当21 target を既存 ADR の4束へ移す。

## 証拠（提出時の HEAD で実行）
基準は `$CELERIS_WU_BASE`。実装と局所検証まで実施。完了日は未確定（plan_issue）。

- 条件0: `python3 agent-docs/progress/2026-11-10-health-v2-r5/classify.py --verify-leaf bundle-api-a agent-docs/progress/2026-11-10-health-v2-r5/inventory.json` → exit 0。担当21 target 移動、keep/r6 243 target を元の path で確認。
- 条件1: `python3 agent-docs/progress/2026-11-10-health-v2-r5/cases.py --crate task-api --base "$CELERIS_WU_BASE"` → exit 0。試験関数855→855、表の行757→757、移動118試験、不変条件277試験（42 file）、removals 0。
- `cargo nextest list -p task-api --message-format json` → 移動前後とも exit 0、試験878→878、binary 114→97。base は移動前の同じ worktree で取得。
- `cargo nextest run -p task-api -E 'binary_id(task-api::cos_delivery_suite_a) | binary_id(task-api::cos_delivery_suite_b) | binary_id(task-api::cos_delivery_suite_c) | binary_id(task-api::routing_budget_suite)'` → exit 0、118 passed、0 skipped。
- `cargo test -p task-api --test cos_delivery_suite_a --test cos_delivery_suite_b --test cos_delivery_suite_c --test routing_budget_suite` → exit 0、各束30・31・33・24 passed、全118試験成功。束から外す必要のある汚染はこの実行では検出されなかった。
- `python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json`（差分と未追跡 path を標準入力）→ exit 0。`sh "$CELERIS_WU_SCOPE_PATHS"` でも担当 file と進捗のみ。
- `cargo clippy --workspace -- -D warnings` → exit 0（1分39秒）。
- `cargo fmt --all -- --check` → exit 0（出力なし）。
- 移動した21 file と変更前の内容を Python で比較 → common の import 置換だけで一致、試験本体に差分なし。
- 条件2: `bash scripts/dev/test-affected.sh` → exit 1。冒頭の health gate が下記3件で失敗し、affected の Rust 試験・web 型検査/unit は未実行。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 1、arch-map.wrong_path が新規3件。下記の計画不整合。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check` → 各 exit 0。

## 未解決
plan_issue: inventory が担当にした `cos_operations`・`chat_stream`・`cos_chat_attach_pin` は、実際には docs/architecture-map.md の137・156行の brace 付き path に載っている。移動後に arch-map.wrong_path の新規3件で health が失敗する。地図は R5 write set 外なので変更していない。検査器・inventory・baseline・対象規則も変更していない。

既存 ADR D4 の r6 条件に従うなら3 target は r6 に再分類し、bundle-api-a の受け入れ条件と inventory の束を改訂する必要がある。別案は地図の更新を認可する別の作業範囲を用意すること。どちらも現 leaf の write set を越えるため、この run では実施しない。変更は次の run で再利用できる形で commit する。

## 提案
classify.py の architecture-map 参照抽出と、実際の arch-map.wrong_path の brace 展開の差を、検査器を担当する別の task で調べる。R5 の検査器を弱めて通さない。
