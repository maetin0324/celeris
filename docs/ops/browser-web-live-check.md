---
tasks: [01M46W97H391DSFW1XJ745W0G9, 01M47QXZR0QMCYZM9KAZC81BCD]
---

# Browser web Live View 実機確認

`scripts/dev/browser-web-live-check.sh` は、別 UID の launcher、使い捨て DB の daemon、loopback の許可・不許可の試験ページ、Live View の upstream 代役（fixture）、web gateway を起動する host 専用の opt-in 台本である。worker sandbox は user namespace を作れないため、配送後に Fable が隔離できる host で実行し、`checks.json` とログをこの進捗に転記する。未 opt-in では副作用なしで exit 2。

## 前提と隔離

- 台本は**試験 daemon の UID**で実行する。launcher は別の `celeris-browser` UID で、`sudo -n -u` だけを許可する。既存 systemd launcher や本番 DB は使わない。
- `CELERIS_BROWSER_EVIDENCE_DIR` は絶対パスの専用ディレクトリにする（既定 `/var/tmp/cb-<UID>`）。daemon の `db`、`workspace_root`、launcher の `socket`・`state_dir`・`session_root`、web の owner socket はすべてこの下の絶対パスとする。台本は開始前にこれを検査し、既存 socket があれば拒否する。
- **`/tmp` の下は使わない。** launcher は session ごとに `/tmp` を tmpfs で覆うため、`/tmp` の下の launcher state・socket・bwrap 等は session の中から見えない（attempt 4 の bwrap ENOENT）。台本は証跡ディレクトリ、launcher config とその親、daemon config、上の各パス、launcher の `bwrap`/`sandboxd`/`egress`/`chrome`/`agent_browser` のどれかが `/tmp` の下（symlink 解決後）なら開始前に拒否する。既定はすべて `/var/tmp` の下。
- 証跡ディレクトリ名は短くする（例 `/var/tmp/cb-1001`）。run の workspace はこの下にでき、action socket の path が Unix socket の 107 byte 上限を超えると run が `isolated_runtime_unavailable` で止まる（attempt 5。launcher 経路の socket path の修正（D4）が入っていない build では特に）。ルートは launcher が通れるよう 0711、ログは umask 077 で 0600。web owner socket と秘密鍵は各 0700 の私有サブディレクトリに置く。
- `CELERIS_BROWSER_TEST_LAUNCHER_CONFIG` は root 所有で group/other 書込不可の TOML。親ディレクトリも root 所有で group/other 書込不可にする（既定 `/var/tmp/celeris-browser-config-<UID>/launcher.toml`、ディレクトリは 0755）。書式は [launcher 配置手順](browser-launcher-host-setup.md) §4。`allowed_uids` に試験 daemon UID を入れ、`socket` と `state_dir` は証跡ディレクトリ内へ向ける。`test_loopback_allow` に許可ページの `127.0.0.1:<PAGE_PORT>` を必ず設定する。省略時は off なので台本は設定漏れを launcher 起動前に拒否する。本番 path/config/DB/socket では許可を有効化できない。bwrap、sandboxd、egress、Chrome、agent-browser の実体と subuid/subgid を用意する。launcher UID が `state_dir` を所有する。台本は `sudo -n` で一時 socket を daemon UID 所有の 0600 で作り、fd 3 を launcher に渡す（systemd socket activation と同じ契約）。socket がすでに fd 3 のとき `os.dup2(fd, 3, inheritable=True)` は何もせず `O_CLOEXEC` が残るので、続けて `os.set_inheritable(3, True)` で継承可にする（attempt 1 の launcher abort）。
- `CELERIS_BROWSER_TEST_DAEMON_CONFIG` は使い捨て DB・workspace、`[browser] runtime="launcher"` と同じ socket、loopback の `[api] listen`・`token_file`、`org_include`（`engineering` node を含む）、`browser-specialist` または ACP の**ローカル試験 harness**と provider を設定する。`CELERIS_BROWSER_CONFORMANCE_FILE` には対象 backend の実測済み台帳を指定する。試験 harness は run を少なくとも 3 分維持し、外部 URL を開かず、許可ページを表示して範囲外 origin `http://127.0.0.1:17731` への遷移拒否を行う。拒否証跡は harness が作らず、launcher の session 記録から台本が採取する。外部ネットワークに出る provider は使わない。
- gateway の Ed25519 秘密鍵 `CELERIS_WEB_ATTESTATION_KEY_FILE` と daemon の `[api] browser_attestation_public_key_file` は同じ鍵ペアを使う。daemon には `[api] browser_credentiald_control_socket` も試験ディレクトリ内で指定する。web password file と owner socket の親は試験 daemon UID 所有で 0700。`web/` の依存と `dist/` を先に用意する。
- `CELERIS_BROWSER_TEST_DAEMON_BIN` と `CELERIS_BROWSER_TEST_LAUNCHER_BIN` は検査したビルドの実行ファイル。試験用 socket を root で作って launcher UID に切り替えるため、当該 Python ラッパーを `sudo -n` で起動できる権限が必要。試験用の未使用 API/web/page port を確保する。既定の web/許可ページ/不許可ページ/Live View fixture port は 17729/17730/17731/17732 で、`CELERIS_BROWSER_TEST_{WEB,PAGE,DENIED,LIVE}_PORT` で変更できる。5 つの port（API を含む）は互いに異なり、7700/7710 は拒否する。

