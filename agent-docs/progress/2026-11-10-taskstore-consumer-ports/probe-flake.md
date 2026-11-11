---
title: 健全性v2 R3 probe-flake — campaign の preflight が自分の binary を写す元を、path の置き換えで消えない元にする
tasks: [01M6TYVJT2HG9NY5FW9FMCZS9A]
status: done
updated: 2026-11-11
---
# 健全性v2 R3 probe-flake — campaign の preflight が自分の binary を写す元を、path の置き換えで消えない元にする

完了日: 2026-11-11

基準は `$CELERIS_WU_BASE`（この WU の base commit）。人の決定 probe-scope（a: R3 の leaf probe-flake で直す。直す file は `crates/campaign/src/preflight_isolation.rs` と `crates/campaign/tests/` だけ）に従った。B0 の `slices.json` は書き換えていない。

## 結論
段 org-port の統合で `celeris-campaign::arm_set_cli_normal` の `ab_only_counts_repetitions_retries_grading_and_cost_and_resumes_without_reposting` が落ちた原因は、R3 の変更ではなく次の競合だった。
1. `crates/celeris/tests/ops_r1_pipeline.rs` の `campaign_bin()` が、試験の途中で同じ `CARGO_TARGET_DIR` に `cargo build -p celeris-campaign --bin celeris-campaign` を走らせる。
2. この build は試験の build と依存の feature・profile が違うので作り直しになり、`target/debug/celeris-campaign` を消して新しい file を置く（inode が替わる）。
3. その間に走っていた `celeris-campaign preflight` では、`std::env::current_exe()` が `<path> (deleted)` を返す。`preflight_isolation::run` がそこから `fs::copy` すると ENOENT になり、A・B・operator の 3 件が `deny_readable:<主体>:probe:stage probe: No such file or directory (os error 2)` になる。

`preflight_isolation.rs` の写し元を `/proc/self/exe`（定数 `SELF_EXE`）に替えて直した。

## 原因の証拠
- 統合の log（`integration-checks/integrate-org-port/` の test-affected の log）の要点:
  - `FAIL [  45.779s] (1048/8198) celeris-campaign::arm_set_cli_normal ab_only_counts_repetitions_retries_grading_and_cost_and_resumes_without_reposting`
  - missing の詳細は `deny_readable:A:probe:stage probe: No such file or directory (os error 2)`、同じ文で B・operator。L1（包まない主体。写さない）は出ていない。
  - 同じ run で `SLOW [  69.782s] (1051/8198) celeris::ops_r1_pipeline ops_r1_pipeline_passing_a_and_cut_off_b_agree` と `SLOW [  71.548s] (1053/8198) celeris::ops_r1_pipeline ops_r1_pipeline_all_stages_agree_and_two_passes_are_byte_identical` が走っていた。開始の番号が近く、どちらも 45 秒より長いので、2 つの試験は同時に走っていた。
  - `Summary [ 625.249s] 8198 tests run: 8197 passed (26 slow), 1 failed, 23 skipped`
- `crates/celeris/tests/ops_r1_pipeline.rs` の `campaign_bin()` の行（コメント「古い binary を使わないよう毎回 build する（新しければ何もしない）」）:
  `Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).args(["build", "-p", "celeris-campaign", "--bin", "celeris-campaign"])`
- 「新しければ何もしない」は成り立たない。この WU の target で `cargo nextest run -p celeris-campaign --test preflight_deny_file` の後に `cargo build -p celeris-campaign --bin celeris-campaign` を走らせると、task-core・task-ops・task-worker・task-dispatch・celeris-campaign などを compile し直し（`Finished dev profile … in 1m 32s`）、`target/debug/celeris-campaign` の inode が替わった（build の前後で `stat -c %i` の値が違う）。
- 決定的な再現（下の試験 `probe_stage_copies_the_running_binary_after_its_path_is_replaced`）: 直す前の実装で、統合の log と同じ 3 件の詳細が出て落ちた。置き換えの後の `/proc/<子の pid>/exe` は ` (deleted)` で終わることも、試験の中で確かめている。

## 変更
- `crates/campaign/src/preflight_isolation.rs`:
  - `pub const SELF_EXE: &str = "/proc/self/exe"` を置き、`requests` は `--probe-program` が無いとき `PathBuf::from(SELF_EXE)` を写し元にする。kernel が実行ファイルとして持つ file を開くので、path が消えても置き換わっても、走っている binary そのものを読める。
  - 旧い経路（`std::env::current_exe()` の path を写し元にする分岐と、その失敗を `deny_readable:probe binary: …` にする分岐）を消した。新旧の切替や fallback は置いていない。
  - fail-closed は変わらない。`/proc/self/exe` を開けなければ、A・B・operator の各要求が `stage probe: …` で失敗し、`deny_readable` の miss になる。`deny_readable` の判定（読めた path・一覧できた親・許可表の外の mount）は変えていない。`--probe-program` を渡したときの挙動も変えていない。
