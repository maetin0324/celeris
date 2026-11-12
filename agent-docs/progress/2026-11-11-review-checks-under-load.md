---
title: host の負荷で review の検査が時間切れになる — 重い検査は host 共通の枠を取り、枠の待ちを timeout に数えない
tasks: [01M75G99Q005DBQW4S5TZG88RK]
status: running
updated: 2026-11-12
---
# host の負荷で review の検査が時間切れになる — 重い検査は host 共通の枠を取り、枠の待ちを timeout に数えない

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

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo nextest run -p task-worker check_slots`: 4 passed（`a_heavy_check_waits_for_a_free_slot_and_the_wait_is_not_counted_in_its_timeout`
  ほか 3 件）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all`: 差分なしにした。
- shell の helper の手動確認: 枠 1 本で 2 つ目の取得が「check-slot: 枠を待つ（理由: host の検査の枠 1 本がすべて使用中（one））」を
  1 回出し、1 つ目の終了後に枠を取った。
- `bash scripts/dev/test-affected.sh`: 未実行（attempt 1 の壁時計内に収まらない。次の run で実行する）。

## commit
- phase 1: 重い検査は host 共通の枠を取ってから始め、枠の待ちを timeout に数えない

## 未解決事項
- `bash scripts/dev/test-affected.sh` と `sh scripts/dev/progress-index.sh --check` の提出時の実行。
- `ownerless_runs` の reviewer の行の TTL は枠の待ちを含まない（ADR の既知の限界）。

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
