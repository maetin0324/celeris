---
title: "launcher 経路 CredentialUse 実装付記"
tasks: [01M4FSDZPA8H9Q70AMAEYVXDPN]
status: done
updated: 2026-10-09
---

# launcher 経路 CredentialUse 実装付記

## 決定

- daemon worker を injection IPC の controller とし、`LiveSessionRegistration` に daemon worker と launcher runtime の PID/starttime を登録する。launcher は `SharedCdp` / `CdpController` を所有する trusted CDP controller として固定認証 verb を実行し、credentiald は `with_launcher_uid` と Attested admission で launcher peer を認証する。
- daemon→launcher は `session_id`、`auth_section_id`、`lease_id`、`origin`、`target` だけを引数に持つ `Authenticate` verb とし、status だけを応答する。credential 値は daemon worker、LLM、一般 action IPC に戻さない。
- `LauncherRuntime::start_guarded` の Started 応答から daemon worker が `launcher_session_proof` を検証する。隔離成功後に `register_live_session`、続けて `attach_launcher_proof` を呼び、`LauncherProofRegistration` に proof と `SCM_CREDENTIALS` 由来 peer UID を結び付ける。credentiald は `admit_attested` / `verify_launcher_session` で実 process facts を再照合する。
- `refuse_confidential` は proof 検証成功・namespace owner が daemon UID と異なる・`isolation_ok` の全条件時だけ CredentialUse を通す。それ以外は接続前に既存文言 `browser credential use is not available through the launcher runtime` で拒否し、fallback しない。IdentityRestore は対象外。
- shim の `credential_use` と `credential_policy_ids` は条件成立時だけ有効値とし、その他は `false` / `[]`。
- `launcher_credential_` 試験で proof 有無/偽造/期限/UID、daemon owner、isolation 不成立、固定 verb、秘密非露出を検査する。credentiald は `injection_ipc.rs`、worker は `browser_launcher_run.rs` / `browser.rs` / `browser_cdp_sink.rs` / `browser_launcher/` を主対象とし、userns 実 process 試験は opt-in。

## 状態

文書のみ変更。実装・実 process 試験・本番設定変更は後続 WorkUnit の範囲。
