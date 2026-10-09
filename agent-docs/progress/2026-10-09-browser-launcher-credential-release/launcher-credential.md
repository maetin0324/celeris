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

## credentiald WorkUnit 実装

- credentiald は既存の `Admission::Attested` で session に結び付いた `LauncherProofRegistration` を毎回読み、実 runtime facts と `verify_launcher_session` で照合する。証明欠落、launcher UID 未設定、peer/config/proof UID 不一致、owner 不一致、isolation 不成立、process 検査失敗は `InjectCode::IsolationRequired` に fail closed する。daemon UID の runtime は同一 UID 条件で拒否する。
- 登録 IPC の `LauncherProofRegistration` は session/instance/peer UID/proof のみを持ち、credential 値を含めない。credential 値は従来どおり注入 sink に限定され、試験で daemon worker 等へ返らないことを確認する。
- `crates/celeris-credentiald/tests/prod_admission.rs` に `launcher_credential_` の許可・証明なし・偽造 instance・stale process（starttime 不一致）・peer UID 不一致の 5 ケースを追加した。proof 型に壁時計の期限フィールドは無いため、失効した process 証明は PID/starttime の照合で検査する。
- 検査: `TMPDIR=/tmp/cd-lc cargo test -p celeris-credentiald` は全 suite 成功。既定の長い run TMPDIR では既存 broker fixture 3 件が `SUN_LEN`/readiness timeout で失敗したため、短い物理 TMPDIR で再実行した。`TMPDIR=/tmp/cd-lc cargo clippy -p celeris-credentiald -- -D warnings` 成功。
