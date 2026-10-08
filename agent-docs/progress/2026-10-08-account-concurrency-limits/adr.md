---
title: ADR: アカウントごとの同時実行数、暗黙アカウント、プロバイダ concurrency の deprecate
tasks: [01M4D7RVKXY9A967ZXDADWVHQJ]
status: done
updated: 2026-10-08
---
# 進捗: アカウントごとの同時実行数（WorkUnit adr）

- task: `01M4D7RVKXY9A967ZXDADWVHQJ`、run `01M4D9PZY1SHNH1D0BFN3BARKG`（2026-10-08）
- 成果: [ADR 2026-10-08 account-concurrency-limits](../../adr/2026-10-08-account-concurrency-limits.md)

## したこと

- ADR を書いた。(a) `[[providers]].concurrency`・API の `concurrency` 入力・`capacity::provider_full`・web/gui の入力欄の deprecate（応答の `concurrency` は
  deprecated として残し、`in_use` / `in_use_cos` は観測値）、(b) プロバイダ行は LLM source 管理に専念、(c) 欄名 `max_runs` と実効上限の順
  （保存値 → 暗黙なら旧 concurrency → `max_runs_per_account`）、CoS +1 は実アカウントだけ、(d) 保存先は SQLite の `account_limits` 表
  （理由: 暗黙アカウントに根ディレクトリが無い・AccountBook は壊れると空に戻る観測値の帳簿・秘密の置き場と離す）、PATCH の本文・範囲 1..=32・400/404/409・
  路の衝突の扱い、(e) 人の決定 `nonpool-limit` = (ii) implicit-account（`implicit/<provider id>`、一覧の印、削除は行に追従、既定は旧 concurrency を引き継ぐ、CoS +1 無し）、
  (f) llm-proxy の `max_concurrent_per_account` ほか対象外の列挙。
- 上書きされる ADR-0012・0017・0024・0089・0132 の末尾に、この ADR を指す付記を足した。

## 確かめたこと

- 本番 config（`~/.config/celeris/config.toml`、読むだけ）の非プール行 `opencode-qwen`・`pi-qwen`・`ldr-qwen`・`paperqa-qwen`・`langmem-main` は
  どれも `concurrency = 1` を明示している。暗黙アカウントの既定が旧 concurrency を引き継げば、config を変えなくても 1 本で止まる。
- `sh scripts/dev/check-adr-numbers.sh` と `sh scripts/dev/check-doc-links.sh agent-docs/adr agent-docs/progress/2026-10-08-account-concurrency-limits`。

## 申し送り

- この task には celeris の worktree が付いていなかった（task の repos が空）。ADR は `main`（`14cddd28`）から作った sparse clone の
  branch `celeris/01M4D7RVKXY9A967ZXDADWVHQJ` に置き、同じ内容を task の artifacts にも写した。
