---
tasks: [01M4896B1Q1617Q95NT2BNDNAF]
status: done
completed: 2026-10-06
---
# R6: egress が HTTP forward proxy の GET / HEAD を受ける（WorkUnit egress-get）

## やったこと

- ADR `agent-docs/adr/2026-10-06-egress-http-forward-get.md` を先に書いた（scheme は http のみ、Host 必須で URI と一致、hop-by-hop を落として origin-form＋`Connection: close`、1 接続 1 要求、body 付き要求は拒否）。
- `crates/task-worker/src/browser_egress.rs`: `GET`/`HEAD` の絶対 URI を `forward_request` で解析し、CONNECT と同じ DNS 前判定（`check_egress`・試験専用 loopback）→ resolver で解決後の全アドレス検査（SSRF 拒否）→ 固定 IP へ接続、を通す。上流には origin-form の要求を 1 つ書き、応答を上限内で流し終えたら書き側を閉じ、残りの client 入力は読み捨てて閉じる（未読入力のまま閉じると Unix stream の相手に reset が届くため）。拒否は `Denial`（`not_allowed`・`ip_literal`・`private_address` など CONNECT と同じ種類と、新しい `scheme_not_allowed`・`host_mismatch`・`request_body`）で返す。
- `crates/task-worker/src/browser_runtime.rs`: `DenialRecorder` の受ける種類に新しい 3 種類を足した（足さないと `egress-denied.jsonl` に書かれない）。
- 試験 `browser_egress/tests.rs` に 8 件を追加（in-process の loopback 上流＋`UnixStream::pair`、https の試験証明書は使わない）:
  - `egress_get_allowed_origin_is_forwarded_in_origin_form` — 許可 origin の GET が `GET /page?q=1`＋`Connection: close` で届き、応答が返る
  - `egress_head_and_default_path_are_forwarded` — HEAD と空 path → `/`
  - `egress_get_query_only_uri_uses_origin_form` — query だけの URI も `/?q=1` に変換
  - `egress_get_drops_hop_by_hop_headers` — Connection が列挙する名前・Proxy-*・Keep-Alive・TE・Trailer・Upgrade が落ち、Cookie などは届く
  - `egress_get_serves_one_request_per_connection` — pipelining した 2 つ目は上流に届かず、応答 1 つで EOF
  - `egress_get_switching_origin_on_the_same_connection_never_connects` — 同じ接続で別 origin への 2 つ目は接続されない
  - `egress_get_denials_are_recorded_like_connect` — 許可外 origin・IP literal・Host 欠落/不一致・https・body・POST・userinfo・origin-form・HTTP/1.0・obs-fold・単独 LF が 403 で拒否され、理由が `egress-denied.jsonl` に記録され、path・ヘッダ・body の秘密を含まない
  - `egress_get_allowed_name_is_checked_after_dns` — 許可した名前が private address に解決されると `private_address` で拒否、公開アドレスは固定されて origin-form の転送になる
  - 既存試験の `GET https://…` の拒否は `scheme_not_allowed` の拒否として引き続き pass。

## 証拠

- `cargo test -p task-worker --lib browser_` → 90 passed, 0 failed
- `cargo test -p task-worker --lib browser_egress` → exit 0、24 passed, 0 failed
- `cargo test -p task-worker --lib` → exit 0、805 passed, 0 failed, 4 ignored
- `cargo clippy --workspace --all-targets -- -D warnings` → exit 0

workspace 全体の `bash scripts/dev/test-parallel.sh` は統合後の verify 葉で回す（この WU の変更は task-worker の browser_egress・browser_runtime の記録種類の表だけ）。

## 未解決事項

- 本番に効くのは host の `celeris-browser-launcher`／egress バイナリを入れ替えた後（人の作業）。実機の 4 回目は Fable。
- policy の `allow` は `host:port` で scheme を持たないので、https で許可した `host:443` へ `GET http://host:443/` も同じ許可判定で通る（CONNECT で `host:80` を許可したときと対称）。scheme まで分けるなら `allow` の形を変える必要がある。

## 提案

- `EgressPolicy.allow` に scheme を持たせ、GET（http）と CONNECT（https）を origin 単位で分ける（上の未解決事項）。
