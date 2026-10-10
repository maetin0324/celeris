---
task: 01M4HRQBN8XHKQRD1DAXHXBSEX
wu: reverify-full
status: done
completed: 2026-10-10
---
# reverify-full: rebase 後 HEAD の全体試験の取り直し

HEAD `12afa217` で全体試験を取り直した。全件合格し、試験 file の変更は無い。

## 経緯
- final review では `bash scripts/dev/test-parallel.sh` が 5023 passed / 1 failed（nextest exit 100）だった。stdout の末尾しか残っておらず、落ちた test の名前は分からない（task dir の runs/・integration-checks/ にも 5023 の run の nextest 本文は無い）。
- 前回の attempt は `CARGO_TARGET_DIR` の親が Read-only file system で、build 前に exit 101 で止まった（環境由来）。今回は書き込めた。

## 証拠
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0
  `CELERIS_TEST_SUMMARY {"passed": 5024, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "binaries": 175, "nextest_secs": 336.2, "summary_parsed": true}`
  （final review の 5023+1 = 5024 と件数が一致する。FLAKY・TRY の再試行行も無い）
- 過去の失敗 log で唯一の task-worker の候補 `browser::post_login_tests::daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages`（integrate-close log、browser_post_login_tests.rs:443。post-login-flake で直したもの）を単独で 3 回: 3/3 passed（各 13.4〜13.6 秒）
- `cargo fmt --all -- --check`: exit 0
- `cargo clippy --workspace -- -D warnings`: exit 0

## 判定
- final review の 1 件は HEAD で再現しない。名前が分からないので決定的か flaky かは分けられない。全件合格で再現しないので、負荷か時間に依る flaky と推定する。
- 直したもの: なし（変更はこの進捗ファイルだけ）

## 提案
- final review の check が落ちたときは、nextest の FAIL 行と panic 位置を stdout_tail の外にも残す（失敗した test 名の行を grep して result に入れる）。今回は名前が分からず、1 回の取り直し run を使った。
