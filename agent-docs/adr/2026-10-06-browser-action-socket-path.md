# browser: shim と daemon の間の action socket を短い固定長の場所に置く

- 日付: 2026-10-06
- 状態: 実装済み（task 01M47QXZR0QMCYZM9KAZC81BCD、WorkUnit run-path）
- 関連: ADR-0108 D4（action socket）、ADR-0116 D5（launcher 経路）、ADR 2026-10-05-browser-department-web-live-view、実機確認の証跡 `agent-docs/progress/2026-10-05-browser-web-live-view/real-check-evidence/`

## 背景

action socket は `<workspace>/<task>/runs/<run>/browser.action.sock`（session dir の `with_extension("action.sock")`）にあった。Unix socket の `sun_path` は NUL 込みで 108 byte なので bind できる path は 107 byte まで。task id・run id は 26 文字の ULID で、本番の `/local/celeris/data/workspaces` でも 109 byte になり bind が失敗していた（D4）。daemon 経路（`browser.rs`）と launcher 経路（`browser_launcher_run.rs`）が同じ式を別々に持っていた。

## 決定

### D1. 置き場所は `/tmp/celeris-browser-<euid>/<hash 16 桁>.sock`

- hash は session dir（`runs/<run>/browser` や `browser-fallback-N`）の path の sha256 の先頭 16 桁。run と attempt ごとに別の socket になり、同じ session dir なら同じ path。
- 長さは workspace の深さに依らず固定（uid 7 桁でも 50 byte 前後）。
- `/tmp` を選んだのは、harness（shim の利用者）の run 名前空間から見える短い場所であるため。worker の名前空間は `/tmp` を覆わない。launcher は session ごとに `/tmp` を tmpfs で覆うが、それは launcher の session の中の話で、action socket は daemon と harness の間にあるので影響しない。
- 共有の `/tmp` なので dir は 0700 で作り、symlink でない・所有者が自分・group/other の権限が無いことを確かめる。満たさなければ起動前に誤り。
- 同じ名前に前の run の socket が残っていれば消す。socket 以外の物があれば誤り。

### D2. 両経路で共通の関数、bind 前に長さを検査する

`browser_action::action_socket_path(session_dir)`（base を与える `action_socket_path_in` は試験用にも使う）。path が 107 byte を超えたら、path と byte 数と上限を含む `AdapterError::Other` を返し、dir も作らない。

### D3. shim は socket path を config.json から読む

両経路の `config.json` に `action_socket` を書く。`browser_cli.py` はそれを使い、無ければ旧来の `ROOT.with_suffix(".action.sock")` に戻る（古い config・`scripts/tests/test_browser_cli.py` の互換）。

## 同時に直したこと（D6 と launcher の origin 照合）

- launcher 経路の `config.json` に `policy_sha256`（`policy.json` の byte の sha256、daemon 経路と同じ形）を書く。無いと shim の `load_policy` が全 action を拒否していた（実機確認 attempt 8 の `browser policy unavailable`）。`policy.json` は daemon 経路と同じ `PreparedBrowserPolicy::action_policy`（`launch`・`close` を含む）。
- launcher の `action_allowed` は URL の host を `allowed_domains` と文字列比較していたが、daemon が渡すのは正規 origin（`https://host[:port]`、loopback の `http://…`）なので、どの navigate も通らなかった。`browser_policy::url_origin_allowed` で scheme・host・port を照合する。`://` を含まない旧来の素の host は従来の比較を残す。host の launcher の差し替えは人の作業（実機の再確認は Fable）。

## 試験

偽 launcher（`FakeLauncher`）と一時 dir だけで確かめる。userns・実 browser・実 launcher は使わない。

- 本番相当の深い session dir でも 107 byte 以内・固定長・session dir ごとに別・bind できる・残った socket を消して同じ path を使える。
- base が長すぎると path と長さを含む誤りで、何も作らない。dir の mode が緩いと誤り。
- launcher 経路の `config.json`/`policy.json` を実の shim の `load_policy` が受け入れる（python3）。
- 実の shim で許可 origin（`https://example.com`・`http://127.0.0.1:17730`）の open は偽 launcher に `Open` として届き、不許可 origin は shim でも action server でも拒否される。
