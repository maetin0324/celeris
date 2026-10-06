---
tasks: [01M46W97H391DSFW1XJ745W0G9]
---

# Browser web Live View 実機確認

`scripts/dev/browser-web-live-check.sh` は、別 UID の launcher、使い捨て DB の daemon、loopback の許可・不許可の試験ページ、web gateway を起動する host 専用の opt-in 台本である。worker sandbox は user namespace を作れないため、配送後に Fable が隔離できる host で実行し、`checks.json` とログをこの進捗に転記する。未 opt-in では副作用なしで exit 2。

## 前提と隔離

- 台本は**試験 daemon の UID**で実行する。launcher は別の `celeris-browser` UID で、`sudo -n -u` だけを許可する。既存 systemd launcher や本番 DB は使わない。
- `CELERIS_BROWSER_EVIDENCE_DIR` は絶対パスの専用ディレクトリにする。daemon の `db`、`workspace_root`、launcher の `socket` と `state_dir`、web の owner socket はすべてこの下の絶対パスとする。台本は開始前にこれを検査し、既存 socket があれば拒否する。ルートは launcher が通れるよう 0711、ログは umask 077 で 0600。web owner socket と秘密鍵は各 0700 の私有サブディレクトリに置く。
- `CELERIS_BROWSER_TEST_LAUNCHER_CONFIG` は root 所有で group/other 書込不可の TOML。親ディレクトリも root 所有で group/other 書込不可にする（例: `/tmp/celeris-browser-config-XXXX/` を 0755）。書式は [launcher 配置手順](browser-launcher-host-setup.md) §4。`allowed_uids` に試験 daemon UID を入れ、`socket` と `state_dir` は証跡ディレクトリ内へ向ける。bwrap、sandboxd、egress、Chrome、agent-browser の実体と subuid/subgid を用意する。launcher UID が `state_dir` を所有する。台本は `sudo -n` で一時 socket を daemon UID 所有の 0600 で作り、fd 3 を launcher に渡す（systemd socket activation と同じ契約）。
- `CELERIS_BROWSER_TEST_DAEMON_CONFIG` は使い捨て DB・workspace、`[browser] runtime="launcher"` と同じ socket、loopback の `[api] listen`・`token_file`、`org_include`（`engineering` node を含む）、`browser-specialist` または ACP の**ローカル試験 harness**と provider を設定する。`CELERIS_BROWSER_CONFORMANCE_FILE` には対象 backend の実測済み台帳を指定する。試験 harness は run を少なくとも 3 分維持し、外部 URL を開かず、許可ページを表示して範囲外 origin `http://127.0.0.1:17731` への遷移拒否を `CELERIS_BROWSER_TEST_DENIAL_FILE` に記録する。外部ネットワークに出る provider は使わない。
- gateway の Ed25519 秘密鍵 `CELERIS_WEB_ATTESTATION_KEY_FILE` と daemon の `[api] browser_attestation_public_key_file` は同じ鍵ペアを使う。daemon には `[api] browser_credentiald_control_socket` も試験ディレクトリ内で指定する。web password file と owner socket の親は試験 daemon UID 所有で 0700。`web/` の依存と `dist/` を先に用意する。
- `CELERIS_BROWSER_TEST_DAEMON_BIN` と `CELERIS_BROWSER_TEST_LAUNCHER_BIN` は検査したビルドの実行ファイル。試験用 socket を root で作って launcher UID に切り替えるため、当該 Python ラッパーを `sudo -n` で起動できる権限が必要。試験用の未使用 API/web/page port を確保する。既定の web/許可ページ/不許可ページ port は 17729/17730/17731 で、環境変数で変更できる。

設定例（実際の絶対パス・UID・ツールを置換する）:

```toml
# launcher.toml（root 所有の別ディレクトリに配置）
socket = "/tmp/celeris-browser-check-XXXX/launcher.sock"
state_dir = "/tmp/celeris-browser-check-XXXX/launcher-state"
session_root = "/tmp/celeris-browser-check-XXXX/launcher-state/sessions"
allowed_uids = [1001]
bwrap = "/usr/bin/bwrap"
sandboxd = "/usr/local/libexec/celeris/celeris-browser-sandboxd"
egress = "/usr/local/libexec/celeris/celeris-browser-egress"
chrome = "/usr/bin/google-chrome"
agent_browser = "/usr/local/libexec/celeris/agent-browser"
resolver = "127.0.0.1"
```

