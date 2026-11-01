---
title: K2 ops-r2 の開始前チェック修正 — WU cfg（測定 config の方針版引数・B の pool・LLM 経路 off）
tasks: [01M68D4ECCN4V367TYHAKRQZMF]
status: done
updated: 2026-10-31
---
# WU cfg

完了日: 2026-10-31

## 内容
- commit `cfg: 測定 config の方針版引数・B の pool・LLM 経路 off`。変更は `scripts/campaign/ops-r1-measure-config.sh` と `scripts/campaign/test_ops_r1_measure_config.py`（試験）、ADR・adr.md の追記。
- 方針版: `--policy-version <N>` を必須にした（数字のみ）。`[cos.triage] policy_version` へ引数の値を書く。`--skills-src` の cos-inbox-triage の front matter version と違えば止める。固定の 6 は消した。
- B の pool: 旧 `--claude-accounts` を `--b-account-dir` に置き換えた。`--harness real` では必須。`<state>/pool` を作り、b の basename の symlink を 1 つだけ置き、`[accounts] claude_dir` をその pool にする。a・operator の dir は入れない。
- b の検査: `.credentials.json` が直下にあること、`projects/`・`memory/` が無いこと、`~/.claude` の下でないこと、account id が `[A-Za-z0-9_-]` のみで先頭が `.` でないこと。既存の pool が要求と違えば止める。
- LLM 経路 off: `[maintenance.stagnation]` の 4 閾値を 0 にした。`[knowledge.langmem]`・`[knowledge.gc]` は無効、`[memory]`・`[llm_proxy]`・`[[mcp.listeners]]`・`[[cron.seed]]` は置かない。CoS の受信箱 triage は config で切れない（ADR 2026-10-31-ops-r2-preflight-fixes D8）。

## 検査
- 試験は `scripts/campaign/test_ops_r1_measure_config.py` に追加した。この進捗ファイルを書いた時点で再実行はしていない。

## 未解決
- CoS の受信箱 triage は切れないので、試行の task の待ちだけが載る前提のまま。
