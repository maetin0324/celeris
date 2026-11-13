---
title: 入口の Cloudflare の 403 を認証の失敗と取り違えない
tasks: [01M7A9RJRPEMV9M4M65R0P39PJ]
status: done
updated: 2026-11-13
---
# 入口の Cloudflare の 403 を認証の失敗と取り違えない

完了日: 2026-11-13

## 変更
- ADR `2026-11-13-edge-block-is-not-auth-failure`: 遮断の印の一覧と、認証の文言と同じ文にあるときは遮断を優先する規則（ADR 2026-10-17 D1 に付記）。
- `crates/task-worker/src/provider.rs`: `EDGE_BLOCK_PATTERNS` を判定の最初に見て Throttled（60 秒）にする。
- `crates/task-worker/src/provider/tests.rs`: 今回の文は Throttled、OAuth 切れ・401・not logged in は AuthFailed、403 だけは未分類の 3 試験。

## 証拠（提出時の HEAD で実行）
下の「検査」節に記載。

## 検査
- `cargo nextest run -p task-worker provider`、`bash scripts/dev/test-affected.sh`、clippy、fmt の結果は報告本文に記載。

## 未解決
- 無し。旧い分類を固定する試験は無かった（既存の OAuth 切れの試験は規則の下でも有効）。

## 本番への移行
config・DB の変更は要らない。deploy 後に手順は無い。

## 提案
- 同じ harness の全アカウントが同じ時刻に AuthFailed（または cooldown）になったとき、BadNews（ADR 2026-10-17 D5 の通知とは別に「harness の全滅」）を 1 回記録して人に知らせる。今回は CoS の run が 41 分失敗したが誰にも知らされなかった。実装はこの task の範囲外。
- 遮断が連続したとき（例: 同一アカウントで 60 秒 cooldown が N 回続く）の通知・backoff の伸長も検討する。
