# 保存済み browser credential 再利用の再試行

tasks: [01M4KGQG4H9CAR36C0K0D8A1H9]

前回 run の記録。今回の実装と最終検証は [attempt 3](retry-3.md) を参照。

## 状態

- ADR 2026-10-09-browser-credential-username-and-post-login-read.md の付記 2026-10-10j に、owner・site policy・TrustedLogin の完全一致、毎 run の credential_use 承認、失敗 credential の無効化、期限と一覧/削除の要件を記録済み。
- 前回 review の clippy failure を修正: credentiald 一覧を `sort_by_key` に変更。
- credentiald の owner/期限/検索基礎は存在するが、run 時の自動候補照合から approval wait への接続、管理画面の一覧/削除など acceptance 0–2 の実装完了を確認できていない。この task は未完了。

## 検査

- `cargo fmt --all -- --check`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `pnpm -C web lint`: exit 0。reduced-motion の `!important` に lint warning 4 件。
- `pnpm -C web test`: Vitest 91 files / 657 tests pass、Node 92 tests pass。
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 100。5099 run、5064 pass、35 fail、14 ignored。preflight の `userns: false` と診断。35 件は userns/isolated runtime が必要な試験で `Operation not permitted` 等により失敗（ADR-0126 B4）。Doctests pass。

## 次

daemon が task から決める owner を worker に信頼可能に渡す。credential request で credentiald の owner/policy/TrustedLogin 完全一致検索を行い、該当時は metadata のみで WaitingForApproval credential_use wait を開き既存 Authenticate 経路を使う。失敗 credential の無効化、一覧/削除 UI/API、設定可能な保存期限、回帰試験と docs/ops を追加する。最後に userns が使える release host で gate を再実行する。
