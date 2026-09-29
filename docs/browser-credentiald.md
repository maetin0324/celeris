# Browser credential broker（Phase 2）

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

`celeris-credentiald` は [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md) のローカル broker である。起動は `celeris-credentiald serve <control-client-pid>...`。`XDG_RUNTIME_DIR` が所有者の 0700 directory でなければ起動を拒否し、`$XDG_RUNTIME_DIR/celeris-credentiald/{control,resolve}.sock` を 0600 で作る。control は起動時に列挙した PID と process start time に限定し、resolve は同一 UID と短命 binding を要求する。両 socket は一接続につき一つの JSON request/response を扱い、request は write 側を閉じて終える。request は 64 KiB 以下にする。

手動 provider の鍵は `~/.config/celeris/credentiald/keys/master-v1.key`、暗号文は `~/.local/celeris/credentiald/vault/<credential_id>.json`、journal は `~/.local/celeris/credentiald/audit/journal.jsonl` に置く。専用 directory は 0700、ファイルは 0600。control の `initialize_key` は明示的な初回操作であり、暗号文が残る状態の鍵欠落を復旧しない。`register` は `reference`、`policy`、`revision`、`secret: {username,password}` を受け、更新時は ciphertext だけを atomic rename する。`grant` は承認を確認した信頼済み制御側が `LeaseRequest` を送る。broker は approval ID と actor ID を記録するが、承認の真正性は control 側が確定する。`bind` は task/run/session/exact origin/policy hash/expiry を登録し、生成した予測不能な token を返す。`revoke` は未使用 lease を失効させる。

固定 plugin `celeris-credential` は `celeris-credentiald bridge` で動かす。supervisor は binding token を private pipe の FD 3 に渡す。argv・環境変数・plugin JSON に token を置かない。bridge は固定版 agent-browser 0.38.1 の `{protocol:"agent-browser.plugin.v1",type:"credential.resolve",capability:"credential.read",request:{profileName,itemRef,url}}` を受ける。`itemRef` は lease ID、`url` は補助的な origin 照合であり、それ自体を権限証明にしない。成功時だけ stdout の plugin 応答に `{credential:{username,password}}` を返す。失敗時は `success:false` のみ。通常の LLM-facing 出力には `CredentialUseResult {success,failure_code}` を使い、plugin stdout を転送しない。

この crate の origin 照合は、ブラウザの実際の top-level page を観測しない。supervisor と固定版 browser source が注入直前まで origin を検証できなければ、認証利用を開始しない。broker 単体の成功をブラウザへの安全な注入の証明として扱わない。

## 起動設定

手動鍵は初回に `celeris-credentiald init` で明示的に作成する。既存 vault がある状態で鍵が失われた場合、このコマンドは鍵を再生成せず停止する。credentiald は daemon と同じ user の systemd user service として起動する。例は [`deploy/systemd/celeris-credentiald@.service`](../deploy/systemd/celeris-credentiald@.service) にあり、`celeris@<release>` の MainPID を control 許可リストに渡す。daemon が入れ替わったら broker も同じ release 名で再起動し、旧 lease は失効する。

`config.toml` の `[api]` に次を追加する。片方だけの設定は API 起動エラーにする。公開鍵と socket は絶対パス、または config ファイルからの相対パスを指定する。

```toml
[api]
browser_attestation_public_key_file = "/home/USER/.config/celeris/browser-attestation.pub"
browser_credentiald_control_socket = "/run/user/UID/celeris-credentiald/control.sock"
```

`PUT /api/v1/tasks/{id}/browser/policy` で管理者が task policy を登録し、`GET` で確認する。登録前の browser task は起動を拒否する。変更は task が draft/ready のときだけ許可する。credentiald の socket が無い場合、手動登録は 503 で止まり、DB に秘密を書かない。

