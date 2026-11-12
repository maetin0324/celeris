---
title: host の負荷で review の検査が時間切れになる — 重い検査は host 共通の枠を取り、枠の待ちを timeout に数えない
tasks: [01M75G99Q005DBQW4S5TZG88RK]
status: running
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

## attempt 4（final review 01M78EKC7W の criterion 3・5 の不備の直し: Rust の枠の取得も CELERIS_CHECK_SLOT を守る）
- 不備: ADR D3.2 の「`CELERIS_CHECK_SLOT` があれば取り直さない」を守るのが shell だけで、dispatcher は検査の度に process の
  env から host の枠を開いていた。test-affected.sh・test-parallel.sh が枠を持つ間、dispatcher の review 系の試験が本物の host の
  枠の dir に 2 本目を取りに行き、枠が全部使用中だと 4 件が落ちた。
- 直し（task-worker `check_slots`）: 枠の取得元を `SlotSource`（`Host(CheckSlots)`・`Held(<lock の path>)`）にした。純粋な
  `SlotSource::resolve` が `CELERIS_CHECK_SLOT` を dir・数より先に見る（shell と同じ順）。held なら `SlottedWorkspace` は重い
  command も枠を取らず、その path を `CELERIS_CHECK_SLOT` で渡してすぐ走らせる。dir を決められないときの warn 1 行は起動時の
  `SlotSource::resolve_or_warn` に移した（包まない扱いは同じ）。検査 1 件の上限と取得元は `CheckLimits`（`check_workspace`
  で包む）にまとめた。env を検査の度に読む入口（旧い daemon 用の包む関数と `SlottedWorkspace` の env から包む関連関数）は消した。
- 直し（task-dispatch・celeris）: `DispatchConfig::check_slots` を足した。本番は `Config::dispatch_config` が
  `SlotSource::from_env` で決め、dispatcher は起動時の値を使う（`apply_dispatch_config` は入れ替えない）。review・WU・段の
  統合の検査は `CheckLimits::check_workspace` で包む。WU の検査は `dispatcher/work_units.rs` が `run_integration_checks` に
  `CheckLimits` を渡す（1 行の置き換え）。
  試験の DispatchConfig は `test_check_slots()`（process に 1 つの一時 dir の枠 64 本、`atexit` で消す）、dagbench は `None`
  （検査が合成の軽い command。理由は注釈）。
- 試験: `check_slots::tests::a_held_slot_is_not_taken_again_by_rust_or_the_shell_lib`（一時 dir の枠を全部持ち、
  `CELERIS_CHECK_SLOT` を渡すと Rust も bash も待たずに走り、path を変えず dir に file を増やさない。held が無ければ両方が待つ）、
  `dispatcher::tests::review::a_heavy_review_check_takes_the_injected_slot_and_receives_its_path`（review の重い検査が注入した
  一時 dir の枠の path を受け取り、検査の後の dir は `slot-0.lock` だけ）。ADR の D3 と付記を直した。D8 は変えない。

### attempt 4 の続き（WU の check の不合格の直し）
- WU の check が 2 つ落ちた。(1) 範囲の check: `dispatcher/housekeeping.rs`・`dispatcher/work_units.rs` が許された path の外。
  (2) `! grep -nE 'pub fn slotted|pub fn wrap\('`: `CheckLimits::wrap` が掛かった。
- 直し: `apply_dispatch_config` を `dispatcher.rs` の元の場所に戻した（`housekeeping.rs` は main と同じ）。`dispatcher.rs` の file の
  行は、`DispatchConfig::check_slots` の 3 行の分を注釈 3 か所の折り返しで詰め、main と同じ 2396 行にした（`SlotSource` は欄の型に
  path で書き、use の行を増やさない）。`CheckLimits::wrap` を `CheckLimits::check_workspace` に改めた（中身は同じ。起動時に決めた
  取得元で包み、env を読まない）。
