# ADR 2026-11-12: 重い検査は host 共通の枠を取ってから始め、枠の待ちは timeout に数えない

- 状態: 採用（実装は task 01M75G99Q005DBQW4S5TZG88RK）
- 関係: [ADR-0074](0074-parallel-work-units-checkpoints-milestones-quota.md) D6.1（review の検査の timeout と 2 倍の再試行）の付記、
  [ADR 2026-10-20 全体の試験は release だけ](2026-10-20-full-tests-at-release-only.md)、
  [ADR 2026-11-11 unit の task 依存](2026-11-11-unit-task-dependencies.md)（人の決定を待ちに使わない）

## 背景

2026-11-11、task 01M752V5864C の review で受け入れ条件の `bash scripts/dev/test-affected.sh`（14 crate・nextest 8224 件）が
`review_timeout_secs = 600`（再試行 1200 秒）で 2 回時間切れになった。load average は 60〜100（24 core）。同じ時刻に
release gate の `cargo nextest run --workspace --test-threads 8`、別 task の review の test-affected、running の worker が
自分で走らせた test-affected が重なっていた。どれも互いを知らずに並列度を決め（test-affected は nextest の既定 = 論理
core 数）、host 全体で同時に走る重い検査の数に上限が無かった。3 回通らなかった worker は「load average 20 以下の時に
『再検査』と答えてください」という認可（approval 01M75G3FT8）を人に出した。負荷の待ちを人に見張らせる形である。

## 決定

- **D1 枠の置き場所は host の local の lock file**: 重い検査は host 共通の枠（check slot）を 1 つ取ってから始める。枠は
  `${CELERIS_CHECK_SLOTS_DIR:-$XDG_RUNTIME_DIR/celeris/check-slots}/slot-<i>.lock`（i = 0..N-1）の `flock(2)` の排他 lock。
  - dir は host の local でなければならない。`$XDG_RUNTIME_DIR`（`/run/user/<uid>`）は systemd が user ごとに置く host の
    tmpfs で、daemon の user unit（user の systemd manager が `XDG_RUNTIME_DIR` を渡す）と worker の run の両方から見えて
    書ける（2026-11-12 に planner が worker の run の `/proc/self/mountinfo` で tmpfs と確認）。`CELERIS_CHECK_SLOTS_DIR` で
    差し替えるときも host の local の dir を指す（試験は一時 dir を指す）。
  - user の home（このホストでは nfs4 の共有 home）には置かない。同じ home を mount する別の host と同じ lock file を取り合い、
    上限が host の core 数ではなく共有全体にかかる。`waiting/` の pid は local の `/proc` で生死を判定するので、別の host の
    待ちを誤って消したり出したりする。NFS の `flock` は server 経由の lock になり、client の再起動・切断で外れ方も変わる。
  - `CELERIS_CHECK_SLOTS_DIR` も `XDG_RUNTIME_DIR` も無ければ dir を決めない（home などの別の dir に落ちない。D3）。
  - tmpfs なので host の再起動で dir ごと消えてよい。正本は lock だけで、lock は process と一緒に消える。dir は次に枠を取る
    process が作り直す。
  daemon（Rust）と script（shell の `flock`）が同じ file を取り合うので、daemon の review・WU・段の統合の検査、release gate
  の試験、worker が自分で走らせる試験のどれも同じ上限に従う。process が終われば kernel が lock を外すので、kill -9 でも
  枠は漏れない。正本は lock だけで、holder の file（`slot-<i>.holder`）と待ちの file（`waiting/`）は表示用である
  （空いているかの判定は lock の取得で行い、holder の file は見ない）。
  lock の fd は子孫に継がせない。daemon は Rust の既定の `O_CLOEXEC` で開き、script は子を起こす呼び出し（test-affected.sh の
  nextest・doc test、test-parallel.sh の準備・nextest・doc test）に `{CHECK_SLOT_FD}>&-` を付ける（bash の `{var}<>` は
  CLOEXEC を付けない）。継ぐと、試験が残した孤児（2026-11-10 の release gate の `sleep 3600`）が script の終了後も枠を握る。
