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
- run-gate attempt 1: admission helper と shim flag の条件分岐を追加。`cargo test -p task-worker --lib launcher_credential_` は 12 件成功・1 件失敗（launcher authenticate server fixture が長い TMPDIR 配下の Unix socket bind で `SUN_LEN` 超過）。ただし run 経路は isolation attestation を helper に渡さず `true` 固定で、proof attach と Authenticate 呼び出しも未配線。統合は未完了。
- `launcher_credential_` 試験で proof 有無/偽造/期限/UID、daemon owner、isolation 不成立、固定 verb、秘密非露出を検査する。credentiald は `injection_ipc.rs`、worker は `browser_launcher_run.rs` / `browser.rs` / `browser_cdp_sink.rs` / `browser_launcher/` を主対象とし、userns 実 process 試験は opt-in。

## 状態

文書のみ変更。実装・実 process 試験・本番設定変更は後続 WorkUnit の範囲。

## credentiald WorkUnit 実装

- credentiald は既存の `Admission::Attested` で session に結び付いた `LauncherProofRegistration` を毎回読み、実 runtime facts と `verify_launcher_session` で照合する。証明欠落、launcher UID 未設定、peer/config/proof UID 不一致、owner 不一致、isolation 不成立、process 検査失敗は `InjectCode::IsolationRequired` に fail closed する。daemon UID の runtime は同一 UID 条件で拒否する。
- 登録 IPC の `LauncherProofRegistration` は session/instance/peer UID/proof のみを持ち、credential 値を含めない。credential 値は従来どおり注入 sink に限定され、試験で daemon worker 等へ返らないことを確認する。
- `crates/celeris-credentiald/tests/prod_admission.rs` に `launcher_credential_` の許可・証明なし・偽造 instance・stale process（starttime 不一致）・peer UID 不一致の 5 ケースを追加した。proof 型に壁時計の期限フィールドは無いため、失効した process 証明は PID/starttime の照合で検査する。
- 検査: `TMPDIR=/tmp/cd-lc cargo test -p celeris-credentiald` は全 suite 成功。既定の長い run TMPDIR では既存 broker fixture 3 件が `SUN_LEN`/readiness timeout で失敗したため、短い物理 TMPDIR で再実行した。`TMPDIR=/tmp/cd-lc cargo clippy -p celeris-credentiald -- -D warnings` 成功。

## launcher-inject WorkUnit（2026-10-09）

- protocol に secret-free `AuthenticateArgs`（session/auth section/lease/origin/target）と status-only `AuthenticateResult` を定義し、未知 field を拒否する。
- server は接続 owner・lease・isolation を検査して backend authenticate を dispatch する。失敗時も固定 `rejected` status のみを返す。daemon client に authenticate 関数を追加。
- 現時点で `RuntimeSession.authenticate` は未接続のため拒否する。credentiald socket の設定と launcher-owned `CdpController` からの broker injection、auth section open/close・値 cleanup は未実装であり、成功経路はまだ解放していない。
- `launcher_credential_` の試験 3 件で固定引数/未知 secret field 拒否・必須引数検査・fake backend の拒否 status を確認する。`TMPDIR=/tmp/lci cargo test -p task-worker --lib browser_launcher` は 37 件成功、`TMPDIR=/tmp/lci cargo clippy -p task-worker --lib -- -D warnings` 成功。長い run TMPDIR では Unix socket path が SUN_LEN を超えるため、短い物理 TMPDIR を使った。
- 残作業: `RuntimeSession.authenticate` は fail-closed の拒否実装のまま。credentiald injection socket と launcher-owned `CdpController` をつなぐ実行経路、auth section open/close、注入値 cleanup は未実装で、この WorkUnit の launcher runtime では実際の認証注入はまだできない。

## run-gate retry (2026-10-09)

- 前回 check の原因は `launcher_credential_` 試験が 3 件で、計画の下限 10 件を満たさなかったこと。条件判定を独立させ、証明欠落・証明内 isolation 偽・daemon owner・daemon 側 isolation 失敗・namespace binding 欠落・request なし・UID 照合、shim config を検査する試験を追加した。計 13 件が列挙される。
- shim は proof と daemon の `verify_isolation` が返した attestation の両方を通った run のみ credential フラグと policy ID を受け取る。credential wait 再開も要求判定に含める。秘密値は run context に設定せず、harness へ status のみ渡す。
- 検証: `TMPDIR=/tmp/lc cargo test -p task-worker --lib launcher_credential_` は 13 件成功。標準の長い `TMPDIR` では既存 browser launcher socket fixture が `SUN_LEN` を超えた。
- 未解決: `crates/task-worker/src/browser_launcher/backend.rs` の実 backend `authenticate` は依然 `Unauthorized` 固定で、task-worker launcher run から credentiald の `attach_launcher_proof` も呼び出していない。よって条件成立時の実 CredentialUse 注入と broker 秘密非露出の実行経路は未達であり、完了扱いにできない。これらの並行 WorkUnit 所有ファイルは変更していない。
- run-gate 再開確認: `credential_admitted` の isolation 条件を固定 `true` から `start_guarded` の `IsolationAttestation` と launcher runtime session ID の一致へ変更。`TMPDIR=/tmp/lc-r3 cargo test -p task-worker --lib launcher_credential_` は13件成功、`cargo clippy -p task-worker --lib -- -D warnings` 成功。`cargo test -p task-worker --lib browser_launcher_run` は filter 名不一致で0件実行（exit 0）だったため、有効な接頭辞 filter で再実行した。
- 残作業: `browser_launcher_run::run` に credential supervisor が渡らず、approved credential wait の消費・lease/section の発行・live session 登録後の proof attach・launcher Authenticate を結ぶ配線は未実装。`launcher-auth` 担当の backend authenticate は変更していない。接続/section target は既存 `inject_h3` が CDP から作るため、launcher の controller を介した同等処理が必要。
- run-gate 実装継続: 条件成立後に credentiald live session 登録→launcher proof attach を行い、承認済み credential wait を消費して secret-free `AuthenticateArgs` を launcher へ送る。status のみ `BrowserContext.credential_used` に反映。認証処理失敗・拒否は固定の従来文言で終了し、shim config は admission 成立後のみ credential 有効化する。fake launcher/backend が Authenticate 成功 status を返す試験を追加。