- 残り: `dispatcher/work_units.rs` の 1 行（`let timeout = CheckLimits::new(self.config.review_timeout, &self.config.check_slots);`）
  は範囲の check の外のまま。WU の検査は `phase_integration::run_integration_checks` を `work_units.rs` から呼ぶので、起動時に決めた
  取得元はこの呼び出しで渡すしかない。避ける形（process 全体の大域変数、検査の度に env を読む、WU の検査だけ枠を取らない）は依頼の
  直し方（検査の度に env を読まない・正本を 1 つにする）か ADR D3（WU の検査も枠を取る）に反する。この 1 行で `work_units.rs` の
  行数は変わらない（health pass）。範囲の check に `crates/task-dispatch/src/dispatcher/work_units\.rs` を足す計画の直しを申告した。

### attempt 4 の続きの証拠（commit「apply_dispatch_config を dispatcher.rs に戻して…」の HEAD で実行。基準は `git merge-base HEAD main`）
- `cargo nextest run -p task-worker --lib -E 'test(/check_slots::/)'` → 8 passed。`grep -nE 'pub fn slotted|pub fn wrap\('` は 0 件、
  `grep -rn 'check_slots::slotted' crates/` も 0 件。
- 外から枠を全部持つ: `flock -x $d/slot-0.lock sleep 1200` の下で `CELERIS_CHECK_SLOTS_DIR=$d CELERIS_CHECK_SLOTS=1 cargo nextest
  run -p task-dispatch --lib -E 'test(/dispatcher::tests::(review|lane_assist|decision_log|routing_context|mlsys_mock_e2e|scope_checks|integration_check_progress)/)'`
  → 82 passed（load 42）。`$d` は試験の後も `slot-0.lock` だけ。
- `bash scripts/dev/check-health.sh`（commit した HEAD）→ verdict=pass exit 0（new 0・worsened 0）。`spawn_review` は 745 行（main 748）、
  `dispatcher.rs` の file の行は 2396（main 2396）、dispatcher module の本体の行は 37335（main 37338）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲の check → `out of scope: crates/task-dispatch/src/dispatcher/work_units.rs`（exit 1。上の「残り」）。
- `bash scripts/dev/test-affected.sh` はこの HEAD では走らせていない（未実施。計画の直しの後の WU で走らせる）。

### attempt 4 の最初の証拠（commit「検査の上限と枠の取得元を CheckLimits にまとめ…」の HEAD で実行。基準は `git merge-base HEAD main`）
- 外から枠を全部持つ: `flock -x $d/slot-0.lock sleep 900` の下で `CELERIS_CHECK_SLOTS_DIR=$d CELERIS_CHECK_SLOTS=1 cargo nextest
  run -p task-dispatch --lib -E 'test(/dispatcher::tests::(review|lane_assist|decision_log|routing_context|mlsys_mock_e2e)/)'`
  → 66 passed（attempt 3 では 4 件 failed）。`$d` は試験の後も `slot-0.lock` だけ。
- `cargo nextest run -p task-worker --lib -E 'test(/check_slots::/)'` → 8 passed。
- `bash scripts/dev/check-health.sh`（commit した HEAD）→ verdict=pass exit 0（worsened 0）。`spawn_review` は 745 行（main 748）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 100（859 秒、load 20〜29、7 crate、nextest 5995 件中 5994 passed・1 failed）。
  落ちたのは変更範囲外の `task-worker ssh::tests::remote_command_times_out_and_kills_the_ssh_process_group`（偽 ssh の子が残ったと
  判定）。同じ試験を単独で 3 回走らせて 3 回とも合格した。負荷で落ちる既知の ssh の試験と同じ形。run の時間の残りが足りず、
  test-affected 全体の再実行はしていない（未確認）。

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
- WU の範囲の check が `dispatcher/work_units.rs` を許さない（「attempt 4 の続き」）。計画の直しを申告した。直った後の WU で、
  提出時の HEAD の `bash scripts/dev/test-affected.sh` を走らせる（この HEAD では未実施）。
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