- **D2 枠の数と試験の並列度**: N = `CELERIS_CHECK_SLOTS`（正の整数）があればそれ、無ければ `max(1, core 数 / 12)`
  （24 core で 2）。枠を取った試験は nextest の `--test-threads` を `max(1, core 数 / N)`（24 core で 12）にする。
  1 本の試験 thread が daemon・tar などの子 process で 1 core を超えるので、N × threads を core 数に合わせる。
  daemon は `std::thread::available_parallelism`、script は `nproc` で core 数を数える。dir・枠の数・並列度の規則は言語ごとに
  2 か所にある（task-worker の `check_slots::{slots_dir, slot_count, test_threads}` と `scripts/dev/lib/check-slot.sh` の
  `check_slot_dir`・`check_slot_count`・`check_slot_test_threads`）。`check_slots` の試験が bash にその file を読ませ、core 数
  1・12・24・48 と `CELERIS_CHECK_SLOTS` の値ごとの枠の数・並列度と、dir の解決が一致することを確かめる（片方だけ変えると落ちる）。
- **D3 どこで取るか**:
  1. daemon の検査（review の受け入れ条件と repo の検査、WU の検査、段の統合の検査）: `task_worker::check_slots::SlottedWorkspace`
     が `Workspace::exec` を包み、重い command（D4）だけ枠を取ってから内側の `exec` を timeout 付きで呼ぶ。枠を待つ
     時間は timeout に数えない（timeout は枠を取った後の実行にだけかかる）。取った枠は `CELERIS_CHECK_SLOT=<lock の path>`
     として command に渡す。remote（ssh）の作業場所は包まない（枠は daemon の host の資源で、remote の host の負荷とは別）。
     枠の取得元（`task_worker::check_slots::SlotSource`）は daemon の起動時に env から 1 回だけ決め（`SlotSource::from_env`。
     `crates/celeris/src/config/dispatch.rs` の `Config::dispatch_config`）、`DispatchConfig::check_slots` で dispatcher に
     渡す。dispatcher は検査の度に env を読まない（`with_check_slot` で設定の取得元を使う）。設定の再読み込み
     （`apply_dispatch_config`）では入れ替えない。試験の dispatcher は一時 dir の枠を入れ、host の枠の dir を開かない。
     `CELERIS_CHECK_SLOTS_DIR` も `XDG_RUNTIME_DIR` も無い process では枠の dir を決められないので、起動時の解決
     （`SlotSource::resolve_or_warn`）で warn を 1 行出し、検査を包まない（daemon は user の systemd unit で、user の
     manager が `XDG_RUNTIME_DIR` を渡すので、本番では起きない見込み。本番の daemon の env は未確認なので、配備の後に
     `celerisctl scratch status` で枠の dir を確かめる）。
  2. 呼び出し元が既に枠を持つ（`CELERIS_CHECK_SLOT` が空でない）なら、daemon も script も枠を取り直さない（入れ子で自分の
     枠を待つ deadlock と、親の枠と別に 2 本目を取ることを防ぐ）。規則は Rust と shell で同じで、dir・数より先に見る:
     - Rust: `SlotSource::resolve` が `CELERIS_CHECK_SLOT` を見て `SlotSource::Held(<lock の path>)` を返す。
       `SlottedWorkspace` は held なら重い command も枠を取らず、その path を `CELERIS_CHECK_SLOT` で渡してすぐ内側の
       `exec` を呼ぶ（test-affected.sh・test-parallel.sh の中で起きた daemon が対象）。
     - shell: `check_slot_acquire` は `CELERIS_CHECK_SLOT` があれば取らずに 0 を返す（daemon が取って渡した場合）。
     `check_slots` の試験が一時 dir の枠を全部持った上で `CELERIS_CHECK_SLOT` を渡し、Rust も bash も待たずに走ることを
     確かめる（held が無ければ両方が待つことも確かめる）。
  3. script: `scripts/dev/test-affected.sh` は nextest の前、`scripts/dev/test-parallel.sh`（release gate）は準備の build の
     前に `scripts/dev/lib/check-slot.sh` の `check_slot_acquire` で枠を取る（2 の held なら取らない）。dir を決められなければ
     理由を stderr に出して exit 2（枠なしで走らない）。
  4. worker が直接打つ `cargo clippy`・`cargo build` は枠を取らない（試験より短く、worker の run の壁時計の中で収まる。
     絞る対象は host の負荷の大半を占める nextest の試験）。
