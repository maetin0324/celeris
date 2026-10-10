---
task: 01M4JAK3MY5G1K8T66Q4RTWSS5
unit: ops-doc
status: done
completed: 2026-10-10
---

# ops-doc: launcher Live View（protocol 8）の本番反映手順

## 成果

- `docs/ops/browser-launcher-live-view.md`（新規。1 行目 `# 題名`、front matter `tasks`）: 差し替え順と v7/v8 互換表・根拠
  （機能ごとの版下限、frame は専用接続だけ）、release 昇格と web 追従、launcher の再 build（`/local` scratch の
  `CARGO_TARGET_DIR`）・退避・差し替え・admission 実証、doctor の `launcher` が protocol 8 で OK になる確認（両方揃うまで NG は期待どおり）、
  台帳の再生成（ledger 用 `CARGO_TARGET_DIR`）、Live View 確認（本人表示・credential session・非 owner 401/403・input 不可・後始末）、戻し方。
- 相互リンク: `docs/ops/browser-launcher-credential-release.md`・`docs/ops/browser-web-live-check.md` から新手順へ。
- コードは変えていない。

## 証拠

- `sh scripts/dev/check-doc-links.sh docs/ops` → `check-doc-links: ok`、exit 0
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`、exit 0
- `git diff --check` → exit 0
- 手順で名指した試験名・UI 文言・定数（`PROTOCOL_VERSION = 8`、`browser_launcher_daemon_checks_live_protocol_before_enable`、
  `browser_launcher_v7_continues_without_live_view`、`browser_launcher_live_view_rejects_input`、live-view-frame.tsx の文言）は grep で実在を確認。

## 未解決事項

- 手順は本番で未実行（運用セッションが配送後に行う）。
- 別 task（launcher artifact transfer）が protocol 8 を fetch_artifact に使い Live View を 9 とする計画の記録がある。両方が main に入る順によっては
  この手順の「protocol 8」と版下限の表を統合時に直す必要がある。

## 提案

- `celerisctl browser doctor` の `launcher` 行は版の完全一致で NG を出すため、差し替えの間の互換状態（v8 daemon + v7 launcher は Live View のみ無効）
  を WARN として区別すると運用で迷わない。
