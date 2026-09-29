# ADR-0086: Browser isolated runtime の egress transport

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 日付: 2026-09-29
- 状態: Accepted（transport の実装。P4-A 全体の受入ではない）
- 関連: [ADR-0085](0085-browser-phase4-runtime-selection.md)、[ADR-0084](0084-browser-phase4-isolation-injection-routing.md)

## 決定

1. host trusted controller の egress transport は引き渡された Unix stream 上の HTTP CONNECT のみを受ける。host TCP listener、worker が変更できる policy、環境変数の HTTP_PROXY/ALL_PROXY、任意上位 proxy は使わない。namespace 内の browser からこの stream への接続と controller の運用配線は別の結合作業とする。
2. CONNECT の authority を解決前に検査する。IP literal、別 port、DNS/DoT port、allowlist 外、proxy 用認証や転送 header、絶対 URL、本文、曖昧な HTTP framing を拒否する。拒否応答とエラーは固定値で、URL/header/DNS packet をログに出さない。
3. DNS は管理者 policy の単一 resolver の TCP/53 に限定する。A/AAAA を両方読み、応答 ID・question・CNAME の連鎖・record owner を照合する。UDP fallback、OS の再解決、search suffix は使わない。全候補に `check_egress` を適用した後、検査済み `SocketAddr` へ直接接続する。connect 時の再解決による DNS rebinding を防ぐ。
4. header、DNS response、CNAME の深さ、接続数（呼出し側）、接続時間、tunnel の時間と転送量を制限する。server は秘密・page content を永続化しない。
5. 実 socket/DNS fixture は transport の負例を検証する。namespace 外への直接接続ができないこと、別 host UID、CDP/IPC、orphan、identity 復元はこの試験の主張に含めない。既存の機密 routing 拒否と H3 は維持する。
6. IPv6 を有効にする場合も通常の global unicast `2000::/3` 以外は拒否し、IETF protocol assignment `2001::/23`、6to4、documentation を保守的に拒否する。mapped IPv4 は IPv4 として検査し、接続時に canonical 化する。IPv4 の廃止済み6to4 relay `192.88.99.0/24` も拒否する。根拠: [IANA IPv6 registry](https://www.iana.org/assignments/iana-ipv6-special-registry/)、[IANA IPv4 registry](https://www.iana.org/assignments/iana-ipv4-special-registry/)（2026-09-29参照）。個別の公開例外を追加する場合は別途方針を決める。
7. `celeris-browser-egress` は1接続ごとの独立プロセス。controller が作った接続済み Unix stream を FD 3、bounded JSON policy を stdin で渡す。接続先 socket のpathやpolicyファイルをworkerと共有しない。stdoutへデータを出さず、stderrとexit codeは固定値のみ。stdin設定待ちも時間を制限する。parent死亡時はSIGKILL、接続終了時はプロセス終了とする。これはproxy実行入口であり、worker/runtimeとのproduction配線・全体適合を意味しない。

## 実環境の制約

この run の sandbox 外では bwrap の user/net namespace 生成は成功した。一方 `unshare --user --map-auto --map-user=1 --map-group=1 id` は newuidmap の EPERM で失敗した。親 user namespace の UID map は `0:100000:1001, 1001:1001:1, 1002:101002:64534` で、`/etc/subuid` の rmaeda 用 `165536:65536` が親の範囲外にある。設定変更・別ホスト実行はこのタスクの範囲外。別 UID の実適合を同 UID bwrap 成功で代用しない。
