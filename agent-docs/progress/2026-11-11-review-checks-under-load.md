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
  command）は host 共通の枠（`${CELERIS_CHECK_SLOTS_DIR:-$XDG_RUNTIME_DIR/celeris/check-slots}/slot-<i>.lock` の flock。
  dir は host の local の tmpfs、数は `max(1, core 数 / 12)`）を取ってから始め、枠を待つ時間は timeout に数えない。
- daemon: `task_worker::check_slots::SlottedWorkspace` が review（local の作業場所・2 つ目以降の repo）、WU の検査、段の統合の
  検査の `exec` を包む。待ちの理由は 1 回だけ log と `waiting/` に残す。人への質問・認可・受信箱の件にはしない。
- script: `scripts/dev/lib/check-slot.sh` を足し、test-affected.sh（nextest の前）と test-parallel.sh（準備の build の前）が
  枠を取る。nextest の `--test-threads` は `core 数 / 枠の数`。daemon が取った枠は `CELERIS_CHECK_SLOT` で渡り、取り直さない。
- worker の prompt（`result_json_instructions`、全 harness 共通）に「host の負荷・資源の待ちに question・決定・認可を使わない」
  を足した（ADR 2026-11-11 unit-task-dependencies D7 と同じ方針）。
- `celerisctl scratch status` が枠の使用中と、枠を待つ検査（label・開始時刻・理由）を出す。

## attempt 3（final review 01M78AJAY9 の criterion 4 の不備 2 点と advisory 1 点の直し）
- 枠の dir を host の local にした。既定は `${CELERIS_CHECK_SLOTS_DIR:-$XDG_RUNTIME_DIR/celeris/check-slots}`（以前の既定は
  nfs4 の共有 home の下で、別の host と枠を取り合い、`waiting/` の pid を local の `/proc` で誤って判定した）。home に落ちる
  経路は置かない。両方の env が無ければ daemon は warn を 1 行出して包まず、script は理由を stderr に出して exit 2。
  Rust は env を読む `CheckSlots::from_env` と純粋な `slots_dir`・`slot_count`・`test_threads`・`CheckSlots::resolve`、
  `SlottedWorkspace::wrap_with` に分けた。shell は `check_slot_dir`・`check_slot_count`・`check_slot_test_threads` に分けた。
  `CELERIS_CHECK_SLOTS` の解釈も両方で「10 進の数字だけで 1 以上」にそろえた（以前は Rust だけ前後の空白と `+` を許し、
  shell は `00` を 1 にしていた）。
- 旧い並列度の説明を消した: `scripts/dev/test-parallel.sh` の env の説明、`docs/ops/nextest.md` の「並列数」、
  `docs/ops/selfdeploy.md` の `cargo-test` 段を、今の既定（`CELERIS_TEST_JOBS` が無ければ枠の並列度
  `max(1, nproc / 枠の数)`、24 core・枠 2 で 12）に書き換えた。ADR-0041 §8 の「並列数」に置き換えの付記を足した。
  `scripts/selfdeploy/release.sh` の usage の 1 行は残した（「提案」）。
- 式の二重化の advisory: `check_slots` の試験が bash に `scripts/dev/lib/check-slot.sh` を読ませ（core 数は
  `OMP_NUM_THREADS`、env は Command にだけ渡す）、core 数 1・12・24・48 × `CELERIS_CHECK_SLOTS` 9 通りの枠の数・並列度と、
  dir の解決 6 通りが Rust と一致することを確かめる。
- ADR D1（host の local の要件・NFS を避ける理由・tmpfs で消えてよい理由）、D2（一致の試験）、D3（env が無いときの扱い）、
  「撤去」（release.sh の残り・撤去の条件・責任者）を直した。
- `celerisctl scratch status` は枠の行に `--test-threads N` を足し、dir を決められないときはその理由を 1 行出す。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`。main への rebase の後に実行）
- attempt 3: `bash scripts/dev/test-affected.sh`: exit 0（1169 秒、load 25〜43）。対象 crate は celeris・celeris-campaign・celerisctl・
  llm-proxy・task-api・task-dispatch・task-worker。nextest 5993 passed・22 skipped、`check-slot: 枠 0/2 を使う。--test-threads 12`
  （枠の dir は `/run/user/1001/celeris/check-slots` に作られた）。repo-health verdict=pass（new 0・worsened 0）、
  runner-tmp-lifecycle ok（199 checks）、runner-tmp-repro ok（18 case）、script bundle SKIP 0 件。
- attempt 3: `cargo nextest run -p task-worker -E 'test(/check_slots::/)'`: 7 passed（新しい
  `slot_count_and_test_threads_match_the_shell_lib`・`slots_dir_matches_the_shell_lib_and_never_falls_back_to_home`・
  `without_a_slot_dir_the_daemon_warns_once_and_runs_checks_unwrapped` を含む）。
- attempt 3: `cargo clippy --workspace -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0、
  `sh scripts/dev/progress-index.sh --check` exit 0。
