---
title: K2 の grade-case を EVAL_ROOT で作って採点に使う経路、人の sudo と L1 probe の食い違いの直し
tasks: [01M673C32XMMPZ83Z6WK818WZV]
status: done
updated: 2026-10-30
---
# K2 の grade-case を EVAL_ROOT で作って採点に使う経路、人の sudo と L1 probe の食い違いの直し

## やったこと
- ADR 2026-10-30-ops-r2-isolation-principal に付記 G1〜G6 を足した（設計を先に書いた）。
- G1 `celeris-campaign gen-grade-case`（`crates/campaign/src/grade_case_gen.rs` の `cli`、`grade_case_gen::generate` を再利用）。
  入力は `<private-root>/final/<case>/private.json` と隠しテスト `<hidden-root>/final/<case>/hidden/`（`.grader/run.sh` があれば wrapper）。
  出力は `<out-root>/final/<case>/`、stdout は `<case>.json` の sha256 の 1 行だけ。誤りは固定文で、private の command・path を出さない。
  隠しテストが無い・空・symlink や特殊 file を含むときは拒否（hidden_command だけの grade-case は作らない）。出力 dir が既にあれば拒否。
  試験 `crates/campaign/tests/k_cases_grade.rs::gen_grade_case_cli_prints_only_sha_and_refuses_without_hidden`（fixture の private と hidden だけを使う）。
- G2 `scripts/campaign/ops-r2-grade.py` が `--grade-case-root EVAL_ROOT/grade-cases` を固定で付け、引数の `--grade-case-root`（`=` 形も）を exit 64 で拒む。試験は `scripts/campaign/test_ops_r2_grade.py`。
- G3 `scripts/campaign/ops-r2-set-private-sha.py --sha256 <private> --grade-case-sha256 <grade-case>` で `case.json` の 2 つの sha256 を替える。
  中身は読まない。漏洩済みの旧 private の値と repo の旧 grade-case の値を拒否。`Case::public_json` は `private_ref`・`grade_case_ref` を除くので manifest の再 freeze は要らない（script が公開部の不変を確かめる）。試験 2 件を追加。
- G4 repo の `crates/campaign/fixtures/grade-cases/final/k2-bf-xattr-restart-campaign/` 一式は漏洩済みの旧解答として HEAD に残し（fixture の試験が使う）、
  deny list の最低限（`docs/ops/ops-r2-k2-preflight.md` §3、`docs/ops/ops-r2-isolation.md` §4.5）と開始前チェック §0 の `grade_case_ref` 照合の行に載せた。
- G5 人の uid の `(ALL) NOPASSWD: ALL` による L1 の限界を ADR と運用文書 §4.4 に書き、旧い「sudo -n -u celeris-eval /bin/cat が拒否される」確認を消した。bwrap の中で sudo が "no new privileges" で失敗することは実機確認済みと改めた。
- G6 `probe-read --principal l1` は `mounts_outside_allow` を報告に残すが判定に入れない（`crates/campaign/src/runtime_probe.rs`）。§4.7 の期待（exit 0）と一致。試験 `crates/campaign/tests/runtime_probe.rs::l1_principal_reports_mounts_but_does_not_fail_on_them`。

## 証拠（提出時の HEAD で実行、基準は `git merge-base HEAD main`）
- `bash scripts/dev/test-affected.sh` → exit 0、nextest 2894 件 passed・9 skipped、`progress_index_commit_hashes: all checks passed`
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0
- `python3 -I scripts/campaign/test_ops_r2_grade.py` → 13 件 OK

## 本番への移行（operator が評価器 uid の shell で行う。順に）
1. 隠しテスト file を `/var/lib/celeris-eval/hidden-tests/final/k2-bf-xattr-restart-campaign/hidden/` の下に置く（相対 path がそのまま作業 dir に挿入される。wrapper にするなら `.grader/run.sh`）。持ち主 `celeris-eval`。
2. この branch の build の `celeris-campaign` で `celeris-campaign gen-grade-case --case k2-bf-xattr-restart-campaign --cases-root <repo>/crates/campaign/cases --private-root /var/lib/celeris-eval/private --hidden-root /var/lib/celeris-eval/hidden-tests --out-root /var/lib/celeris-eval/grade-cases` を実行し、stdout の sha256 を人に渡す（中身は見せない）。
3. 人が private と grade-case の 2 つの sha256 を承認票に書き、repo で `python3 -I scripts/campaign/ops-r2-set-private-sha.py --case k2-bf-xattr-restart-campaign --sha256 <private の値> --grade-case-sha256 <grade-case の値>` を実行して `case.json` を替え、commit する（値は commit message に書かない）。
4. `/var/lib/celeris-eval-bin/grade` を新しい `scripts/campaign/ops-r2-grade.py` に、`/var/lib/celeris-eval-bin/celeris-campaign` をこの branch の build に入れ替える（`celeris-eval` 持ち 0555）。`sha256sum` で repo の file・build と一致することを確かめる。
5. `docs/ops/ops-r2-isolation.md` §4.4（bwrap の中から sudo が失敗する）・§4.7（L1 probe が exit 0）と `docs/ops/ops-r2-k2-preflight.md` §0・§3 を通す。

## 未解決事項
- 隠しテスト file の中身（新しい private の expected の仕様に沿うもの）は評価器側が repo の外で作る。まだ無い。
- 実 uid（celeris-eval）での `gen-grade-case`・採点の通しは人が上の手順で確かめる（この task では EVAL_ROOT を読んでいない）。

## 提案
- なし
