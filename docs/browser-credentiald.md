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

## 結線の現状

worker の `request-credential <policy-id> <exact-HTTPS-origin> <purpose>` は policy を再検証し、browser の実行を止めて `WAITING_FOR_AUTH` を作る。手動登録後の次の run は `WAITING_FOR_APPROVAL` を作り、拒否は task を failed にする。broker の登録・lease・plugin bridge は fake substrate と API の試験で通した。承認後の実 agent-browser への credential 注入は、top-level origin を各 fill/submit の直前に確認する実装と plugin 起動契約が未接続のため、worker は明示的に拒否する。fake 試験の success は実ブラウザでの認証成功を意味しない。