- attempt 3: `git grep 'min(8, max(2, nproc/3))' -- scripts/dev/test-parallel.sh docs/ops/nextest.md docs/ops/selfdeploy.md` は 0 件
  （exit 1）。`grep -n '\.cache\|HOME' crates/task-worker/src/check_slots.rs scripts/dev/lib/check-slot.sh` も 0 件。
- test-affected・clippy は「phase 1: 枠の dir を host の local …」の commit で実行した。その後の commit は進捗ファイルだけを変える。

## 証拠（attempt 2、記録）
- `bash scripts/dev/test-affected.sh`（最後の実行）: exit 0（1188 秒、load 50〜60）。nextest 6071 件 6071 passed、script bundle SKIP 0 件、
  `check-slot: 枠 0/2 を使う。--test-threads 12`。
- 同じ検査の 1 回前の実行: exit 100（1279 秒）。repo-health pass、runner-tmp-lifecycle ok（199 checks）、runner-tmp-repro ok、
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
- phase 1: 進捗に最後の test-affected の結果を書く
- phase 1: 枠の dir を host の local（XDG_RUNTIME_DIR）に移し、旧い並列度の説明を消し、Rust と shell の式の一致を試験で固める
- phase 1: 進捗に attempt 3 の検査の結果を書く

## 未解決事項
- `ownerless_runs` の reviewer の行の TTL は枠の待ちを含まない（ADR の既知の限界）。
- 本番の daemon の env に `XDG_RUNTIME_DIR` があるかは未確認（この run から daemon の `/proc/<pid>/environ` は読めない）。
  worker の run（この run）では `XDG_RUNTIME_DIR=/run/user/1001`（tmpfs）で、書き込めた。配備の後に `celerisctl scratch status`
  で確かめる（「本番への移行」2）。
- 範囲外の browser の試験（上記）が負荷で落ちることがある。

## 本番への移行
1. この変更を含む release を promote する（人。`scripts/selfdeploy/promote.sh`）。daemon の検査と release gate の
   test-parallel.sh、worker の test-affected.sh が同じ枠（`/run/user/<uid>/celeris/check-slots`）を使い始める。
2. 確認: `celerisctl scratch status` の末尾が `check slots /run/user/<uid>/celeris/check-slots · N / 2 in use · M waiting ·
   --test-threads 12` になる。`check slots: unknown dir …` が出たら、daemon の unit の env に `XDG_RUNTIME_DIR` が無い
   （枠なしで検査が走る）。daemon の log の `check slots: neither CELERIS_CHECK_SLOTS_DIR nor XDG_RUNTIME_DIR is set` でも分かる。
   重い検査が重なった時は daemon の log に `check waits for a host check slot` が待ち 1 件につき 1 回出る。
3. 試験と attempt 1・2 の script が残した旧い dir `~/.cache/celeris/check-slots`（`slot-<i>.lock`・`slot-<i>.holder`・
   `waiting/`）は消してよい（人。`rm -r ~/.cache/celeris/check-slots`）。この branch より前の版の test-affected.sh が走って
   いない時に消す（走っていれば、その process が lock を持つ）。新しい版はこの dir を読まない。
4. `review_timeout_secs` は 1800 のまま残す（ADR D8）。600 に戻す条件: 枠のある release で 14 crate の test-affected
   （task-core を変える task）の review の検査の実行時間（log の check 開始から終了まで、枠の待ちを除く）を 3 回以上見て、
   最長が 300 秒以下であること。満たしたら `PATCH /api/v1/config/fields` で `review_timeout_secs = 600` にし、退避の
   `~/.config/celeris/config.toml.bak-20261111-review-timeout` は消さずに残す。

## 提案
- `scripts/selfdeploy/release.sh` の usage の `CELERIS_TEST_JOBS` の行（34 行目）は旧い既定の式のまま残る。selfdeploy の
  高 risk の path class なので、この task（設計 brief の無い差分）では変えなかった。挙動は test-parallel.sh が決めるので、
  誤っているのは usage の文だけ。撤去の条件: selfdeploy の設計 review を通す別 task で、その行を枠の並列度
  （`max(1, nproc / 枠の数)`、24 core・枠 2 で 12）の説明に書き換える。責任者: operator がその task を起こす。
- 枠の数と core あたりの比（12）を config に移すか（今は環境変数と既定の式）。
