---
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
completed: 2026-10-06
---

# Browser web 実機台本の R1・R3・R4 補修

## 変更

- 設定編集は web 画面と同じ `/api/v1/org/browser-execution/browser-settings` を使う。許可 origin の編集・不正値の拒否・復元を台本で確認する。
- 試験 launcher の `test_loopback_allow` は許可ページの `127.0.0.1:<PAGE_PORT>` **1 件だけ**と検証する。現在の browser session の `egress-denied.jsonl` を試験用の `sudo -n` 経路で読み、範囲外 port の `private_address` を `egress-denied.json` に保存する。許可ページへの run の GET を確認した後、Playwright で内容を確認して `allowed-page.png` を撮る。
- cleanup は起動した process group の停止後、試験 launcher socket と web-private/owner.sock を削除する。拒否証跡は再実行で上書きでき、古い session の記録を合格判定に使わない。
- 別ログイン session による Live View GET と upgrade がともに 403 になること、owner が lease を保持する間の `input_mouse` が upstream fixture に届くことを追加した。lease 無しの拒否検査は agent 実行中と返却後に続ける。
- `docs/ops/browser-web-live-check.md` に試験専用 egress 許可、証跡、再実行と上記の検査を記載した。

## 検査

- `bash -n scripts/dev/browser-web-live-check.sh`: exit 0。
- `bash scripts/dev/browser-web-live-check.sh`（未 opt-in）: exit 2 を確認。
- 台本内の Python 7 ブロックを `compile()` で構文確認: exit 0。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`、`sh scripts/dev/check-adr-numbers.sh`: すべて exit 0。
- `git diff --check`: exit 0。`CELERIS_WU_BASE` からの変更は台本・手順書・本進捗だけ。

実 launcher の確認は worker sandbox に user namespace がないため、この葉では行わない。Fable が統合後に手順書どおり実行し、`allowed-page.png`、`egress-denied.json`、`checks.json` を確認する。
