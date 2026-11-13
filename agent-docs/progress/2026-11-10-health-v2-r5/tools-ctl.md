---
title: 健全性v2 R5 tools-ctl — measure.sh の run に crate の追記・束の 1 process 実行・base の実行・--require を足し、celerisctl を測る
tasks: [01M76AG298YER7PJB6M781C77X]
status: done
updated: 2026-11-11
completed: 2026-11-11
---
# 健全性v2 R5 tools-ctl

完了日: 2026-11-11

WorkUnit `tools-ctl`（measure-run の最初の leaf）。HEAD はこの WU の base の commit（件名『R5 measure-build: base と HEAD を交互に測る compare を measure.sh に足し、前後の build/link と edit→verify を記録する』）、base は `git merge-base HEAD main`。

## 結論
- celerisctl の試験は HEAD で 2 回とも 205 件すべて合格した（baseline も 205 件）。失敗・時間切れは 0 件。
- 束 2 つ（`browser_eval_surface_suite` 22 件・`unmapped_suite` 2 件）は `cargo test -p celerisctl --test <bundle>` の 1 process の実行でも全件合格した。件数は nextest の同じ binary の件数（`cargo nextest list -p celerisctl`）と同じ。1 process の中の env・CWD・時刻の汚染による失敗は無かった。
- 落ちた試験が無いので、base の独立 target での再実行（原因の切り分け）は要らなかった。bundle-fix に回す celerisctl の target は無い。
- 試験の秒は baseline より長い（nextest の Summary で 3.96・3.61 秒 → 5.96・10.83 秒）。同じ時の load average の 1 分値の最大も 29.00 → 55.14 で、比は 1.901。2 倍以下なので base_run は走らせていない（needed=false）。秒の差が変更によるかは未確認。参考として、確認用の試行（下の「台本の確認」）で base（merge-base）の celerisctl を load 36.89〜37.85 で 1 回走らせた時の Summary は 6.08 秒・205 件合格だった。

## 変更
- 『R5 tools-ctl: measure.sh の run に crate の追記・束の 1 process 実行・base の実行・--require を足し、celerisctl を測る』
  - `measure.sh run` は既存の out に crate を足す。正本の `<out>-raw/steps.jsonl` から測り直す crate の行だけを除いてから測る（前は毎回空にしていた）。out の label と違う label では exit 2 で走らせない。crate ごとに `session`（label・git_head・merge-base・開始時刻・開始時の loadavg・CARGO_TARGET_DIR）を書く。top-level の git_head・loadavg・started_at は最後の呼び出しの値。
  - label が head の run は run2 の後に段 `bundles` を足した。`crates/<crate>/tests/*/main.rs` の束ごとに `cargo test -p <crate> --test <bundle> --no-run` で build を済ませ（`build_secs`）、続けて `cargo test -p <crate> --test <bundle>` を 1 回走らせて exit・passed・failed・ignored・失敗した試験名・秒・loadavg を書く。締め切りで走らせなかった束は skipped の `bundles:<bundle>` に理由を書く。
  - 続けて段 `base_run`。`MEASURE_BASELINE`（既定は同じ dir の baseline.json）の同じ crate の run1・run2 の loadavg_before・after の 1 分値の最大と、この run の同じ値の最大を比べる。比が 2 を超えるか baseline に crate が無ければ、base を `$TMPDIR` の git worktree に作り、`cargo nextest run -p <crate> --no-run` の後に `cargo nextest run -p <crate> --no-fail-fast` を 1 回走らせて秒・件数・失敗名・loadavg・base の sha・比を書き、worktree を消す。要らなければ needed=false・比・両方の値・理由だけを書く。
  - base の worktree の作成と削除は関数 `base_tree_add`・`base_tree_remove` にし、compare と base_run が同じ関数を使う（compare の中の写しは消した）。nextest の log の読み取りも関数 `nextest_result` にして run1・run2 と base_run が使う。
  - label が base でも head でもない run は bundles・base_run を走らせず、skipped に理由を書く。
  - `--validate` は label が base でない run の出力に bundles（見つけた全束の結果か skipped）・base_run（needed=true なら実行の欄も）を求める。baseline.json（label base）と after-build.json（compare）の検証は変えていない。
  - `--require <out.json> <crate,...>` を足した。--validate が通り、label が head で、名指した各 crate に run1・run2・bundles（見つけた全束の結果）・base_run が skipped でなく揃う時だけ exit 0。needed=false の base_run も揃ったと数える。
  - 計測: `MEASURE_DEADLINE`（run の開始から 3000 秒）で `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh run head agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl` を走らせ、`after-run.json` と `after-run-raw/` を書いた。

