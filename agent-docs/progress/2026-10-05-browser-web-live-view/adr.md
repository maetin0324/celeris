---
title: ADR — ブラウザ実行課と web の Live View・操作・承認画面の設計
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# ADR — ブラウザ実行課と web の Live View・操作・承認画面の設計

WorkUnit `adr`。[2026-10-05-browser-department-web-live-view](../../adr/2026-10-05-browser-department-web-live-view.md) を書いた。コードは変えていない。

## 決めたこと（要点）

- D1 部署: `browser-execution`（ブラウザ実行課、section、親 `engineering`、genre `coding`）。
  - grant: `browser.allowed_domains = ["localhost","127.0.0.1"]`（loopback から始める）。`credential_use` 無し、live_view_url 無し。
  - 予算: 本番 `coding` harness の 120 turn / 7200 秒に、node の `budget {max_lane: standard, max_attempts: 2}` を足す。
  - adapter: `claude-code` を推奨する。
  - 置き場所の理由は 3 つ。
    - coding harness を継げる
    - 「cos > 部 > 課」の形を保てる
    - grant が継承で広がらない
- D2 gateway: `web/server/browser/` に旧 GUI の owner session・attestation・live relay・control・waits・identity を移す。
  - 認可の判定は 9 段の guard で毎回行い、拒否は固定コードで返す。
  - 宛先は loopback の `CELERIS_WEB_LIVE_VIEW_UPSTREAM` だけ。live grant は更新する。
  - generic `/api` relay では browser の変更系を `browser_route_required` で拒否し、`live_view_url` を redact する。
- D3 画面: `web/features/browser/` に置く。
  - route は `/browser`・`/browser/runs/$taskId/$runId`・`/projects/$id/browser-identities`。nav は 1 行だけ足す。
  - 「監視のみ／あなたが操作中」を明示する。返却は 3 層（resume・離脱時の disconnect・期限切れで paused）。
  - 旧 GUI との対応表は D3.7。
- D4 試験: 次の 4 種。
  - gateway 単体: `node:test`、偽 daemon と偽 dashboard、注入した時計
  - fake-daemon の rich fixture
  - Playwright
  - opt-in の実機台本（未実行は exit 2）

## 証拠

- `sh scripts/dev/check-doc-links.sh`: exit 0（ok）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0（ok）
- `sh scripts/dev/check-adr-numbers.sh`: exit 0（141 files）
- `sh scripts/dev/progress-index.sh --check`: exit 0（ok）
- 範囲 check: `git diff --cached --name-only "$CELERIS_WU_BASE"` の差分は 2 file（ADR と本ファイル）だけ
- 次の 2 つは走らせていない。この WU は文書だけを変え、crate・web に差分が無いため。close-out の葉で走らせる。
  - `bash scripts/dev/test-parallel.sh`
  - `cargo clippy --workspace -- -D warnings`

## 未解決事項

- launcher runtime（本番）に agent-browser dashboard の upstream があるか未確認。無ければ Live View の映像は出ず、
  イベントによる監視と lease 操作だけになる。real-check で確かめる。
- `celerisctl browser owner-session approve` が GUI 以外の socket path を受けるか未確認。受けなければ葉 gateway で CLI に引数を足す。
- 人の決定を 3 件出した。
  - `browser-live-input`: lease holder の入力を転送するか
  - `browser-default-adapter`: browser task の既定 adapter
  - `browser-initial-domains`: grant の初期 origin

## 提案

- task-api に `GET /api/v1/browser/runs`（browser session の一覧と状態）を足す。
  gateway が tasks と events から合成する現案（D2.4）は、件数の上限があって重い。
- 旧 GUI の browser 画面の撤去は、web で実機確認が済んでから別 task で行う。