- **D4 重い command の判定**: command の字句に `cargo`・`cargo-nextest`・`nextest` の語、または `test-affected`・
  `test-parallel` を含むもの（決定的。`task_worker::check_slots::is_heavy`）。`sh scripts/dev/progress-index.sh --check` の
  ような軽い検査は枠を待たない。
- **D5 待ちの理由は 1 回だけ残し、人に聞かない**: 枠がすべて使用中なら、待ちの始めに 1 回だけ理由（枠の数と使用中の
  検査の label）を log（`tracing::info`、"check waits for a host check slot"）と `waiting/` の file に書く。待ちは
  人への質問・認可・受信箱の件にしない。待つ間は 10 秒ごとに lock を取り直す（時計は tokio の時計なので試験では
  `tokio::time::pause` で進める）。
- **D6 status**: `celerisctl scratch status`（text）が枠の一覧（使用中の検査の label・開始時刻）と、枠を待っている検査
  （label・開始時刻・理由）を出す。`waiting/` の file は pid が生きているものだけ出す。
- **D7 worker への指示**: worker の prompt の結果ファイルの指示（全 harness 共通の `result_json_instructions`）に「host の
  負荷・資源（検査の枠・disk・quota）の待ちに question を使わない。負荷で検査が時間切れになったら結果をそのまま
  報告する。待ちは Celeris が持つ」を足す。ADR 2026-11-11 unit-task-dependencies D7（人の決定は人にしか決められない
  ことだけに置く）と同じ方針で、食い違わない。
- **D8 `review_timeout_secs`**: 枠の待ちを数えないので、timeout は「枠を取った後、高々 N 本の重い試験が並ぶ host で
  1 本が走る時間」だけを見ればよい。2026-11-12 の実測では task-core を変えた test-affected が load 20〜70 で
  1091〜1184 秒（nextest 720〜733 秒、24 threads）かかった。枠で並列が 2 本・12 threads に限られた host でも、
  14 crate の test-affected は 600 秒に収まる見込みが立たない（未計測）。よって本番の暫定値 1800 は 600 に戻さない。
  戻すのは、枠のある release で 14 crate の test-affected を計測し、最長が 600 秒の半分（300 秒）以下になった後とする
  （計測は進捗ファイルの「本番への移行」節の手順）。2 倍の再試行（上限 1800 秒）はそのまま残す。

## 撤去

- 上限なしで重い検査を同時に走らせる経路（daemon の検査の素の `LocalWorkspace`、test-affected.sh の nextest の既定
  並列度、test-parallel.sh の `jobs_default`）を同じ変更で置き換えた。切替 flag・互換分岐は置かない。
- 旧い並列度の既定の説明は同じ変更で直した（`scripts/dev/test-parallel.sh` の env の説明、`docs/ops/nextest.md` の「並列数」、
  `docs/ops/selfdeploy.md` の `cargo-test` 段、ADR-0041 §8 の「並列数」に付記）。`agent-docs/progress/phase-G.md` と
  `scripts/dev/testdata/runner-tmp-repro/test-parallel.sh.pinned` は履歴と固定の写しなので変えない。
- 残す所（一時）: `scripts/selfdeploy/release.sh` の usage の `CELERIS_TEST_JOBS` の行（34 行目）が旧い既定の式を書いている。
  selfdeploy の高 risk の path class にあり、この task の計画（設計 brief を持たない差分）では変えない。挙動は
  test-parallel.sh が決めるので、残るのは usage の文だけである。撤去の条件: selfdeploy の設計 review を通す別 task で
  その行を枠の並列度の説明に書き換える。責任者: operator がその task を起こす（この task の進捗ファイルの「提案」）。
- 枠の dir の旧い既定（user の home の cache の下）は D1 で置き換えた。home に落ちる経路は残さない。試験が残した旧い dir は
  進捗ファイルの「本番への移行」の手順で消す。

