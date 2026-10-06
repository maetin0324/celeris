# browser egress: HTTP forward proxy の GET / HEAD（絶対 URI）を受ける

- 日付: 2026-10-06
- 状態: 実装済み（task 01M4896B1Q1617Q95NT2BNDNAF、WorkUnit egress-get）
- 関連: ADR-0104（bounded CONNECT transport）、ADR-0102（隔離 runtime と egress）、3 回目の実機確認の R6（`agent-docs/progress/2026-10-05-browser-web-live-view/real-check-evidence/rerun3-2026-10-06/`）

## 背景

`crates/task-worker/src/browser_egress.rs` は CONNECT だけを受けていた。Chrome は proxy 越しに `http://` のページを開くとき CONNECT を使わず、絶対 URI の通常の要求（`GET http://127.0.0.1:17730/ HTTP/1.1`）を proxy へ送る。これが `malformed` で落ち、http の許可 origin（試験用の loopback を含む）がどれも開けなかった。人の決定（R6）: egress を HTTP forward proxy の GET にも対応させ、CONNECT と同じ許可判定と拒否理由の記録を通す。

## 決定

### D1. 受ける形と scheme

- 要求行は `GET <絶対 URI> HTTP/1.1` か `HEAD <絶対 URI> HTTP/1.1` だけ。他の method（POST など）・HTTP/1.0・origin-form の要求は従来どおり `malformed` で拒否。
- scheme は `http` だけ（大小文字は問わない）。`https://` の GET は拒否し `scheme_not_allowed` を記録する。https は CONNECT 経由だけ（TLS は browser と origin の間で終わり、egress は中身を見ない）。
- URI の authority は `host[:port]`、port の既定は 80。userinfo（`@`）・fragment（`#`）・非 ASCII・port 0/53/853・先頭 0 付き port は拒否。path が空なら `/`。

### D2. 許可判定は CONNECT と同じ

URI の host:port に CONNECT と同じ判定を通す。DNS 前に `check_egress`（task ∩ grant の `allow`、IP literal・不正 host の拒否。`Unresolved` 以外は拒否）、試験専用 loopback 許可（`test_loopback_allow` の `127.0.0.1:<port>`）は DNS を引かず直結、それ以外は設定の resolver で A/AAAA を引き、全部の解決先を `check_egress` で検査（private・rebinding・IPv6 無効を拒否）してから最初の解決先へ固定して接続する。拒否理由は CONNECT と同じ `Denial`（`not_allowed`・`ip_literal`・`private_address` など）で返し、`celeris-browser-egress` が stderr へ 1 行、`DenialRecorder` が `egress-denied.jsonl` に書く。記録の host は文字種と長さを確かめた URI の host だけで、path・query・ヘッダは記録しない。GET で増える理由（`scheme_not_allowed`・`host_mismatch`・`request_body`）は `DenialRecorder` の受ける種類の表（`browser_runtime.rs`）に足す。

### D3. Host ヘッダは必須で URI と一致

Host ヘッダが無い・重複・`host[:port]`（port の既定 80）が URI の host:port と一致しない、のいずれも拒否し `host_mismatch`（URI の host と port 付き）を記録する。上流へ送る Host は URI の authority（一致を確かめた値）。

### D4. hop-by-hop ヘッダを落とし origin-form で送る

- 落とす: `Connection` と、その値に列挙された名前、`Proxy-Connection`、`Proxy-Authorization`、`Keep-Alive`、`TE`、`Trailer`、`Upgrade`（RFC 9110 §7.6.1 と proxy 認証の資格情報）。
- それ以外の end-to-end ヘッダ（Accept・Cookie・User-Agent・Referer など）はそのまま渡す。ヘッダ名は token 文字だけ、継続行（obs-fold）や空白入りの名前は `malformed`。
- 上流への要求行は `<METHOD> <path?query> HTTP/1.1`（origin-form）、末尾に `Connection: close` を付ける。URI が query だけなら path は `/` にする。単独の CR/LF を含む要求行・ヘッダは拒否し、上流へのヘッダ注入を防ぐ。

### D5. keep-alive はしない（1 接続 1 要求）

proxy は 1 接続で要求を 1 つだけ読む。上流の応答を（転送量上限 64 MiB・時間上限 300 秒の中で）上流が閉じるまで流したら client 側も閉じる。後続の要求（pipelining）は読まずに捨てる。同じ接続で次の要求を別 origin に向け、判定済みの接続へ乗り換える穴を作らないため。応答を流し終えたら書き側を閉じ、client が送り残したものは読み捨てて（64 KiB・1 秒まで、解釈しない）から閉じる。Unix stream は未読の入力を残して閉じると相手に reset が届き、直前の応答を失わせうるため。Chrome は応答の `Connection: close` を見て次の要求で新しい接続を張るので、要求ごとに判定が通る。

### D6. body 付き要求は拒否

`Content-Length` が 0 以外、または `Transfer-Encoding` があれば拒否し `request_body` を記録する。`Content-Length: 0` は受けて上流へは送らない（body が無いことは変わらない）。ヘッダ全体の長さの上限は CONNECT と同じ 8192 byte。

## 試験

`browser_egress/tests.rs` で in-process の loopback 上流（`TcpListener`）と `UnixStream::pair` だけを使う。https の試験証明書は使わない。

- 許可 origin（試験専用 loopback）の GET が上流に origin-form・`Connection: close` で届き、応答が client に返る。HEAD と query だけの URI も同じ。
- hop-by-hop ヘッダ（Connection が列挙する名前を含む）が上流に届かず、end-to-end ヘッダは届く。
- 許可外 origin・IP literal・DNS 後の private address の GET が拒否され、拒否理由が `Denial` と `egress-denied.jsonl` に記録される（path・ヘッダの秘密を含まない）。
- Host の欠落・不一致、https の GET、body 付き要求の拒否と記録。
- pipelining した 2 つ目の要求は上流に届かず、接続は 1 要求で閉じる。
