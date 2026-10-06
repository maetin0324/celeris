# ADR 2026-10-06: CoS chat harness の能力と画像 delivery の境界

---
tasks: [01M47VN94QK6KHXQDNPCA0V7KK]
---

- 日付: 2026-10-06
- 状態: 能力層を実装。adapter 個別の配線は後続 WorkUnit
- 正本: [CoS chat home](2026-10-05-cos-chat-home.md) D2「config と harness の写像」および D4

## 決定

`task-worker::cos_chat::HarnessCapabilities` は継続方式、native image、画像読取 tool、shell、filesystem、MCP を表す。`for_adapter("claude-code"|"codex"|"acp")` は D2/D4 の実装目標を返す。OpenCode は `acp` として扱い、image/resource と terminal/fs/MCP は実行時のプロトコル確認前には false とする。実行可能だと確かめた能力だけを `CosChatContext.harness_capabilities` に載せる。この欄が無い run は能力未確認であり、静的な実装目標から実行時の成功を推測しない。

`image_delivery` は manifest の `delivery=file` を path、`delivery=image` を確認済みの native image → native、画像読取 tool → path+tool、どちらも無い → unsupported に写す純関数。添付の prompt 行にはこの結果を `actual=` で書く。unsupported の理由は画像を見ていないことを明示する。path+tool は画像読取 tool の使用前に内容を断言しない。その他の能力不足も `capability_reason` が status/reason に使える共通文言を返す。

継続方式は Claude の session resume、Codex の exec resume、ACP の session load として区別する。resume 不能時の fresh と DB 履歴は後続の adapter が実装する。非 CoS 判定は既存どおり `RunContext.cos_chat` の有無のみ。通常の conversation と ADR-0140 の WU 継続経路に変更を入れない。

## 実装範囲

この段階は能力表・delivery 判定・共通 reason・prompt のみ。adapter の CLI 引数、ACP メッセージ、能力交渉、画像バイトの受け渡しは後続の各 harness WorkUnit が接続する。dispatcher は能力未確認を `None` として渡し、画像の受け渡しができたと主張しない。