設定例（実際の絶対パス・UID・ツールを置換する）:

```toml
# launcher.toml（root 所有の別ディレクトリに配置）
# /tmp の下は不可（launcher が session ごとに /tmp を tmpfs で覆う）
socket = "/var/tmp/cb-1001/launcher.sock"
state_dir = "/var/tmp/cb-1001/launcher-state"
session_root = "/var/tmp/cb-1001/launcher-state/sessions"
allowed_uids = [1001]
bwrap = "/usr/bin/bwrap"
sandboxd = "/usr/local/libexec/celeris/celeris-browser-sandboxd"
egress = "/usr/local/libexec/celeris/celeris-browser-egress"
chrome = "/usr/bin/google-chrome"
agent_browser = "/usr/local/libexec/celeris/agent-browser"
resolver = "127.0.0.1"
test_loopback_allow = ["127.0.0.1:17730"] # 必ず試験 PAGE_PORT と一致させる
```

```toml
# daemon.toml（必須部分。org と provider/harness 設定を追加する）
db = "/var/tmp/cb-1001/test.sqlite3"
workspace_root = "/var/tmp/cb-1001/workspaces"
org_include = "/var/tmp/cb-1001/org.toml"
tick_ms = 200

[api]
listen = "127.0.0.1:17728"
token_file = "/var/tmp/cb-1001/api.token"
browser_attestation_public_key_file = "/var/tmp/cb-1001/attestation.pub"
browser_credentiald_control_socket = "/var/tmp/cb-1001/credentiald/control.sock"

[browser]
runtime = "launcher"
launcher_socket = "/var/tmp/cb-1001/launcher.sock"
```

試験用 `org.toml` は `config/org.example.toml` から作る。DB が空なら seed され、台本は browser-execution が既存なら PATCH、なければ `docs/ops/browser-department-org.json` を POST する。いずれも grant は試験ページの origin 1 件に置換する。

task の作成後、台本は `PUT /api/v1/tasks/{id}/browser/policy` で task の browser policy（`policy_id` `real-check-loopback`、`domain_mode` `common_hosts`、`network_domains` は許可ページの origin 1 件、`allowed_actions` は navigate/snapshot/extract/click/scroll）を保存し、`GET` で読み戻す。policy の無い task は dispatch が `browser_policy_required` で browser run を始めない（attempt 3）。

## 実行

```bash
export CELERIS_BROWSER_REAL_CHECK=1 CELERIS_USERNS_TESTS=1
export CELERIS_BROWSER_EVIDENCE_DIR=/var/tmp/cb-1001
export CELERIS_BROWSER_TEST_LAUNCHER_CONFIG=/var/tmp/celeris-browser-config-1001/launcher.toml
export CELERIS_BROWSER_TEST_DAEMON_CONFIG="$CELERIS_BROWSER_EVIDENCE_DIR/daemon.toml"
export CELERIS_BROWSER_TEST_LAUNCHER_BIN=/absolute/path/to/celeris-browser-launcher
export CELERIS_BROWSER_TEST_DAEMON_BIN=/absolute/path/to/celeris
export CELERIS_BROWSER_TEST_LAUNCHER_USER=celeris-browser
export CELERIS_BROWSER_TEST_PAGE_PORT=17730
export CELERIS_BROWSER_CONFORMANCE_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/conformance.json"
export CELERIS_WEB_PASSWORD_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/web.password"
export CELERIS_WEB_ATTESTATION_KEY_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/web-private/attestation.key"
export CELERIS_WEB_OWNER_SOCKET="$CELERIS_BROWSER_EVIDENCE_DIR/web-private/owner.sock"
# 任意: export CELERIS_BROWSER_TEST_DENIAL_FILE="$CELERIS_BROWSER_EVIDENCE_DIR/egress-denied.json"
# 既定と違う場合だけ: CELERIS_BROWSER_TEST_DECISION_ORIGIN=https://real-check.celeris.invalid
bash scripts/dev/browser-web-live-check.sh
```