## 付記（実装の突き合わせ）

- `SlottedWorkspace` を使う所（`DispatchConfig::check_slots` を `with_check_slot` に渡す）: `dispatcher/review_spawn.rs`
  （review の local の作業場所と 2 つ目以降の repo の検査）、`dispatcher/work_units.rs`（WU の検査）、
  `dispatcher/phase_integration.rs`（段の統合の検査。`run_integration_checks` が取得元を受け取る）。
- 試験: `task_worker::check_slots` の `a_heavy_check_waits_for_a_free_slot_and_the_wait_is_not_counted_in_its_timeout`
  （枠 1 本を別の holder が持つ間、timeout の 10 倍待っても実行しない・理由は 1 回・枠が空けば全 timeout で実行し合格）、
  `a_light_check_never_waits_for_a_slot`、`heavy_commands_are_the_cargo_and_test_scripts`、`status_lists_holders_and_live_waiters`、
  `slot_count_and_test_threads_match_the_shell_lib`・`slots_dir_matches_the_shell_lib_and_never_falls_back_to_home`（D1・D2 の
  Rust と shell の一致、env が無いと script は exit 2）、`without_a_slot_dir_the_daemon_warns_once_and_runs_checks_unwrapped`（D3）。
- 既知の限界: `ownerless_runs` の reviewer の行の TTL（`3 × (条件数 + 2) × review_timeout`）は枠の待ちを含まない。
  daemon の引き継ぎ中に旧 daemon の review が長く枠を待つと、新 daemon が行を早く閉じうる（行は索引で、判定は旧 daemon が
  届ける）。発生したら別 task で扱う。

## 付記 2026-11-12（final review の不合格の直し: held の規則を Rust にも置き、取得元を起動時に注入する）

- 不備: D3.2 の「`CELERIS_CHECK_SLOT` があれば取り直さない」を守るのが shell の `check_slot_acquire` だけで、Rust の枠の取得は
  見ていなかった。さらに dispatcher は検査の度に process の env から host の枠を開いていた。このため test-affected.sh・
  test-parallel.sh が枠を持つ間に走る dispatcher の試験が、本物の host の枠の dir（`$XDG_RUNTIME_DIR/celeris/check-slots`）を
  開いて 2 本目の枠を取りに行き、枠が全部使用中だと review 系の試験 4 件が待ちで落ちた（試験の holder・waiting の file も
  host の dir に残った）。
- 直し: 枠の取得元を `SlotSource`（`Host(CheckSlots)` と `Held(<lock の path>)`）の 1 つの型にし、env を受け取る純粋な
  `SlotSource::resolve` が `CELERIS_CHECK_SLOT` を dir・数より先に見る（D3.2）。`DispatchConfig::check_slots` を足し、本番は
  daemon の起動時に `SlotSource::from_env` で 1 回だけ決める（D3.1）。env を検査の度に読む入口（daemon の local の作業場所を
  包む関数と、`SlottedWorkspace` の env から包む関連関数）は消した。dir を決められないときの warn 1 行と包まない扱いは
  起動時の解決に移した（検査の度には warn しない）。
- 試験の dispatcher（`dispatcher/tests/mod.rs`・`tests/review.rs`）は process に 1 つの一時 dir の枠（64 本、`atexit` で消す）を
  入れる。dagbench（`campaign` の `dagbench/celeris_side.rs`）は検査が合成の軽い command なので `None`（枠を使わない）を入れる。
- 試験: `a_held_slot_is_not_taken_again_by_rust_or_the_shell_lib`（Rust と bash の held の一致。枠を全部持った一時 dir で、
  held なら待たずに走り path を変えず dir に file を増やさない。held が無ければ両方が待つ）、dispatcher の
  `a_heavy_review_check_takes_the_injected_slot_and_receives_its_path`（review の重い検査が注入した一時 dir の枠の path を
  `CELERIS_CHECK_SLOT` で受け取り、検査の後の枠の dir は lock だけ）。D8 の `review_timeout_secs` の判断は変えない。
