---
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
completed: 2026-10-06
---

# Browser web 実機台本修正の既存 branch 取り込み（script-fix2）

前 run（script-fix）が記録前に failed になったため、作業済み branch `celeris-wu/01M46W97H391DSFW1XJ745W0G9/script-fix` の tip を作り直さず取り込んだ。base `2f30f62b` から `git merge --ff-only 2784084b` でき、fast-forward で統合した。衝突はなかった。

## 取り込んだ sha

- `2784084b` ブラウザ実機台本の設定編集経路を web と揃える（R1）
- `5ad7ad63` ブラウザ実機台本の再実行と操作検査を補う（R3・R4・カバー欠け）

## R1・R3・R4・カバー欠けの対応箇所

- R1（settings URL）: 台本は web 画面と同じ `web+"/api/org/browser-execution/browser-settings"` を使う（`scripts/dev/browser-web-live-check.sh:390`。gateway が `/api/*` を daemon の `/api/v1/*` に写す）。手順書の検査段 2 も `PATCH /api/org/browser-execution/browser-settings` に直している（`docs/ops/browser-web-live-check.md:91`）。
- R3（試験専用 egress 許可と拒否理由の記録）: launcher config の `test_loopback_allow` が許可ページの `127.0.0.1:<PAGE_PORT>` 1 件だけであることを起動前に完全一致検査（同 `:120-129`）。launcher session の `egress-denied.jsonl` から範囲外 port の `kind=private_address`・host・port・時刻・session id を検査して `egress-denied.json` を保存し、不許可ページへの GET が無いことと許可ページへの run 由来 GET を確認した後、Playwright で `allowed-page.png` を撮る（同 `:480-538`）。手順書の「試験専用 egress 許可」「証跡」節に記録。
- R4（cleanup の socket 残り）: trap の cleanup が process group 停止後に `$LAUNCHER_SOCKET` と `$OWNER_SOCKET` を削除する（同 `:137-145`）。手順書の証跡節に「launcher socket と `web-private/owner.sock` を消す。同じ証跡ディレクトリで再実行できる」と記載。
- カバー欠け 2 件: (1) 別ログイン session の Live View GET と stream upgrade がともに 403（同 `:436-446`）。(2) owner が lease を保持中の `input_mouse` が Live View upstream fixture に届くこと（`leased_input`、同 `:361-376`・`:462`）。lease 無しの拒否は agent 実行中と返却後の 2 回（同 `:448`・`:464`）。手順書の検査段 5・6・7 に記載。

## 検査結果

- `bash scripts/dev/browser-web-live-check.sh`（未 opt-in: `CELERIS_BROWSER_REAL_CHECK` も `CELERIS_USERNS_TESTS` も 1 でない）: exit 2。
- `bash -n scripts/dev/browser-web-live-check.sh`: exit 0。
- 台本内の Python 7 ブロックを `compile()` で構文確認: exit 0。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`: すべて exit 0。
- `git diff --check`: exit 0。
- 範囲: `git diff --name-only "$CELERIS_WU_BASE"` は `scripts/dev/browser-web-live-check.sh`・`docs/ops/browser-web-live-check.md`・`agent-docs/progress/2026-10-05-browser-web-live-view/script-fix.md`（取り込み分）と本ファイルだけ。`crates/`・`web/`・`gui/` の差分は無い。

実 launcher での実行は worker sandbox に user namespace が無いため行わない。Fable が integrate-fix2 後に rerun3 で `docs/ops/browser-web-live-check.md` 手順書どおり実行し、`checks.json`・`allowed-page.png`・`egress-denied.json` を証跡に残す。
