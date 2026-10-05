# Browser allowed_domains の origin 形式

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
---

## 決定

`allowed_domains` と task policy の `network_domains` は origin の集合として扱う。形式は `https://host[:port]`、または loopback (`localhost`、`127.0.0.1`、`[::1]`) に限る `http://host[:port]`。port 省略時は HTTPS=443、HTTP=80 とし、正規形では既定 port を省く。初期 seed は `http://localhost:3000` と `http://127.0.0.1:3000` のみ。別の port が必要なら管理者が明示して追加する。

host は ASCII DNS 名または IPv4、括弧付き IPv6 は `[::1]` のみを許す。wildcard は DNS host の `*.` 接頭だけで、apex は含まない。`*` 全体、public suffix 自体への wildcard、userinfo、`/` 以外の path、query、fragment、空や不正な port、HTTPS/HTTP 以外の scheme を拒否する。public suffix 判定は保守的に単一ラベル全体と国別 TLD の二段 suffix 全体を拒否し、`github.io` 等の private suffix は組込み表で拒否する。完全な PSL ではないため、未登録の private suffix は運用前に表を更新する。

旧 host 形式は読み込み時に **HTTPS の既定 port 443 にだけ**写す。例: `example.com` → `https://example.com`、`*.example.com` → `https://*.example.com`。HTTP や他 port への許可は増えない。新たな値は origin 形式で保存する。包含と交差は scheme・host・実効 port の全てで判定し、wildcard の包含を使う。

## 配線

task-core は grant と task policy の交差を origin の集合として計算する。agent-browser の起動引数、broker と egress への渡し方は後続の強制工程で更新する。古い host 形式の実行データも同じ狭める規則で読み取る。

task ごとの新規指定は `Task.requirements.browser.allowed_domains` に置く。API、CoS、execution plan の spec も同じ `requirements.browser.allowed_domains` を使う。新規 task の値には scheme を明示し、旧 host 形式の読み替えは既存 grant と既存実行データの互換読取りだけに限定する。
