# Browser credential broker

---
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
---

`celeris-credentiald` は browser task の資格情報を daemon と worker の外で保管する。手動登録の鍵は `~/.config/celeris/credentiald/keys/master-v1.key`、暗号文は `~/.local/celeris/credentiald/vault/` に置く。初期化は `celeris-credentiald init`。既存 vault の鍵を失った場合は再生成しない。

## 起動と設定

broker は daemon と同じ OS ユーザーの systemd user service で起動する。[unit 例](../../deploy/systemd/celeris-credentiald@.service) は daemon の MainPID を control 許可リストに渡す。`XDG_RUNTIME_DIR` は本人所有の 0700 directory が必要。socket は `$XDG_RUNTIME_DIR/celeris-credentiald/{control,resolve}.sock` に 0600 で作られる。

```toml
[api]
browser_attestation_public_key_file = "/home/USER/.config/celeris/browser-attestation.pub"
browser_credentiald_control_socket = "/run/user/UID/celeris-credentiald/control.sock"
```

両項目を設定する。task の browser policy は `PUT /api/v1/tasks/{id}/browser/policy` で登録し、`GET` で確認できる。登録前の browser task は起動しない。credentiald に接続できない場合は手動登録を拒否し、秘密を DB に書かない。

## 認証利用の条件

直接の `resolve.sock` 呼び出しと旧 `celeris-credentiald bridge` は秘密を返さない。認証利用は isolated runtime、承認済みの wait、信頼できる selector、実基盤の conformance record がそろった H3 経路だけで行う。`CredentialUse` を含む policy は適合する backend が無ければ起動前に拒否する。H3 は agent の CDP 操作と Live View を止め、broker の lease を用いて controller が値を注入する。認証区間で用いた値は閉じる前に消去する。

登録・承認はブラウザへの注入を許す操作なので、task policy と実行中の browser session の対応を確認する。詳細な境界は [ADR-0080](../../agent-docs/adr/0080-browser-phase2-policy-broker-approval.md) と [ADR-0103](../../agent-docs/adr/0103-browser-phase4-runtime-selection.md) を参照。
