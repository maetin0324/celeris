---
title: "A 用 HarnessExecutor（既存の強いハーネス運用を後付け制限なしで包む）"
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# adapter-a（WU adapter-a）

## 実装済み
- `crates/campaign/src/harness_exec.rs`: `HarnessExecutor`（state_dir + 注入可能な `CommandBuilder`）。manifest の harness を `sh` wrapper 経由の subprocess（新 process group）として trial workspace で実行。既定 builder は `claude -p <prompt> --model <model> --output-format json`。
- 制限なし: Celeris の routing・許可・turn 制限を通さず、追加もしない（module 冒頭コメントが ADR 追補用の注記）。適用するのは manifest の `trial_timeout_secs`（超過で group kill → `Timeout`）だけ。token/wall の budget は runner が判定。
- 台帳連携: `submit` が spawn 前に `starting` 目印を fsync、spawn 後に `meta.json`（pid・pgid・/proc start ticks・開始時刻・timeout）を fsync。ledger の `run_id` に `pid:N/pgid:N` を記録。`lookup` は meta から既存 process を返す（再起動後も同じ process を追跡、再起動しない）。meta 無しで目印だけ残った場合（spawn 中 crash）は二重起動を避けて Err で止める（人が確認）。
- `poll`: exit.code（wrapper が原子的に書く）→ Passed（0）/ Failed。process が exit 無しで消えたら Error。timeout 超過で group kill → Timeout。確定結果は `final.json` で冪等。`cancel`: group kill、Cancelled を確定（二重 cancel 可）。
- token/cost: stdout 最終 JSON の `usage.input_tokens + output_tokens` と `total_cost_usd`。取れなければ `None`（推定しない）。
- Passed/Failed は harness の終了コードの暫定値。正本は独立採点（別 unit）。

## mock（stub）検証済み（`tests/harness_exec.rs`、8 件）
完走＋usage 取得／usage 無しは None／非 0 は Failed／Wait 中 workspace 保持／再起動で重複起動なし（started 1 回）／spawn 中 crash 目印で再起動拒否／timeout で process group（孫 process 含む）kill／cancel の group kill と冪等／runner 経由 3 arm 完走と再実行で再起動なし。

## 実経路検証済み
なし（実 LLM は呼んでいない）。

## 未実施
- CLI `--executor harness`・ADR 追補 D 番号（cli-adr unit）。lib.rs は `pub mod harness_exec;` のみ追加。
- 同じ out の同時 2 起動の排他 lock は無い（starting 目印の create_new が spawn 二重化は弾く）。

## 提案
- ClaudeCodeBuilder の非対話 permission 設定は harness 側の設定に任せている。本測定の運用設定を確認すること。