`[api]` の二項目を設定すると、daemon は起動時に同じ control socket を browser supervisor にも渡す（`task_worker::browser_credential::configure`）。plugin bridge は daemon と同じ release の `bin/celeris-credentiald`（`release.sh` が bundle に入れる）を使い、bridge が接続する resolve socket は control socket の二つ上の directory（`XDG_RUNTIME_DIR` 相当）から決める。未設定なら承認済みの credential 使用は `approved browser credential use denied` で止まる。

systemd user unit の順序は `celeris@<release>` → `celeris-credentiald@<release>`。初回だけ次を行う。

```sh
~/.local/celeris/current/bin/celeris-credentiald init
systemctl --user enable --now celeris-credentiald@<release>
```

## 結線の現状

1. worker の `request-credential <policy-id> <exact-HTTPS-origin> <purpose>` は policy を再検証し、browser を閉じて `WAITING_FOR_AUTH`（task は Blocked）を作る。登録依頼は `GET /api/v1/browser/waits` に出る。
2. 人が API/GUI で登録すると秘密は control socket 経由で broker にだけ渡り、task は Ready に戻る。次の run は browser を起動せずに `WAITING_FOR_APPROVAL` を作る。session ID はこの時点で予約される。
3. 拒否すると task は Failed になり、lease は発行されない。
4. 承認後の run では supervisor が承認を一度だけ消費する（`consume_credential_approval`）。照合には承認 wait を開いた論理 run/session を使う。dispatch ごとの run ID は変わるからである。その後 broker に bind と grant を求める。lease は 60 秒・一回限りで、policy hash は `sha256:` を除いた digest で broker ID に写す。
5. supervisor だけが credential 区間の policy（`auth_login`・`close`・`launch`・`navigate`・`url`・`plugin:celeris-credential:credential.read`）と plugin 設定で substrate を操作する。`open <origin>/` → `get url` で top-level origin を照合 → `auth login celeris-credential --credential-provider celeris-credential --item <lease> --no-navigate --url <origin>/` → `get url` で再照合、の順に進む。binding token は session daemon を起動する `open` の FD 3 に渡し、plugin がそこから読む。login 後は同じ `policy.json` の内容だけを harness policy に置き換える。config と policy のパスは session 中に変えない。origin が一致しない、または失敗した場合は lease を失効させ、task を再試行なしの Error にする。
6. 成功後の harness は、観測系（snapshot・gettext・screenshot・download）を外した policy で同じ session を続ける。Live View の URL は出さない。モデルには `result: success` だけを伝える。

試験は `crates/task-api/tests/browser_e2e.rs` にある。store・API・broker IPC・`celeris-credentiald bridge` は本物を使い、ブラウザだけを fake substrate（`tests/fixtures/fake-agent-browser.py`）と fixture site に置き換える。扱う場面は「未登録→待ち→登録→再開→success」「承認拒否→failed」「login URL が別 origin へ redirect→deny（auth login は走らない）」の三つで、それぞれ tempdir 全ファイル（DB・WAL・workspace・runs・artifacts・vault・journal）、task の events、worker 出力を sentinel で全走査する。

**fake と実機の区別**: API の端から端までの試験は fake substrate を使う。別の `scripts/browser-auth-login-check.py` は、worker が生成した設定・policy・argv と実 agent-browser 0.38.1 / Chromium を使い、scratch broker と loopback HTTPS fixture で login 成功を確認する。`wu/live-gui/artifacts/auth-wiring/production5/` には一回 lease の再使用拒否（broker journal の `used`）と sentinel 0 件も残した。GUI 登録→再開と Live View の実ブラウザ証跡は `wu/live-gui/artifacts/g14-final/` にある。これらは別々の試験であり、LLM harness を含む単一の実環境走行ではない。

固定版 0.38.1 では plugin 設定は `plugins` 配列で、`auth login` は positional name と `--item` を使う。plugin は session daemon から起動されるため、binding token を daemon の起点となる `open` の FD 3 に渡す。`get url` は内部 action `url` を要する。plugin 要求の `url` は CLI の `--url` 値なので、supervisor はその前後に top-level URL を別途照合する。
