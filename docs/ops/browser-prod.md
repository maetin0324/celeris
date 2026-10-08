---
tasks: [01M4CH1MPV1KB69SYKXS79E0SP]
---

# browser 本番運用: 適合台帳

browser task は release dir の適合台帳（`releases/<sha12>/browser/conformance.json`）が無い・古いと dispatch 前に止まる（ledger-gate）。
通常は `scripts/selfdeploy/release.sh` の browser-ledger 段が作る。台帳の生成に失敗しても release は失敗しない（未配置のまま、ledger-gate が止める）。

## 台帳を作り直す（人が実行する）

1. 台帳の状態を見る: `celerisctl browser ledger check`（ok なら何もしなくてよい）。
2. 作り直す: `bash scripts/selfdeploy/browser-ledger.sh <sha12> [--force]`。agent-browser と LLM 認証が使える host で実行する。
3. 置けたら daemon は台帳の mtime を見て再起動なしに拾う。置けなければ既存の台帳を残して exit 1。

site policy・credential の設定手順は preflight 葉で追記する。
