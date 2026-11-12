---
title: host の負荷で review の検査が時間切れになる — 重い検査は host 共通の枠を取り、枠の待ちを timeout に数えない
tasks: [01M75G99Q005DBQW4S5TZG88RK]
status: done
updated: 2026-11-12
---
# host の負荷で review の検査が時間切れになる — 重い検査は host 共通の枠を取り、枠の待ちを timeout に数えない

完了日: 2026-11-12

## 結論
- ADR `agent-docs/adr/2026-11-12-host-check-slots.md` を書いた。重い検査（cargo・nextest・test-affected・test-parallel を含む
  command）は host 共通の枠（`$HOME/.cache/celeris/check-slots/slot-<i>.lock` の flock、数は `max(1, core 数 / 12)`）を
  取ってから始め、枠を待つ時間は timeout に数えない。
- daemon: `task_worker::check_slots::SlottedWorkspace` が review（local の作業場所・2 つ目以降の repo）、WU の検査、段の統合の
  検査の `exec` を包む。待ちの理由は 1 回だけ log と `waiting/` に残す。人への質問・認可・受信箱の件にはしない。
- script: `scripts/dev/lib/check-slot.sh` を足し、test-affected.sh（nextest の前）と test-parallel.sh（準備の build の前）が
  枠を取る。nextest の `--test-threads` は `core 数 / 枠の数`。daemon が取った枠は `CELERIS_CHECK_SLOT` で渡り、取り直さない。
- worker の prompt（`result_json_instructions`、全 harness 共通）に「host の負荷・資源の待ちに question・決定・認可を使わない」
  を足した（ADR 2026-11-11 unit-task-dependencies D7 と同じ方針）。
- `celerisctl scratch status` が枠の使用中と、枠を待つ検査（label・開始時刻・理由）を出す。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`。main への rebase の後に実行）
- `bash scripts/dev/test-affected.sh`: exit 100（1279 秒）。repo-health pass、runner-tmp-lifecycle ok（199 checks）、runner-tmp-repro ok、
  `check-slot: 枠 0/2 を使う。--test-threads 12`、nextest 6071 件中 6070 passed・1 failed。落ちたのは範囲外の
  `task-worker browser::post_login_tests::daemon_post_login_pair_login_reads_lms_and_refuses_idp_other_and_password_pages`
  （本物の Chromium の download 数が 2、期待 1。load 30〜60）。単独の再実行
  `cargo nextest run -p task-worker daemon_post_login_pair_login_reads_lms` は 1 passed。この変更は browser の code に触れない。
- `cargo nextest run -p task-worker check_slots`: 4 passed（`a_heavy_check_waits_for_a_free_slot_and_the_wait_is_not_counted_in_its_timeout`
  ほか 3 件。tokio の時計を止めて timeout の 10 倍待っても始まらない・理由は 1 回・枠が空けば全 timeout で合格）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- shell の helper の手動確認: 枠 1 本で 2 つ目の取得が「check-slot: 枠を待つ（理由: host の検査の枠 1 本がすべて使用中（one））」を
  1 回出し、1 つ目の終了後に枠を取った。子が残した孤児（`sleep 30 &`）が居ても、script の終了で枠は空いた。

## commit
- phase 1: 重い検査は host 共通の枠を取ってから始め、枠の待ちを timeout に数えない
- phase 1: ADR に枠の dir を決められない時の扱いを書く
- phase 1: 検査の作業場所を slotted で包み、dispatcher の行を増やさない
- phase 1: spawn_review の行を main より増やさない
- phase 1: runner-tmp-lifecycle の偽 repo に check-slot.sh を写し、枠を試験の一時 dir に向ける
- phase 1: 枠の lock の fd を script の子孫に継がせない（試験の孤児が枠を握らない）
- phase 1: 進捗に提出時の検査の結果を書き、done にする

## 未解決事項
- `ownerless_runs` の reviewer の行の TTL は枠の待ちを含まない（ADR の既知の限界）。
- worker の sandbox の中の `$HOME/.cache/celeris/check-slots` が daemon と同じ file（同じ inode）かは本番で未確認。この run の
  sandbox では作成・取得ができた。
- 範囲外の browser の試験（上記）が負荷で落ちることがある。

## 本番への移行
1. この変更を含む release を promote する（人。`scripts/selfdeploy/promote.sh`）。daemon の検査と release gate の
   test-parallel.sh、worker の test-affected.sh が同じ枠（`~/.cache/celeris/check-slots`）を使い始める。
2. 確認: 重い検査が重なった時に `celerisctl scratch status` の末尾に `check slots … · N / 2 in use · M waiting` と、待つ検査の
   理由が出る。daemon の log に `check waits for a host check slot` が待ち 1 件につき 1 回出る。
3. `review_timeout_secs` は 1800 のまま残す（ADR D8）。600 に戻す条件: 枠のある release で 14 crate の test-affected
   （task-core を変える task）の review の検査の実行時間（log の check 開始から終了まで、枠の待ちを除く）を 3 回以上見て、
   最長が 300 秒以下であること。満たしたら `PATCH /api/v1/config/fields` で `review_timeout_secs = 600` にし、退避の
   `~/.config/celeris/config.toml.bak-20261111-review-timeout` は消さずに残す。

## 提案
- 枠の数と core あたりの比（12）を config に移すか（今は環境変数と既定の式）。
