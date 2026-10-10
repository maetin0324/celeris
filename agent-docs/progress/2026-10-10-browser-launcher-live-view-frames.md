---
title: "launcher Live View frame 経路: close"
tasks: [01M4HS8VQDB3JQHJRJ0ZDBY57H]
status: done
updated: 2026-10-10
---

# launcher Live View frame 経路: close

## 完了

- ADR-0080 の末尾に 2026-10-10 付記を追加した。D3 の緩和を本人の owner session への揮発 frame 表示だけに限定し、agent/LLM/他 viewer への配送禁止、永続化禁止、input 転送の D6 拒否維持を記した。
- 付記は [launcher Live View frame 経路 ADR](../adr/2026-10-10-browser-launcher-live-view-frames.md) を参照する。既存の ADR-0080 本文は変更していない。
- 本 WorkUnit の変更範囲はこの ADR-0080 と本進捗文書のみ。`agent-docs/PROGRESS.md` は変更していない。

## 証拠

| 確認 | コマンド | 結果 |
|---|---|---|
| 付記と進捗ファイル | `grep -q '付記（2026-10-10' agent-docs/adr/0080-browser-phase2-policy-broker-approval.md && test -s agent-docs/progress/2026-10-10-browser-launcher-live-view-frames.md` | exit 0 |
| 実装差分なし | `git diff --quiet "$(git merge-base HEAD main)" -- crates/ web/ gui/ scripts/` | exit 0 |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | exit 0 |
| progress index | `sh scripts/dev/progress-index.sh --check` | exit 0 |

文書検査は新規文書を Git に追加した後に実行した。先行 ADR に記載した実装試験名（`browser_live_frame_*`、`browser_launcher_live_frame_*` など）は今後の実装で実施する試験契約であり、この文書-only WorkUnit では実行していない。

## 未解決

- `live-credential-default`: credential session の本人向け Live View を既定で有効にするか、site policy の opt-in とするか、人の決定待ち。提案 ADR は site policy opt-in を推奨する。
- protocol v6 の実装、各層の frame 非永続化・owner-only・backpressure・input 拒否試験は未実施。実装 task の分割と試験名は提案 ADR の「実装 task の分け方」を参照。

## 提案

- `live-credential-default` は site policy opt-in を採用し、auth section の入力値が本人の画面に見える可能性を site ごとに選択可能にする。
- 実装時は launcher、controller/daemon、gateway/web、層間試験に分け、各段の frame queue を容量 1 にして永続 sink へ到達できない型境界を固定する。
