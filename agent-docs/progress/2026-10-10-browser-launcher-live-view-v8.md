---
title: "launcher Live View frame 経路: protocol v8 実装契約"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: running
updated: 2026-10-10
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

## 状態

この WorkUnit は ADR と親進捗索引のみを更新した。コード実装・後続 unit の検証・本番反映手順の作成は後続 WorkUnit の範囲。
