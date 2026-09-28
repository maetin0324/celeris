# Browser credential broker（Phase 2）

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

`celeris-credentiald` は [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md) のローカル broker である。起動は `celeris-credentiald serve <control-client-pid>...`。`XDG_RUNTIME_DIR` が所有者の 0700 directory でなければ起動を拒否し、`$XDG_RUNTIME_DIR/celeris-credentiald/{control,resolve}.sock` を 0600 で作る。control は起動時に列挙した PID と process start time に限定し、resolve は同一 UID と短命 binding を要求する。両 socket は一接続につき一つの JSON request/response を扱い、request は write 側を閉じて終える。request は 64 KiB 以下にする。

手動 provider の鍵は `~/.config/celeris/credentiald/keys/master-v1.key`、暗号文は `~/.local/celeris/credentiald/vault/<credential_id>.json`、journal は `~/.local/celeris/credentiald/audit/journal.jsonl` に置く。専用 directory は 0700、ファイルは 0600。control の `initialize_key` は明示的な初回操作であり、暗号文が残る状態の鍵欠落を復旧しない。`register` は `reference`、`policy`、`revision`、`secret: {username,password}` を受け、更新時は ciphertext だけを atomic rename する。`grant` は承認を確認した信頼済み制御側が `LeaseRequest` を送る。broker は approval ID と actor ID を記録するが、承認の真正性は control 側が確定する。`bind` は task/run/session/exact origin/policy hash/expiry を登録し、生成した予測不能な token を返す。`revoke` は未使用 lease を失効させる。

固定 plugin `celeris-credential` は `celeris-credentiald bridge` で動かす。supervisor は binding token を private pipe の FD 3 に渡す。argv・環境変数・plugin JSON に token を置かない。bridge は固定版 agent-browser 0.38.1 の `{protocol:"agent-browser.plugin.v1",type:"credential.resolve",capability:"credential.read",request:{profileName,itemRef,url}}` を受ける。`itemRef` は lease ID、`url` は補助的な origin 照合であり、それ自体を権限証明にしない。成功時だけ stdout の plugin 応答に `{credential:{username,password}}` を返す。失敗時は `success:false` のみ。通常の LLM-facing 出力には `CredentialUseResult {success,failure_code}` を使い、plugin stdout を転送しない。

この crate の origin 照合は、ブラウザの実際の top-level page を観測しない。supervisor と固定版 browser source が注入直前まで origin を検証できなければ、認証利用を開始しない。broker 単体の成功をブラウザへの安全な注入の証明として扱わない。