## 結果（baseline.json の celerisctl と並べた表）
load average は 1 分値（各段の前 → 後）。秒は計測した壁時計、括弧内は nextest の Summary の秒。

| 項目 | baseline（label base、2026-11-10 17:37Z〜） | HEAD（after-run.json、2026-11-11 22:30Z〜） |
|---|---|---|
| test binary（`-p celerisctl`） | 19 | 15 |
| build `--no-run` | 55.60 秒・Compiling 26・link 19 | 48.88 秒・Compiling 3・link 15 |
| edit→verify（--list ＋再 build） | 5.53 秒・link 1 | 6.76 秒・link 1 |
| run1 | 4.78 秒（3.959）・205 件・失敗 0 | 7.37 秒（5.962）・205 件・失敗 0 |
| run1 の load | 29.00 → 27.64 | 55.14 → 52.58 |
| run2 | 4.45 秒（3.605）・205 件・失敗 0 | 13.65 秒（10.832）・205 件・失敗 0 |
| run2 の load | 27.64 → 27.64 | 52.58 → 54.26 |
| run の load の最大 | 29.00 | 55.14（比 1.901） |
| 束 `browser_eval_surface_suite`（cargo test） | （束ねる前） | exit 0・22 passed・0 failed・0 ignored・7.75 秒（build 0.79 秒）・load 52.80 → 52.10 |
| 束 `unmapped_suite`（cargo test） | （束ねる前） | exit 0・2 passed・0 failed・0 ignored・6.62 秒（build 1.37 秒）・load 52.10 → 50.35 |
| base_run | — | needed=false（1.901 ≤ 2） |

- HEAD の build の Compiling 3 は、baseline と違い target の依存がほぼ build 済みの状態から測ったため。build の前後の比較は measure-build（`measure-build.md`）の方法で読む。
- 同じ HEAD で、この記録の前にもう 1 回測った（台本の baseline の path を repo 相対にする前。記録は同じ crate の測り直しで置き換えた）。その回は run1 9.70 秒（8.883）・run2 10.73 秒（9.794）・205 件合格、束は 22 件・2 件合格（4.11 秒・8.17 秒）、load の最大 27.94（比 0.963、needed=false）だった。

## 台本の確認
- `--validate`: baseline.json → exit 0（`ok: 3 crates, 0 skipped item(s)`）、after-build.json → exit 0（`ok: compare 4 crates (both sides built: 4), 1 skipped item(s)`）、after-run.json → exit 0（`ok: 1 crates, 0 skipped item(s)`）。
- `--require after-run.json celerisctl` → exit 0（`required-complete: celerisctl`）。`--require baseline.json celerisctl` → exit 1（label が base、bundles・base_run が無い）。
- 追記: `$TMPDIR` に写した baseline.json と steps.jsonl に `MEASURE_DEADLINE=0` で `run base … celerisctl` を走らせると、task-api・celeris の記録は残り、celerisctl だけが skipped に置き換わった。同じ out に `run head` を走らせると exit 2（label の違い）。本物の after-run.json でも、celerisctl を測り直した後の steps.jsonl は 7 行のまま（重複しない）。
- base_run の実行の分岐: crate の無い baseline（`MEASURE_BASELINE`）を渡し、out を `$TMPDIR` に置いて `run head … celerisctl` を走らせた。base（merge-base）の worktree で `--no-run` 92.78 秒の後、205 件合格（6.08 秒）、`needed: true`・`base_sha` が書かれ、`worktree remove: exit 0`、`git worktree list` に残りは 0。この out は commit していない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/after-run.json` → exit 0
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --require agent-docs/progress/2026-11-10-health-v2-r5/after-run.json celerisctl` → exit 0
- `bash agent-docs/progress/2026-11-10-health-v2-r5/measure.sh --validate agent-docs/progress/2026-11-10-health-v2-r5/baseline.json` → exit 0、`… after-build.json` → exit 0
- `git diff --name-only $CELERIS_WU_BASE | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R5 agent-docs/progress/2026-11-07-health-v2-b0/slices.json` → `write-set R5: ok`
- `bash scripts/dev/test-affected.sh --list` → 対象なし（exit 0。この WU は crates/ を変えていない）
- `cargo fmt --all -- --check` → exit 0
- `cargo clippy --workspace -- -D warnings` → exit 0（80 秒）
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`

## 未解決
- 無し。celerisctl に束ねが原因の失敗は無い。

## 提案
- run-api・run-celeris・run-worker は同じ `after-run.json` に crate を足す。並列の WU が別々の branch で同じ file を書くと、統合で `after-run.json`・`after-run-raw/steps.jsonl` が衝突する。steps.jsonl は crate ごとの行なので両側を残せば解け、after-run.json はその steps.jsonl から組み直す（手で混ぜない）。組み直しの専用の mode は足していない。
