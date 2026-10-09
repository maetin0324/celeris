# ADR 2026-10-09: egress の wildcard 許可・dual-stack・CNAME NODATA

---
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H]
---

- 日付: 2026-10-09
- 状態: 採用
- 関連: [ADR-0102](0102-browser-phase4-isolation-injection-routing.md)、[ADR-0104](0104-browser-egress-transport.md)、[ADR 2026-10-05 allowed origins](2026-10-05-browser-allowed-origins.md)、[ADR-0116](0116-browser-launcher-implementation.md)

## 背景

本番で launcher runtime の browser（task 01M4GYJ3XGJNWZQDF35F1MDE0H、許可 `https://*.tsukuba.ac.jp`）が、
どの site を開いても Chrome の「This site can't be reached」になった。launcher・sandboxd・egress の中継は正しく
動いており、egress が全要求を 403 で拒否していた。原因は 3 つ。

1. allowed origins ADR は wildcard（`https://*.example.com`、apex を含まない）を包含で扱うと決めたが、
   egress の `check_egress` は `host:port` の**完全一致**しか見ていなかった。`*.tsukuba.ac.jp:443` は
   どの host にも一致せず、`manaba.tsukuba.ac.jp` は `not_allowed`。
2. `check_egress` は解決結果に v6 が 1 つでもあれば `ipv6_disabled` で拒否していた。dual-stack の名前
   （example.com、Cloudflare・Google など大半）は IPv6 無効では一切つながらない。
3. egress の DNS 解析は「CNAME の後に terminal の record が無い」応答を常に解決失敗にしていた。CloudFront
   （`www.tsukuba.ac.jp`）は AAAA を持たず、1.1.1.1 は CNAME と terminal zone の SOA だけを返す（RFC 2308 の
   NODATA）。A は得られているのに AAAA の失敗で全体が失敗した。

## 決定

- D1: egress 許可の項目 `*.<base>:<port>` は、`<base>` の真の下位 host（1 ラベル以上。apex・似た名前は不可）で
  port が完全一致するものだけを許す。`<base>` は DNS 名として正しく、public suffix（origin の解析と同じ判定。
  `*.com`・`*.ac.jp`・`*.github.io`）ではないこと。wildcard で許した後も、解決先の検査（非公開・未解決・v6）は
  変わらない。
- D2: 非公開の判定は従来どおり v4・v6 の**全部**に掛ける（1 つでも非公開なら拒否。rebinding 対策）。IPv6 無効なら
  v6 の宛先へは決してつながない: 公開 v6 は接続先の候補から外し、v4 が 1 つも無いときだけ `ipv6_disabled` で拒否する。
  接続先は `egress_destination`（IPv6 無効なら正規形が v4 のものだけ）で選ぶ。
- D3: CNAME の後に terminal の record が無い応答は、authority 節に terminal 名の zone（terminal 自身かその祖先）の
  SOA があるときだけ NODATA（空）とする。SOA が無い・別 zone の SOA・NS（referral）は不完全な chain として従来どおり
  解決失敗。

## 影響

- 許可の範囲は task policy・grant が既に許していたもの（wildcard の包含）に揃うだけで、許可されていない名前・IP literal・
  非公開・DNS 迂回の拒否は変わらない。v6 への接続は引き続き不可。
- launcher 経路と daemon 内 bwrap 経路は同じ `check_egress` と egress binary を使うので両方に効く。本番は
  `/usr/local/libexec/celeris/celeris-browser-egress` の差し替え（host 側の作業）で反映される。
