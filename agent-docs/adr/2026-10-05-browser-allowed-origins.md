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

## 強制と grant 縮小の伝え方（enforce 葉、2026-10-05 追記）

- 実効許可は「保存された task policy の `network_domains` ∩ `Task.requirements.browser.allowed_domains` ∩ 担当 node の grant」。前の二つの交差は `task_core::browser::task_run_policy`、grant との交差は既存の `EffectiveBrowserPolicy::derive` が行う。どちらも origin（scheme・host・実効 port）で判定し、wildcard は包含で扱う。requirements を持たない旧 task は保存 policy をそのまま使う（grant 全体には広げない）。
- dispatcher は run ごとに（`run_extras` が org を DB から読み直した）現在の grant と、狭めた task policy を `RunContext.profile.browser`・`RunContext.browser_policy` に入れる。task 作成時の grant は保存しない。起動前に `task_worker::browser_policy::admit` で同じ交差を計算し、空・不正なら process を起動せず `browser policy rejected: <固定コード>`（空は `empty_browser_domains`）で run を失敗にする。
- worker は同じ入力から `browser_policy::prepare_for_task` で同じ交差を作り（狭めは冪等）、action server・shared CDP の navigate（`url_origin_allowed`）、credential 要求の origin 判定、egress の `host:port` 許可（`PreparedBrowserPolicy::egress_allow`。CONNECT には scheme が無いので scheme は port〈HTTPS 443・HTTP 80〉で判定）の全てに使う。
- 実行中の run には grant の変更を流し込まない。run の policy は起動時に確定し、grant 縮小は同じ task の**次の run**から効く。人待ち（browser wait）の承認・再開は policy の binding hash を照合するので、縮小で交差が変われば hash が変わり、縮小前に承認した credential 使用や待ちは次の判定で拒否される。実行中の run を直ちに止める必要がある場合は、管理者がその run を取り消す（既存の取消経路）。

## Browser settings API

管理 API は `PATCH /api/v1/org/{id}/browser-settings` とする。`browser-execution` は既存の
`/api/v1/org/{id}` で node id として解釈されるため、専用の下位経路を使う。
body は `allowed_domains`、`credential_policy_ids`、`credential_identity_ids`、
`harnesses`、`budget` の任意の組み合わせを受ける。更新は既存の browser grant を持つ
node に限り、成功時に org browser event を同一 transaction で追記する。
