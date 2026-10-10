---
title: "launcher Live View frame 経路: protocol v8 実装契約"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# launcher Live View frame 経路: protocol v8 実装契約

## 決定の索引

- [ADR 付記 2026-10-10b](../adr/2026-10-10-browser-launcher-live-view-frames.md#付記-2026-10-10b実装-protocol-v8) が実装契約の正本。本文の v6/v5 frame 契約を v8/v7 として読み替える。本番 launcher は protocol v7（post_login v6・consent v7）。
- v8 は `live_start` / `live_stop` と `live_frame` notification を定義する。frame metadata は JSON、body は長さ前置の bounded binary（上限 2 MiB）。frame は daemon が `live_start` を送る専用別接続だけを使う。
- v8 daemon × v7 launcher は `launcher_protocol_no_live_frames` で Live View を無効にする。v7 daemon × v8 launcher は frame を要求しない。いずれも session・credential login・consent は従来どおり。
- daemon 内は非 Serialize / 非 Debug の揮発 `LiveFrame`、容量 1 の `LatestFrameSlot`、既定 `None` の LiveSessionEntry 購読口で中継する。
- task-api frame stream は既存 relay assertion と grant を再確認し、no-store の長さ前置 binary を配信する。grant 失効または owner 切断で閉じ、run 一覧に frame 可否を返す。
- gateway は既存 owner session / Origin / grant guard 付き WebSocket で本人だけへ配送する。frame 経路がある場合 `liveAvailability` は enabled/link。credential session の本人表示を維持し、input は拒否する。
- D6 の path 範囲と試験 prefix は v8 unit 名に更新済み。frame-core、launcher、api-stream、daemon、gateway、spa、cross-tests、ops-doc の境界を ADR 付記に記録した。

## 版の訂正（docs-v9、2026-10-10）

- 版の正は `crates/task-worker/src/browser_launcher/protocol.rs`: `ARTIFACT_PROTOCOL = 8`（fetch_artifact / artifact）、`PROTOCOL_VERSION = LIVE_FRAME_PROTOCOL = 9`（live_start / live_stop / live_frame）。完了節の「protocol v8」は Live View の版として誤りで、Live View は v9 と読む。
- ADR 付記 2026-10-10c（版の訂正・互換表（正）・merge 崩れの修正）を追加。付記 2026-10-10b の「v8」「v7」の読み替え規則を明記し、試験名は実装の名前（`browser_launcher_protocol_v9_frames`、`browser_launcher_v8_continues_without_live_view`）に揃えた。
- `docs/ops/browser-launcher-live-view.md` を protocol 9 に直した。v7 と v8（artifact のみ）の launcher からの更新手順、doctor の `protocol 9` 確認、退避版の名前（`$L.pre-live-view`、版は v7 または v8）、戻し方の互換を書いた。
- 変更は ADR・docs/ops・本進捗だけ。`crates/` は触っていない。
- 版の訂正の検査は本節の後段（regate）で文書検査と合わせて取り直す。

## 人の決定の記録（live-protocol、2026-10-10）

- 決定の要約: Live View = protocol 9、artifact = protocol 8 のまま。回答は CoS の代答で、人の決定ではない。理由は番号衝突の回避（protocol 8 は main の artifact transfer が使用済み）。
- 要件・親 task 題名の「v8」は「Live View 無し・artifact あり」と読む。v7/v8 の launcher・daemon と v9 の組では Live View だけが無効（`launcher_protocol_no_live_frames`）で、session・credential login・consent は動く。
- 詳細・既存試験名・本番差し替え手順の所在は [ADR 付記 2026-10-10d](../adr/2026-10-10-browser-launcher-live-view-frames.md#付記-2026-10-10d人の決定-live-view-は-protocol-9)。

## 完了

全 unit を統合した HEAD `127e53e3d0a02576d160465fc566b64ed55323ea` で全体検査を完了した。全体検査はすべて exit 0。

| 検査 | 結果 |
|---|---|
| `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。nextest 5,054 passed / 0 failed / 13 skipped、doc tests 3 passed（別途 ignored 1）。`CELERIS_TEST_SUMMARY`: 5,057 passed / 0 failed / 14 ignored。 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `pnpm -C web lint` | exit 0。既存 `styles.css` の `!important` 4 件は warning。 |
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0。Vitest 91 files / 657 tests passed、Node 86 passed / 0 failed。 |
| `pnpm -C web build` | exit 0。chunk size warning あり。 |

失敗した試験名はない。本体・試験の修正は不要だった。

## 未解決事項

- 実 Chrome と本番 launcher を使った映像確認および本番反映は未実施。launcher の再 build・差し替えと本人向け確認手順は [運用手順](../../docs/ops/browser-launcher-live-view.md) に記録済み。
- web lint の既存 `!important` 警告 4 件と build の大きな chunk 警告が残る。いずれもこの Live View の gate を妨げない。

## 再検査（regate、2026-10-10）

統合後 HEAD `651a36cb` で全体 gate を取り直した。詳細は [reclose.md](2026-10-10-browser-launcher-live-view-v8/reclose.md)。

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0、passed 5070 / failed 0 / ignored 14。
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require` 付き: exit 0、passed 5070 / failed 0、`userns: true`。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- `pnpm -C web test`: exit 0（Vitest 657 tests、Node 86 tests）。`pnpm -C web exec biome check .` は exit 0 で warning 4 件（既存の `styles.css` の `!important`）。
- 文書検査 3 本（check-doc-links / check-doc-layout / check-adr-numbers）exit 0。
- 未解決事項は上の節のとおり（映像確認と本番反映は運用側）。

## 提案

- 運用セッションで手順に沿って launcher を更新し、本人の credential session を含む実 browser で Live View を確認する。