- `crates/campaign/tests/preflight_deny_file.rs`: 試験 `probe_stage_copies_the_running_binary_after_its_path_is_replaced` と、その部品 `sh_write`・`stage_recorder` を足した。

## 試験を足した理由
- 守る不変条件: `--probe-program` が無いとき、A・B・operator の包みに写す probe は、preflight を走らせている process の実行ファイルそのもの。起動した path が試験中の別の build で置き換わっても、stage は ENOENT にならず、置き換え後の別の file も写さない。
- 既存の試験で検出できない理由: 既存の CLI 試験は、走っている binary の path を置き換えない。置き換えは `ops_r1_pipeline` の build と時刻が重なったときだけ起き、R3 の段の統合では 3 回のうち 1 回（段 org-port）だけ落ちた。
- 再現の手順（時間・負荷に頼らない）:
  1. `CARGO_BIN_EXE_celeris-campaign` を一時 dir に写し、その path から `preflight` を起動する（frozen の `ops-r2-k2.json`。A・B が有効なので A・B・operator・L1 の 4 主体の要求が組まれる）。
  2. `--deny-list` は FIFO にする。`requests` は deny list を読み終えてから写し元を決めるので、子は写し元を決める前に FIFO の読みで止まる。試験の書き手の open が返った時点で、子が FIFO を開いたことが分かる。
  3. 試験は cargo と同じく path の file を消して別の file（`exit 97` の sh script）を置く。`/proc/<pid>/exe` が ` (deleted)` で終わることを確かめてから、deny list を書いて閉じる。
  4. `--sandbox-script` と PATH 先頭の `bwrap` は記録用の script で、`--` の後の staged probe の sha256 を主体ごとに書いてから probe を起動する。`--a-program` は版を返すだけの偽物で、host の claude は起動しない。
  5. 判定: 出力に `stage probe` が無いこと。A・B・operator の staged probe の sha256 が、元の binary の sha256 と一致すること（置き換え後の file を写したら一致しない）。
  - 出来事待ちは channel（子の終了・FIFO の open）で、保険の上限は 300 秒。CPU に負荷はかけない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 直す前（`preflight_isolation.rs` だけ base に戻して実行）: `cargo nextest run -p celeris-campaign --test preflight_deny_file -E 'test(probe_stage_)'` → exit 100。`1 test run: 0 passed, 1 failed`。panic は `assert!(!t.contains("stage probe"))` で、missing は `deny_readable:A:probe:stage probe: No such file or directory (os error 2)`・B・operator の 3 件（統合の log と同じ文）。
- 直した後: `cargo nextest run -p celeris-campaign --test preflight_deny_file` → exit 0、`4 tests run: 4 passed`。
- 残りの検査の結果は「## 検査の結果」に書く。

## 未解決
- 無し（この WU の範囲では）。下の提案の扱いは R5 との調整が要る。

## 提案
- `crates/celeris/tests/ops_r1_pipeline.rs` の `campaign_bin()` が試験中に共有の `CARGO_TARGET_DIR` へ `cargo build -p celeris-campaign` を走らせることは、それ自体が並行する試験を乱す。今回の直しで campaign の preflight は binary の置き換えに耐えるが、同じ path の binary を起動する他の試験（`arm_set_cli_normal` ほかの `CARGO_BIN_EXE_celeris-campaign` の利用者）は、置き換えの瞬間に path が無い（cargo は消してから link する）と起動に失敗しうる。さらにこの build は試験の build と feature が違うので、試験のたびに依存ごと compile し直し（この WU の計測で 1 分 32 秒）、次の試験の build でまた作り直しになる。
  - 案: `ops_r1_pipeline` を celeris-campaign の側の試験（`CARGO_BIN_EXE_celeris-campaign` が引ける crate）へ移すか、celeris の dev-dependency の artifact dependency を使うか、試験の前に build を済ませる仕組み（test-affected・release gate の側）に寄せて、試験の中で cargo を呼ばない。
  - 扱い: R5（test target の束ね）が celeris の test target を動かすので、R5 と同時に変えると衝突する。R5 の後の別 task にするか、R5 の束ねの中で一緒に直すかを決める必要がある。この WU の範囲（`crates/campaign/` だけ）の外なので、ここでは直していない。
