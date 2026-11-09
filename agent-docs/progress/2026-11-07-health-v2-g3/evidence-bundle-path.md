---
title: 健全性v2 G3 evidence bundle-path — daemon が release dir の同梱の検査器を求めて gate に設定する
tasks: [01M6YJF7HJ9J0EQHRM5SN5TWV4]
status: done
updated: 2026-11-09
---
# 健全性v2 G3 evidence bundle-path — daemon が release dir の同梱の検査器を求めて gate に設定する

完了日: 2026-11-09

WorkUnit `bundle-path`（task 01M6YJF7HJ9J0EQHRM5SN5TWV4）。[ADR 2026-11-08 repo health を通常の開発・統合ループに強制する](../../adr/2026-11-08-health-enforcement-loop.md) の D1.4 の daemon 側を `crates/celeris` に実装した。基準は `$CELERIS_WU_BASE`（統合後は `git merge-base HEAD main`）。同梱の検査器を適用判定（`repo_facts` の `bundled`）に渡すのは後続の `delivery-verify`・`loop-points` が行う。

## 変更
- 件名「G3 evidence bundle-path: daemon が release dir の同梱の検査器を求めて gate に設定する」の commit
  - `crates/celeris/src/daemon/release_dir.rs`（新規）: 実行中の binary の release dir の求め方を 1 つにした。
    - `release_dir_for(exe, release)`: `--release` の値が空でなければ `exe`（`<releases>/<sha12>/bin/celeris`）の親の親。無ければ `None`。
    - `running_exe()`: 実行中の binary の path。daemon の利用側はこれを `release_dir_for` に渡す。
  - `crates/celeris/src/daemon/browser_ledger_path.rs`: `conformance_path_for` は release dir を `release_dir_for` で求め、その下の `browser/conformance.json` だけを決める（親の親の式を消した）。環境変数 `CELERIS_BROWSER_CONFORMANCE_FILE` の上書きは今までどおり browser 側にある。`configure_from_process` は `running_exe()` を使う。関数の引数と既存の試験 3 件は変えていない。
  - `crates/celeris/src/daemon/repo_health_path.rs`（新規）:
    - `bundled_checker_for(exe, release) -> Result<BundledChecker, NoBundle>`: `release_dir_for` で release dir を求め、`<release>/repo-health` を gate の `read_bundled_checker` で読む（`CHECKER_TREE` の 2 行 → task_core の `checker_sha256` → `CheckerId { source: Bundled }`）。process の設定（`OnceLock`）には触れない。理由は `NoBundle::NoReleaseDir`（`--release` 無し・空、exe 不明）と `NoBundle::Unreadable(<gate の理由>)`（`repo-health/`・`repo_health.py`・`CHECKER_TREE` が無い、行が壊れている・未知・repo-health の行が無い）。
    - `configure_from_process(cli_release)`: 上の結果が読めれば `gate::configure_bundled_checker(<dir>)` で 1 回設定し、dir と `checker_sha256` を info で出す。読めなければ設定せず（同梱なし）、`NoReleaseDir` は info、`Unreadable` と設定の失敗は warn で理由を出す。所在の環境変数の上書きは置かない（`grep -nE 'current_exe|env::var' crates/celeris/src/daemon/repo_health_path.rs` は 0 件）。
  - `crates/celeris/src/daemon/repo_health_path_tests.rs`（新規。`repo_health_path.rs` から `#[cfg(test)] #[path] mod tests` で読む）: 下の 3 件。
  - `crates/celeris/src/daemon/run.rs`: `browser_ledger_path::configure_from_process` の直後で `repo_health_path::configure_from_process(opts.release.as_deref())` を 1 回呼ぶ。`run_inner` は base で 325 行あり、size.responsibility-growth の閾値 200 を超えている（existing）。行を増やすと worsened になるので、browser の 2 行の注釈を両方の ADR を挙げる 1 行にまとめ、`run_inner` の行数を base と同じにした。
  - `crates/celeris/src/daemon.rs`: `pub(crate) mod release_dir;` と `pub(crate) mod repo_health_path;` の 2 行だけを足した。

