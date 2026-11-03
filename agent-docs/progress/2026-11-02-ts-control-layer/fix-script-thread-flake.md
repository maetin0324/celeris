---
title: fix-script-thread-flake — shutdown 後に celeris-script thread が 1 本読める不安定の修正
tasks: [01M6DCMB75G5F0DSE58ZNZMBAK]
status: done
updated: 2026-11-03
completed: 2026-11-03
---
# fix-script-thread-flake — shutdown 後に celeris-script thread が 1 本読める不安定の修正

## 結論
本体は actor thread を join している。落ちた原因は試験の数え方だった。`JoinHandle::join`（`pthread_join`）は、kernel が thread の終了処理の途中で起こす。kernel が thread を `/proc/self/task` から外すのはその後なので、join の直後でも、join 済みの actor が comm `celeris-script` のまま読めることがある。試験の数え方を「生きている thread だけ」に直した。期待値（shutdown 後 0 本、復元中の actor 1 本）は変えていない。本体（`crates/script-runtime/`・`crates/celeris/src/`）は変えていない。

## 原因
- kernel の `do_exit` は先に `exit_signals` で PF_EXITING を立てる。次の `exit_mm`（`mm_release` の clear_child_tid）で futex を起こし、ここで `pthread_join` が返る。thread を thread group から外す `exit_notify` は、`exit_files`・`exit_fs`・`cgroup_exit` などの後に走る。負荷の下では、終わりかけの thread がこの間で preempt され、join した側が先に `/proc/self/task` を読める。
- 試験の `take_sample` は `/proc/self/task/<tid>/comm` だけを読み、終わりかけの thread も数えていた。
- 落ちた回の観察と合う。落ちた assert は `script_threads() == 0`（left 1）で、その前の `seen.actors == 1` は通っていた。疑い (b)（V8 platform の worker・watchdog・tokio の blocking thread が親の comm を引き継いだまま残る）だとすると、その thread は run の間ずっと `celeris-script` で読めていたはずなので census は 2 になり、先の assert で落ちる。census が 1 で、run の後に 1 本だけ読めたという結果は「actor 自身が join 済みのまま、まだ載っていた」ときだけ起きる。
- 疑い (a)（join しない経路）は無い。`ScriptLayer::stop` → `ScriptGenerations::shutdown`（`ops` の lock の下）→ `GenerationRegistry::shutdown` → `shutdown_all` → `ScriptRuntimeHandle::shutdown` → `Inner::stop` が thread を join する。登録・見回り・復元は同じ lock を順に取るので、shutdown の後に isolate は増えない。watchdog は actor の終わりの Drop で join される。V8 platform の worker は、platform を初期化した時点（最初の actor の起動時）に作られ、起動直後に `V8 DefaultWorke` へ改名する。

## 直し方（`crates/celeris/tests/script_runtime.rs`）
- `take_sample` は `/proc/self/task/<tid>/stat` を読む。`live_comm` は state が Z・X か、flags に PF_EXITING（0x4）が立っている thread を除く。join が返った thread には必ず PF_EXITING が立っている。生きている actor には立っていない。
- 同じ `take_sample` を `script_threads()`（:243 の `assert_rolled_back`、:324 の復元試験）と `census_actors`（`run_daemon` の sampler）が使うので、両方が直る。census にも同じ穴があった。復元試験は 2 周するので、1 周目の actor が zombie のまま 2 周目の sampler に読まれると 2 本と数えられうる。生きている thread だけを数えるようにしたので、この穴も塞がった。
- 決定的な再現のための試験専用の遅延フック `ExitHolder` を足した。この試験 binary を子 process（空の試験 `exit_holder_entry`、環境変数 `CELERIS_TEST_EXIT_HOLD_TID` のときだけ動く）として起こし、子が対象の thread に `PTRACE_SEIZE` する。trace されている thread は終わっても、tracer が wait するまで zombie として `/proc/self/task` に残る。join は普段どおり返るので、「join の後、kernel が外す前」の状態を時間に頼らずに作れる。親は Yama のために `prctl(PR_SET_PTRACER_ANY)` を呼ぶ。libc の 4 関数は FFI で宣言した（Cargo.toml は変えていない）。子は signal の配送による止まりを続けさせる。Drop で子を kill すると、kernel が trace を外して zombie も消える。
- 足した試験:
  - `script_runtime_daemon_shutdown_count_skips_unreaped_actor`: daemon の復元の間に sampler が actor へ holder を付ける。run の後、`listed_script_threads()`（修正前の数え方）が 1 であること（再現の前提）、`script_threads()` が 0、census が 1 であることを確かめる。
  - `script_thread_count_skips_joined_but_listed_thread`: daemon を使わず、名前 `celeris-script` の thread 1 本で同じことを確かめる。holder を外した後に tid が消えることも確かめる。
  - `script_thread_live_comm_reads_stat_lines`: comm に空白・括弧がある行、PF_EXITING・Z・X の行の解釈。

## 再現（修正前に落ち、修正後に通る）
- 修正前の数え方（`live_comm` の除外を一時的に外した状態）で `cargo nextest run -p celeris --test script_runtime --no-fail-fast -E 'test(/skips_joined|skips_unreaped|keeps_loaded/)'` を実行した。結果は 3 件中 2 件 FAIL。`script_runtime_daemon_shutdown_count_skips_unreaped_actor` は `assertion left == right failed: shutdown joined the actor thread`（left 1、right 0）で、統合検査の失敗と同じ assert・同じ値だった。`script_thread_count_skips_joined_but_listed_thread` も left 1、right 0 で落ちた。holder の無い `script_runtime_daemon_restore_keeps_loaded_modes` は通った。除外を戻した後は 22 件すべて pass。
- 先に C の小さな program で仕組みを確かめた。trace した thread を join した直後の stat は `(celeris-script) state=Z PF_EXITING=1` で、tracer が wait すると消えた。trace しない場合は、負荷の無い host では 2000 回中 0 回しか読めなかった。そのため、負荷による確率的な再現は使っていない。

## 受け入れ条件と証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
| 条件 | コマンド | 結果 |
|---|---|---|
| 0. 原因と決定的な再現 | 上の「原因」「再現」 | 修正前 2 FAIL（`shutdown joined the actor thread` left 1）、修正後 pass |
| 1. 期待を緩めていない | `git diff $CELERIS_WU_BASE -- crates/celeris/tests/script_runtime.rs` | `assert_eq!(script_threads(), 0, …)`・`seen.actors == 1` はそのまま。assert の削除・retry・sleep 待ちは無い |
| 2. 5 回連続 pass | `for i in 1 2 3 4 5; do cargo nextest run -p celeris --test script_runtime \|\| exit 1; done` | exit 0、各回 22 passed |
| 3. 検査 | `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` | 下の「検査」 |

## 検査（提出時の HEAD で実行）
- 検査の結果は報告本文に書く（この file の commit の後に実行する）。

## commit
- `phase s5-s6: fix-script-thread-flake: shutdown 後の thread の数え方から終わりかけの thread を除き、ptrace の遅延フックで再現する`

## 未解決
- `ExitHolder` は ptrace が使える環境を前提にする（Yama の ptrace_scope 0・1 で動く。この run の worker の sandbox（ptrace_scope 1）で動くことは確かめた。release gate の host では未確認）。ptrace_scope 2・3 の host では、2 件の再現試験が `exit-holder: seize failed` で落ちる。

## 提案
- なし。
