# 保存済み credential 再利用: attempt 3

tasks: [01M4KG0J2XVR99NYH0FA01G9M0]

tested_sha: 363c8a90b6231c511c28ca5f82d6ab67ee0a07b3

## 実装

- daemon の StoreSink が人 `owner` と DB の site policy を決める。agent の申告値では owner を選べない。同じ owner・policy_id・TrustedLogin 全体・有効期限に合う vault 登録だけを検索する。
- 次 run の credential request は有効な保存があれば `WaitingForAuth` を経ず `WaitingForApproval(credential_use)` を開く。毎回承認し、使用前に選択した登録そのものを再照合する。同じ site policy の登録が複数あっても他の登録にすり替えない。
- site policy 変更・describe 失敗・期限切れ・削除は再入力へ戻る。Authenticate 失敗・post-login 保留は保存を削除し、session 終了後に再入力を開く。owner は登録 wait にも保存し、承認・lease 発行でも照合する。
- web `/browser/settings` の保存情報一覧・削除を追加。API は owner session の署名と用途・削除対象・期限を検証し、gateway は CSRF も検証する。秘密・ciphertext は返さない。
- 保存期限は credentiald の `CELERIS_CREDENTIAL_MAX_AGE_DAYS`（1〜90日、既定90日）。旧 owner 無し登録は自動再利用しない。
- [運用手順](../../../docs/ops/browser-saved-credentials.md)。新しい migration・launcher protocol 変更・本番操作はない。daemon・credentiald・web は同じ変更を含む release に更新する。

## 回帰試験

- `saved_credential_reuse_waits_every_run_and_falls_back_without_secrets`: 暗号化 vault → admitted control IPC → shim request → 使用承認 wait。同じ task の次 run、別 owner、policy 変更、認証失敗処理による削除と即時再入力、次回も再利用しないことを検証。
- `saved_credential_cross_task_use_requires_its_own_owner_approval_each_run`: 同じ task の次 run と別 task の承認を SQLite に保存し、未承認・別 owner・二重消費を拒否。
- credentiald: legacy・owner/policy 分離・期限・期限設定範囲・削除・複数登録の参照再照合・別 approver の lease 拒否。
- task-api: owner 署名・用途・対象・期限・owner 別一覧・vault 削除・秘密非露出。web gateway: 本人確認・CSRF・署名・汎用 relay の迂回拒否。UI: policy/期限/削除の表示と秘密非露出。
- `TMPDIR=/tmp cargo nextest run --workspace -E 'test(saved_credential) | test(owned_credential) | test(multiple_saved) | test(registered_credential_describe)'`: 9/9 pass。

## 検証結果

- `cargo check --workspace`: exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `pnpm -C web lint`: exit 0（既存 reduced-motion CSS の warning 4件）。
- `pnpm -C web typecheck`: exit 0。
- `pnpm -C web test`: exit 0。Vitest 92 files / 659 tests、Node 93 tests pass。
- docs の API schema と web/gui の生成型を更新。`node web/scripts/gen-types.mjs --check`: exit 0。gui は pnpm の指定版差と offline tarball 不足のため、既存の json2ts 実行ファイルを読み取り専用で使い、同じ schema と生成引数でこの worktree の型だけを更新した。
- `CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: **exit 0、5109 passed / 0 failed / 14 ignored、userns=true**。nextest 5106件 + doctest 3件。sandbox 外で実行。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: **exit 0、5109 passed / 0 failed / 14 ignored**。nextest 5106件 + doctest 3件。
- `tested_sha` 以後の変更は `agent-docs/progress/` の検証記録のみ。`git diff tested_sha HEAD -- . ":(exclude)agent-docs/progress/**"` が空であることを確認。

途中の検査で通常全体試験に3件失敗（ADR 除外理由の文字列、旧 fixture の別 owner 登録2件）があり修正した。初回の隔離設定付き試験は `userns=true`、5107 pass / 1 fail（doctest込み）。既存 `real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` の拒否 download 後 screenshot が `Target.setDiscoverTargets: cdp_command_failed` で失敗した。同じ隔離設定の単独再試験は1/1 pass。該当機能のコードは変更せず、最終 SHA で全体 gate を再実行し、全件成功した。最初の失敗原因は未確定で、記録から除外しない。

ログは workspace の成果物ディレクトリ `/local/celeris/data/workspaces/01M4KG0J2XVR99NYH0FA01G9M0/artifacts` の `focused-final.log`、`web-tests-final.log`、`clippy-final-sha.log`、`fmt-final-sha.log`、`release-gate-3.log`、`release-artifact-recheck.log`、`release-gate-final.log`、`test-parallel-final.log` に保存した。