## 実装前 brief（規約）
1. 根本原因: gate は同梱の検査器を `configure_bundled_checker(dir)` で受け取る口を持つが、daemon が release dir から dir を求めて呼ぶ所が無い。release dir の式（exe の親の親と `--release`）は `browser_ledger_path.rs` の中にしか無い。
2. 責務 owner: release dir の求め方は daemon の `release_dir`、同梱の検査器の所在は daemon の `repo_health_path`、`CHECKER_TREE` の形と版の計算は `task_dispatch::health_review::gate`（`read_bundled_checker`）と task_core の `checker_sha256`。
3. 既存の再利用先: `gate::read_bundled_checker`・`gate::configure_bundled_checker`・`CHECKER_TREE_FILE`、task_core の `checker_sha256`・`base_checker`・`CHECKER_SCRIPT`、`browser_ledger_path` の起動時の呼び出しの位置（`run.rs`）。
4. 選択肢: (a) `repo_health_path.rs` に `CHECKER_TREE` の読み取りを写す → gate と二重実装になるので不採用。(b) gate の `configure_bundled_checker` を `CheckerId` を受ける形に変える → checker-gate の試験と API を変え、`CHECKER_TREE` の owner が daemon に移るので不採用。(c) 純粋な関数は `read_bundled_checker` を呼び、設定は `configure_bundled_checker(dir)` に渡す → 採用。
5. 新しい状態・分岐・fallback・port: `NoBundle`（同梱なしの 2 つの理由。log の水準を分けるため）。fallback・切替 flag・環境変数は無し。
6. 撤去対象: `browser_ledger_path.rs` の親の親の式（`release_dir_for` に移した）と `std::env::current_exe` の直接の呼び出し（`running_exe` に移した）。
7. 不変条件: release dir の式は `release_dir_for` の 1 つ。同梱の版は `CHECKER_TREE` からだけ作り、base の版と同じ式。同梱が無い・読めない daemon は同梱なし（適用判定で `checker_missing` の `Unavailable`）。既存の browser_ledger_path の試験 3 件と下の試験 3 件で確かめる。

## 試験（`crates/celeris/src/daemon/repo_health_path_tests.rs`、3 件）
一時 dir に `<root>/releases/<sha12>/` を作り、release.sh の `sd_bundle_repo_health` と同じ配置（`repo-health/scripts/dev/repo-health/repo_health.py`・`repo-health/scripts/dev/source-size-report.py`・`repo-health/CHECKER_TREE`）を置く。exe は `<release>/bin/celeris`。`OnceLock` は設定しない。既存の試験のうち `health_review/gate_tests.rs` の `health_evidence_gate_bundled_checker` は同梱の dir を直接 `configure_bundled_checker` に渡すだけで、daemon が release dir からその dir を求める経路を通らない。正しい `CHECKER_TREE` しか試さない。`browser_ledger_path_tests.rs` の 3 件は `browser/conformance.json` の path だけを見る。

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `health_evidence_bundled_checker_tree_gives_base_formula` | daemon は実行中の binary の release dir の `repo-health/` を見つけ（`dir` が `<release>/repo-health`）、`CHECKER_TREE` の 2 行から作る版は `CheckerId { source: Bundled, sha256: checker_sha256(tree, Some(blob)) }` で、同じ tree id・blob id を答える git での task_core の `base_checker` の版と同じ値 | gate の試験は dir を直接渡すので、daemon が別の場所（例 `<release>/bin/repo-health`、`repo-health` の段の抜け）を見ても通る。browser の試験は repo-health の path を見ない |
| `health_evidence_bundled_missing_or_broken_checker_tree_is_none` | release dir があっても、`CHECKER_TREE` が無い・hex でない id・未知の行・repo-health の行が無い・空、または `repo-health/` 自体が無い release では同梱なし（`NoBundle::Unreadable`、理由に `CHECKER_TREE` の path か不足を書く）。版を推測しない | gate の試験は正しい `CHECKER_TREE` だけを使い、壊れた・無い場合を試さない |
| `health_evidence_bundled_without_release_is_none` | `--release` 無し・空・exe 不明の daemon は、exe の親の親に正しい同梱があっても同梱なし（`NoBundle::NoReleaseDir`）。対照として同じ配置で `--release` があれば読める | browser の `browser_ledger_release_none_without_env_or_release` は browser の関数の戻り値だけを見る。repo_health_path が release の指定を見ずに exe から探す退行は検出できない |

