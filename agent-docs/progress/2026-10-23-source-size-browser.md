---
tasks: [01M5KYMKBGCZP4MXHEF13TYAZQ]
status: done
completed: 2026-10-23
---
# source-size/browser: browser.rs と browser_cdp_sink.rs の責務分離（delivery-repair）

## 経緯
- 責務分離は d7cb0fd4（browser_auth / browser_conformance / browser_cdp_credential_ipc /
  browser_cdp_post_login へ抽出、公開入口は元 module から再公開）。review 合格。
- release gate（prepare）が `task-worker::browser_sandbox_artifacts
  real_sandbox_launcher_chrome_downloads_inline_pdf_after_login` 1 件で落ち、差し戻し。

## 原因の切り分け
- 同じ試験は分離前の 22711d25 でも高負荷（load 40〜100 / 24 CPU）で落ちた（4 回中 1 回、
  `open files.html: runner_reason=request_invalid`）。過去の gate でも 6 回、別の地点で落ちている。
  分離が原因ではない。
- `request_invalid` は決定的な不具合: 書き手（launcher の `runner_call`、`browser_action::run_action`、
  試験の helper）が `*.request` を作ってから中身を書く。runner は 20 ms ごとに `*.request` を読むので、
  空・途中の file を読むと JSON 不正で `request_invalid` を返す。
- gate の失敗（`snapshot after a download timeout: exec_timeout`）は daemon 再起動後の snapshot が
  45 s を超えたもの。手元の高負荷でも 1 回再現したが原因は特定できていない（下の未解決事項）。

## 修正
- 書き手 3 か所とも `<seq>.pending` に書き切ってから `<seq>.request` へ rename する
  （runner は `*.request` だけを読む）。

## 証拠
- 実 browser 試験（CELERIS_USERNS_TESTS=1、`cargo nextest run -p task-worker --test
  browser_sandbox_artifacts --run-ignored all`）: 修正後 3 回とも 3 passed（load 35〜103）。
- `python3 scripts/dev/check-source-size.py`: exit 0（0 failure, 3 debt warning, 1 reasoned exception）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`（main a3933dc2 取り込み後）: exit 0、4468 passed / 17 skipped。

## 未解決事項
- post-login の「Elsewhere Blank PDF」で agent-browser の daemon を止めた直後の snapshot が、
  高負荷でまれに 45 s（EXEC_TIMEOUT）を超える。分離前の commit でも同じ経路は負荷で不安定。
  daemon の死を待つ案は試したが再現が止まらず、採用していない。

## 提案
- daemon 再起動直後の relay 側（旧 agent 接続の後始末と新接続）の待ちを、SIGSTOP 等で決定的に
  再現する試験を足して原因を詰める（別 task）。
