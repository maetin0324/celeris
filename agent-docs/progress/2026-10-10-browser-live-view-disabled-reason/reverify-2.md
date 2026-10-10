---
task: 01M4HRQBN8XHKQRD1DAXHXBSEX
wu: reverify-2
status: done
completed: 2026-10-10
---
# reverify-2: fail-names 導入後の全体試験と失敗 1 件の決定化

## 経緯
- attempt 1〜2: `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` を 2 回流して両回 5024 passed / 0 failed（nextest_secs 302.0 / 129.5）。
  ところが run 後の check（同じコマンド）が 5023 passed / 1 failed（nextest_exit 100、nextest_secs 131.8）で落ちた。
  fail-names の出力で失敗名が出た: `test-parallel: failed: task-worker browser::post_login_tests::daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages`。
  panic は `browser_post_login_tests.rs:501`「other-origin download began (observation stopped: true)」。過去の final review の 1 failed（名前不明）もこれと推定（過去 log の panic 行は同 file の 442/443/501 だけ）。

## 判定（attempt 3）
単独で `TMPDIR=/tmp cargo nextest run -p task-worker -E 'test(daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages)'` を 3 回: 1 回目 FAIL（同じ 501 行、12.0 秒）、2〜3 回目 PASS（13.3 秒）。
負荷と無関係に起きる **flaky**（競合）。

原因: fixture（`browser_post_login_fixture.py`）は他 origin の file を `Content-Type: application/octet-stream` で header だけ先に送り body を 8 秒後に送る。
Chromium は octet-stream を MIME sniff するので、download の開始（`Browser.downloadWillBegin`）が body の到着まで遅れる。
そのため開始と同時に controller が送る `Browser.cancelDownload` と、既に全部届いた body の完了が競合し、完了が勝つと D2-6 の breach（観測停止、
本番の設計どおりの fail-closed）になって試験が落ちる。試験は「取消が間に合う」前提なので、fixture が前提を崩していた。
証拠: 修正前は試験 1 本が 12〜13 秒（8 秒の body 待ちが経路上にあった）、修正後は 2 本で 5.3 秒。

## 直し方（試験 file のみ。本番 code・fixture の .py は不変）
- attempt 3 の 1 回目は fixture の `.py` を直したが、範囲 check（試験 file は `*tests.rs` / `tests/` だけ）で `browser_post_login_fixture.py` が範囲外になった。
  そこで `.py` を base に戻し、同じ変更を `crates/task-worker/src/browser_post_login_tests.rs` 側で掛ける形にした:
  `FIXTURE` を `LazyLock<String>` にし、include した原本の 2 行（`Content-Type: application/octet-stream` → sniff されない `application/x-celeris-other`、
  body の遅延 `wait(8)` → `wait(60)`。60 秒は試験の出来事待ちの保険 `DOWNLOAD_EVENT_TIMEOUT` と同じ）を置き換える。置き換え元が 1 回ずつ現れることを assert し、
  原本が変わったら黙って効かなくなるのではなく試験が落ちる。開始は header 時点で決まり、取消は body より必ず先に届く。
- 呼び出し側は `ChromeFixture::start_with(&FIXTURE)`（post_login 1 箇所、`browser_launcher_run_tests.rs` の launcher 試験 3 箇所）。

## 証拠（attempt 3 再実施、試験側の置き換えで）
- 単独 3 回: `TMPDIR=/tmp cargo nextest run -p task-worker -E 'test(daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages) | test(launcher_credential_post_login_pair_login_then_reads_only_the_lms)'`
  → 3 回とも 2 passed（5.3 / 5.4 / 5.7 秒）
- 全体 1 回目 `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0。CELERIS_TEST_SUMMARY: passed 5024, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, nextest_secs 148.8, tmp_leftovers 3。`test-parallel: failed:` 行なし
- 全体 2 回目 → exit 0。passed 5024, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, nextest_secs 126.9, tmp_leftovers 5。`failed:` 行なし
- `df -h /local`: 69% → 68% → 69%（disk watch の 95% 未満）
- `cargo fmt --all` 済み、`cargo clippy --workspace --all-targets -- -D warnings` → exit 0
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は本進捗・`browser_post_login_tests.rs`・`browser_launcher_run_tests.rs` のみ（範囲 check exit 0）

## 未解決・提案
- 本番の D2-6: 他 origin の download が sniff 対象の型で body が小さいと、取消より完了が先になり観測停止（fail-closed）になりやすい。安全側だが利用者には
  「read origin 外の download を踏むと以後観測できない」になる。sniff 中は download 開始前なので navigation 段で止める（Fetch で response を止める等）案を別 task で検討してよい。