```toml
# daemon.toml（必須部分。org と provider/harness 設定を追加する）
db = "/tmp/celeris-browser-check-XXXX/test.sqlite3"
workspace_root = "/tmp/celeris-browser-check-XXXX/workspaces"
org_include = "/tmp/celeris-browser-check-XXXX/org.toml"
tick_ms = 200

[api]
listen = "127.0.0.1:17728"
token_file = "/tmp/celeris-browser-check-XXXX/api.token"
browser_attestation_public_key_file = "/tmp/celeris-browser-check-XXXX/attestation.pub"
browser_credentiald_control_socket = "/tmp/celeris-browser-check-XXXX/credentiald/control.sock"

[browser]
runtime = "launcher"
launcher_socket = "/tmp/celeris-browser-check-XXXX/launcher.sock"
```

試験用 `org.toml` は `config/org.example.toml` から作る。DB が空なら seed され、台本は browser-execution が既存なら PATCH、なければ `docs/ops/browser-department-org.json` を POST する。いずれも grant は試験ページの origin 1 件に置換する。

## 実行

```bash
export CELERIS_BROWSER_REAL_CHECK=1 CELERIS_USERNS_TESTS=1
export CELERIS_BROWSER_EVIDENCE_DIR=/tmp/celeris-browser-check-XXXX
export CELERIS_BROWSER_TEST_LAUNCHER_CONFIG=/tmp/celeris-browser-config-XXXX/launcher.toml
export CELERIS_BROWSER_TEST_DAEMON_CONFIG="$CELERIS_BROWSER_EVIDENCE_DIR/daemon.toml"
export CELERIS_BROWSER_TEST_LAUNCHER_BIN=/absolute/path/to/celeris-browser-launcher
export CELERIS_BROWSER_TEST_DAEMON_BIN=/absolute/path/to/celeris
export CELERIS_BROWSER_TEST_LAUNCHER_USER=celeris-browser
export CELERIS_BROWSER_CONFORMANCE_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/conformance.json"
export CELERIS_WEB_PASSWORD_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/web.password"
export CELERIS_WEB_ATTESTATION_KEY_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/web-private/attestation.key"
export CELERIS_WEB_OWNER_SOCKET="$CELERIS_BROWSER_EVIDENCE_DIR/web-private/owner.sock"
export CELERIS_BROWSER_TEST_DENIAL_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/egress-denied.json"
bash scripts/dev/browser-web-live-check.sh
```

台本は `GET /health` と `/healthz`、org 投入、`requirements.browser.allowed_domains` が許可ページだけの task 作成、`/browser/runs`、同一 origin `/browser/live/{task}/{run}`、owner 承認、control lease の取得と返却、実 run に紐付けた decision wait の作成・web からの deny を検査する。どの段階でも不合格なら exit 1。範囲外 origin への**実ブラウザ遷移**の拒否は harness 側の実行記録と不許可ページにアクセスがないことの両方で確認し、`checks.json` の API 結果だけをその証拠にしない。harness の記録は `{"allowed_origin":"http://127.0.0.1:17730","attempted_origin":"http://127.0.0.1:17731","denied":true,"source":"agent-browser"}` の厳密な JSON とし、port を変更した場合は両 origin も合わせる。

## 証跡

`CELERIS_BROWSER_EVIDENCE_DIR` には `checks.json`（各 API の期待/実 status と task/run/wait ID）、`health.json`、`web-health.json`、`launcher.log`、`daemon.log`、`web.log`、`page.log`、`denied-page.log`、`egress-denied.json` が残る。`checks.json` に `result: passed` があり、harness の run log に許可ページへの遷移成功と範囲外 origin への拒否が記録され、`denied-page.log` に GET がないことを確認する。ログは秘密情報がないことを確認してから進捗へ必要最小限を転記する。台本は trap で起動したプロセス群を停止する。
