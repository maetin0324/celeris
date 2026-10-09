# browser launcher CredentialUse 解放 ADR 作業

- task: 01M4FRN13Z206RAWBZ0NHZQEGX
- 日付: 2026-10-09
- 状態: ADR 作成完了。実装・台帳・host 実証は後続工程。

## 決定記録

人の決定に基づき [ADR](../../adr/2026-10-09-browser-launcher-credential-release.md) を作成し、ADR-0116・0138・0080 の末尾に相対リンク付き付記を追加した。

- CredentialUse は launcher proof 検証成功、daemon と異なる namespace owner、`isolation_ok` のすべてを満たす session だけ。欠落・失敗は接続前に拒否。
- launcher proof は credentiald injection IPC の `LauncherProofRegistration` として運び、Attested の `admit_attested` による再検証を使う。controller に secret を見せない。
- IdentityRestore は今回は解放しない。
- 台帳 credential evidence は runtime で区別する。launcher が無い host は理由を残して credential backend を空にする。
- 試験接頭辞: `launcher_credential_`、`browser_ledger_launcher_`。
- host 必須モード stutter 3 回と統合後 HEAD の `ADMISSION[real-session]` 再取得は運用セッションで実施する。

## 検査

- `bash scripts/dev/check-adr-numbers.sh` — exit 0
- `bash scripts/dev/check-doc-links.sh` — exit 0
- `bash scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — exit 0

最初に layout checker を引数なしで呼んだため usage で終了した。正しい manifest 引数で再実行して成功した。
