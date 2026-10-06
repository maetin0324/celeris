---
title: 確認台本の F2・F3 修正
tasks: [01M48DGY90GJAW2T8C95267HV3]
status: done
updated: 2026-10-06
completed: 2026-10-06
---

# 確認台本の F2・F3 修正

WorkUnit `check-script`。`scripts/dev/browser-web-live-check.sh` と `docs/ops/browser-web-live-check.md` を修正した。

- F2: 試験ページに `#forbidden-link` を追加し、task に open → snapshot → click を指示。run の許可ページ GET と現在の launcher session の egress 拒否記録を確認・保存してから pause → takeover → release → decision deny を行う。拒否記録は session が生きている間に読む。
- F3: `127.0.0.1:<DENIED_PORT>` の拒否理由を `ip_literal` として検査する。host、port、時刻、session id の検査と、不許可ページに GET がないことの検査を維持した。
- 台本の実機実行はしていない。launcher と Chrome を要する再確認は統合後に行う。

確認: `bash -n scripts/dev/browser-web-live-check.sh`、埋め込み Python の構文検査、段コメントの順序・一意性、`git diff --check`、`crates/` の差分なし。