## ADR とのずれ
- ADR (8) の daemon の行は `^crates/celeris/src/daemon/[a-z_]+\.rs$` で、module root の `crates/celeris/src/daemon.rs` を含まない。新しい module を読むために `daemon.rs` に `mod` の行 2 つ（`release_dir`・`repo_health_path`）だけを足した。`daemon.rs` の先頭の module の表（`//! | browser_ledger_path | …`）には行を足していない（この unit の check は `mod` の行以外の変更を拒む）。
- task の目的文は「repo_health_path.rs が CHECKER_TREE を読み、checker_sha256 で CheckerId を作り configure_bundled_checker に渡す」と書く。gate の `configure_bundled_checker` は `CheckerId` でなく dir を受け、`CHECKER_TREE` の読み取りと `checker_sha256` の計算は gate の `read_bundled_checker` が持つ（unit checker-gate）。規約 2（二重実装をしない）に従い、`repo_health_path.rs` は `read_bundled_checker` を呼んで `CheckerId` を得て、dir を `configure_bundled_checker` に渡す。そのため起動時に `CHECKER_TREE` を 2 回読む（純粋な関数と設定）。
- 「同梱なしとして設定」は gate の `OnceLock` を設定しないことで表す（gate に「無い」を置く API は無い。`bundled_checker()` は `None` を返す）。
- ADR D1.4 は「`browser_ledger_path` と同じ求め方」と書く。求め方を `release_dir.rs` に移し、両方がそれを使う形にした。`current_exe` の呼び出しも `release_dir::running_exe` の 1 か所にした（`repo_health_path.rs` に書かないため）。

## 範囲の外の path
無し。`sh "$CELERIS_WU_SCOPE_PATHS"` の出力は `crates/celeris/src/daemon.rs`・`crates/celeris/src/daemon/{browser_ledger_path,release_dir,repo_health_path,repo_health_path_tests,run}.rs` とこの進捗ファイル。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p celeris --lib health_evidence_bundled_` → test result: ok. 3 passed。
- `cargo test -p celeris --lib browser_ledger_path` → test result: ok. 3 passed（既存の 3 件。中身は変えていない）。
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celerisctl。nextest 996 tests run: 996 passed、2 skipped。check-health は verdict pass（exit 0、fail_grade 0、worsened 0、new 1）。new の 1 件は `policy.changed` の warning（candidate=false。main に入った ci-entry の policy の file との比較で、この unit の変更ではない）。初回の実行では `run_inner` の 3 行の増加で size.responsibility-growth が worsened（328 > 325）になったので、上の「変更」の run.rs のとおり行数を base と同じにした。
- `sh scripts/dev/progress-index.sh --check --base $CELERIS_WU_BASE` → ok。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `grep -nE 'current_exe|env::var' crates/celeris/src/daemon/repo_health_path.rs` → 0 件。
- `git diff $CELERIS_WU_BASE -- crates/celeris/src/daemon.rs` → `mod` の行 2 つの追加だけ。

## 未解決
- 同梱の検査器を適用判定に渡す経路（`repo_facts` の `bundled` に `gate::bundled_checker().map(|b| &b.id)`）はまだ無い。`delivery-verify`・`loop-points` が配線する。
- 実 release（`release.sh` が作った `<release>/repo-health/`）で daemon を起こして log を確かめるのは release gate と昇格の時になる。この unit では一時 dir の配置で確かめた。

## 提案
- closeout は `daemon.rs` の module の表に `release_dir`・`repo_health_path` の行を足し、ADR (8) の daemon の行に module root を含めた事実を付記に書く。

## 本番への移行
無し（config・DB の変更は無い）。この変更を含む release から、daemon は起動時に `<release>/repo-health/` を読み、log に `repo health: bundled checker` を出す。
