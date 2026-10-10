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

## 決定の記録（record-decision、2026-10-10）

- 決定 `live-credential-default`: credential session（ログイン後を含む）の Live View は既定で本人の owner session に表示する。site policy の opt-in は設けない。
- 回答者: 人（運用セッション経由、2026-10-10 の発言）。決定の task id は `01M4HS8VQDB3JQHJRJ0ZDBY57H`。
- ADR の反映: [ADR 2026-10-10 D7](../adr/2026-10-10-browser-launcher-live-view-frames.md) の「未決」節を「決定」節へ移した。ADR-0080 の 2026-10-10 付記にも 1 行で反映した。
- 実装 task の分け方の修正: site policy の欄は作らない。gateway/web の試験は `browser_live_credential_default_owner_only`（既定表示・非 owner 拒否）に置き換えた。`browser_live_credential_default_policy` は削除した。
- 食い違い（人の確認が要る）: 決定の選択肢の表記に「（推奨どおり）」と付いていたが、初稿 ADR の推奨は site policy opt-in であり、選ばれた「既定で出す」と一致しない。人の発言（資格情報を入力した本人だけが見る）に沿って既定表示を採った。表記だけの誤りか、推奨を変えたかは人に確かめる。

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| 未決が決定へ移った | `grep -c '## 未決' agent-docs/adr/2026-10-10-browser-launcher-live-view-frames.md` | 0 件 |
| 回答・回答者・日付・task id | `grep -n 'live-credential-default' agent-docs/adr/2026-10-10-browser-launcher-live-view-frames.md` | D7（決定済み）に回答・回答者・2026-10-10・01M4HS8VQDB3JQHJRJ0ZDBY57H を記載 |
| ADR-0080 付記 | `grep -n 'live-credential-default' agent-docs/adr/0080-browser-phase2-policy-broker-approval.md` | 2026-10-10 付記に 1 行反映 |
| 実装差分なし | `git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/ gui/ scripts/` | exit 0 |
| 文書リンク | `sh scripts/dev/check-doc-links.sh` | exit 0 |
| ADR 番号 | `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (177 files)`、exit 0 |
| progress index | `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok`、exit 0 |

## 未解決

- 決定の表記の食い違い（上記）。人が「既定で出す」を選んだ意図で確定してよいかの確認。
- protocol v6 の実装、各層の frame 非永続化・owner-only・backpressure・input 拒否試験は未実施（実装 task）。

## 提案

- 実装時は launcher、controller/daemon、gateway/web、層間試験に分け、各段の frame queue を容量 1 にして永続 sink へ到達できない型境界を固定する。
- 決定の選択肢の表記は、次回から推奨と一致するか planner が確かめてから人に出す。
