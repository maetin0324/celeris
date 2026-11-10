---
title: 健全性v2 R5 celeris の結合試験の束ね
status: blocked
tasks: [01M6TYVJT53598CPKMGSQGA7XM]
updated: 2026-11-10
---
# 健全性v2 R5 celeris の結合試験の束ね

## 実装前 brief
1. 根本原因: 同じ capability・fixture の試験が別の Cargo target になり、build/link が増えている。
2. 責務 owner: `crates/celeris/tests/` の test target 構成。通知は `celeris::notify`、daemon は `celeris::daemon`、script の fixture は `tests/support/script_compare.rs` が持つ（`docs/architecture-map.md` の該当行）。本番コードの責務は変えない。
3. 再利用先: 既存の `tests/support/mod.rs` と repo root の `tests/support/http_*.rs`。試験本文をそのまま module として読む。
4. 選択肢: 独立 target の維持と、採用済み ADR の同じ capability・fixture ごとの束ねを比較し、後者を採用した。全部を1束にする案は D1・D3 に反する。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: inventory の担当16 target の元の path と、束の root に寄せた支援 module 宣言。
7. 不変条件: 試験関数・表の case・不変条件の試験を維持する（cases.py）。process 汚染を持つ target は移動しない（classify.py）。束の試験が1 process で共存することは各束の cargo test で検査する。

## 変更
- `phase 1: bundle celeris integration test targets` — inventory の16 targetのうち15 targetを4束へ移動。試験本文は変更せず、支援 module 宣言を root に寄せた。
- `browser_eval_surface_suite`: browser_startup_reap・fake_http・workspace_roots。
- `cos_delivery_suite`: cos_chat_stagnation_notify・cos_chat_triage_notify・cos_chat_triage_scenarios・deployment_audit・releases_api・stranded_default_branch・stranded_integration。
- `routing_budget_suite`: script_compare_execution_gate・script_compare_report・tick_scale_baseline。
- `unmapped_suite`: legacy_event_startup・variant_propagation。旧 event shape の replay 契約は維持した。

## 元の path に残す理由
採用済み ADR D3・D4 と inventory に従い、他の target は変更していない。
- `script_runtime` は current_exe による自己再起動と可変 static、`long_run_soak` は自己再起動・signal と名前指定 suite のため keep。
- `instance_handoff` は固定 port/path を持つため keep。CARGO_BIN_EXE_celeris で子 daemon を起動すること自体は D3 の汚染ではないが、この target は他の keep 条件に当たる。
- `daemon_ready_wait` は同じ fixture の相手が無い singleton のため keep。本文は実 daemon を起動せず、JoinHandle と loopback listener で待機関数を検査する。`daemon_port` は target ではなく共有支援 module なのでそのまま残す。
- `bin_smoke` は architecture-map の参照対象なので r6。地図が write set 外のため、今は移動しない。
- `script_compare_cos_digest`・`script_compare_lane_rules`・`script_compare_route_priority`・`script_generation_daemon` は architecture-map 参照対象なので r6。
- `credential_wait_notify` は固定 port、`campaign_lock_e2e` は可変 static と数字、`campaign_wiring_e2e`・`script_generation_e2e` は数字を含む file 名のため keep。actor_trace_flow1〜5 と ops_r1_* も数字を含み write set 外なので維持した。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
検証中。

## 未解決
`unpromoted_release.rs` は inventory で bundle-celeris に分類されているが、architecture-map:150 の brace path が参照している。移動すると health の `arch-map.wrong_path` が exit 1 になることを確認した。地図は R5 の write set 外なので変更しない。この1件の移動は戻し、元の path に維持した。inventory の分類を r6 に修正して ADR の表と担当条件を合わせる再計画が必要。classify.py の検査器自体には触らない。

## 提案
inventory の brace path の参照漏れは inventory を持つ担当が修正し、unpromoted_release を r6 に分類する。重複 setup の整理と性能の前後比較は後続の担当 leaf が行う。