上の `export` のうち launcher/daemon の実行ファイル以外は、値が既定（証跡ディレクトリ `/var/tmp/cb-<UID>` とその下の同名ファイル、launcher config `/var/tmp/celeris-browser-config-<UID>/launcher.toml`、launcher user `celeris-browser`）と同じなら省略できる。

台本は次を順に検査する。どの段階でも不合格なら exit 1。

1. `GET /health` と `/healthz`、org 投入、web login。
2. **設定の編集**: web gateway 経由で `PATCH /api/v1/org/browser-execution/browser-settings`（web の設定画面と同じ経路）を送り、`allowed_domains` を許可・不許可の 2 origin に広げて応答に反映されること、不正な origin（`javascript:alert(1)`）が 422 で拒否されること、許可ページ 1 件に戻して `GET /api/v1/org` の保存値が戻っていることを確かめる。
3. `requirements.browser.allowed_domains` が許可ページだけの task 作成と task browser policy の `PUT`/`GET`（上記）。
4. owner 承認、`/browser/runs` に RUNNING の run が出ること（`live_path` が同一 origin の `/browser/live/{task}/{run}` で、生の `live_view_url` を返さないこと）。
5. **未認証の Live View の拒否**: cookie を持たない client で `live_path` の GET と stream の WebSocket upgrade（`{live_path}/api/session/9222/stream?last_seen=0`）がどちらも 401 になること。そのあと owner session で `live_path` を開く。
6. **lease 無しの入力の拒否**: owner session で stream に upgrade し、control lease を持たないまま `input_mouse` の frame を送る。gateway が `{"type":"input_denied","code":"lease_required"}` を返し、Live View fixture（upstream の代役）が受けた frame に `input_mouse` が増えないことを確かめる。agent 実行中と lease 返却後の 2 回行う。
7. control: 状態取得 → **`pause`**（`pausing` なら `paused` に収束するまで待つ）→ `takeover`（`human_control` になること）→ 返却。takeover は `paused` からしか通らない（ControlPhase。attempt 6 の 409 `invalid_phase`）。
8. 実 run に紐付けた decision wait の作成・一覧・web からの deny。wait の `origin` は task-core の `valid_exact_origin` が要求する正規形の **https** origin（`https://host[:port]`、path なし、`:443` の明示なし）でなければならず、http の loopback 試験ページは使えない（attempt 7 の 422 `origin`）。decision 経路は信頼された API の判断だけで、その origin へは誰も遷移しないので、既定は解決しない `https://real-check.celeris.invalid` とする（`CELERIS_BROWSER_TEST_DECISION_ORIGIN` で変更可、台本が形を検査する）。loopback の http 試験ページは egress の確認専用である。範囲外 origin への**実ブラウザ遷移**の拒否は launcher の session 記録と不許可ページにアクセスがないことで確認する。

## 証跡

`CELERIS_BROWSER_EVIDENCE_DIR` には `checks.json`（各 API の期待/実 status、task/run/wait ID、保存された task policy、設定編集の結果、lease 無し入力の応答と upstream に届いた input frame 数、decision origin）、`health.json`、`web-health.json`、`launcher.log`、`daemon.log`、`web.log`、`page.log`、`denied-page.log`、`live-upstream.log`、`live-upstream-frames.jsonl`、`egress-denied.json` が残る。`egress-denied.json` は launcher の `<state_dir>/sessions/<session_id>/egress-denied.jsonl` から収集し、`kind=private_address`、`host=127.0.0.1`、`port=<DENIED_PORT>`、UTC 時刻、session id を検査する。`checks.json` に `result: passed` があり、`denied-page.log` に GET がないことを確認する。ログは秘密情報がないことを確認してから進捗へ必要最小限を転記する。台本は trap で起動したプロセス群を停止する。
