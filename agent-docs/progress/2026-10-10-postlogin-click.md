---
title: "ログイン後の click / snapshot の失敗（隠れた password 欄）、snapshot の URL、navigate の制限、download 試験の決定化"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
status: done
updated: 2026-10-10
---

# post-login click（2026-10-10）

- branch: `ops/postlogin-click`（main 1b6c354c から）。決定は ADR 付記 2026-10-10c。
- 原因: 実 agent-browser 0.38.1 を relay 越しに動かす試験で、click の CDP は区間後の検査を通ることを確認。本番の失敗は
  manaba の home 系の頁の隠れた空の password 欄を数えて、その頁の観測（snapshot・screenshot・click）を全部拒否していたため
  （見立て。本番の頁の中身は見ていない）。
- 直したこと: 生きている password 欄だけ数える、`snapshot --urls`（2026-10-10d: 当初の `-i` は本文を落としたので外した）、ログイン後の agent の navigate は read_origins だけ
  （navigation の download は event が出ず取消できないことを試験で確認したため）、他 origin の download の試験を新しい tab の
  頁起点の download と event 待ちにした（Chromium の tab ごとの download 制限が時間依存の原因）。

## main（51989f4e）との統合

download の試験は 1 つの仕組みにまとめた: main 側（34f51498・1a6f55e7）の guid 付きの出来事待ち（`wait_event`・
`wait_download_begin`・`wait_download(guid)`）と sniff されない型・本文 60 秒遅れの fixture（Chromium が octet-stream を
sniff して本文まで download を始めず取消と競合する点を除く）に、こちらの「download 未経験の新しい tab で頁起点の click」
（tab ごとの download 制限を避ける）と「区間後の navigate は read_origins だけ」の拒否確認を合わせた。こちらの時間上限だけの
待ち（`wait_download_for`・再試行）は削除。

## 証拠

- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` → exit 0、5027 passed / 0 failed（1 回目は無関係の `celeris-mcp unreachable_server_is_a_transport_error`（他の server が port に答えた）と、新しい実 agent-browser 試験の socket path 長で 2 件失敗 → 試験側を短い socket dir にして解消）
- `cargo clippy --workspace -- -D warnings` exit 0（`--all-targets` も 0）、`cargo fmt --all -- --check` exit 0
- `launcher-admission-evidence.sh --credential` → 2 回とも `EXIT: 0`（credential-unit 36、credential-broker 5）
- 新しい試験 `real_agent_browser_reads_clicks_and_is_refused_after_login`（実 agent-browser 0.38.1。無い host では skip）

## 未解決事項

- 本番の manaba の home 系の頁が実際に何で拒否されていたか（隠れた password 欄か、生きている欄か）は、修正後の run で確かめる。
- read_origins の URL が他 origin の添付へ redirect するときの `open` は download の event が出ず取消できない（egress の範囲）。
