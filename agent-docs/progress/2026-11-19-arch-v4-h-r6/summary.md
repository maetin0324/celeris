---
title: アーキ v4 H R6 summary — gate・verify・promoted の要約を $SD_LOGS/release-records.jsonl に追記し、掃除の後も残る試験を足す
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# summary（WorkUnit）

完了日: 2026-11-20

## 結論

- [ADR 2026-11-20 release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md) の D2〜D5 を実装した（前回の run）。
  `scripts/selfdeploy/lib.sh` の `sd_release_record <gate|verify|promoted> <正本の path>` が正本の file から要約を 1 行の JSON にして
  `$SD_LOGS/release-records.jsonl` に `>>` で追記する。呼び出しは release.sh 3 か所・verify.sh 1 か所・promote.sh 1 か所。
- この run で次の 4 つを仕上げた（計画の直しで範囲 check に 7 本の試験・`scripts/dev/test-release-script-gate.sh`・`fixtures/` が入った）。
  1. 既存の試験 7 本の `unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR` を保ち、理由を 1 行の注釈にした。
  2. `scripts/dev/test-release-script-gate.sh` の `run_release` の `env` に `-u CELERIS_LOGS_DIR -u CELERIS_BACKUPS_DIR` を足した。
     偽の木に `check-slot.sh`・`test-tmp-lifecycle.sh`・`scripts/selfdeploy/*.py`・`cos-skills.txt`・`repo_health.py` を写し、
     R6 の前から落ちていたこの試験が exit 0 になった。
  3. `browser_ledger_release_stages.sh`（偽の木 2 か所）と `release_web_bundle_requires_deps.sh` の偽の木に、
     `release_web_required.sh` と同じ 2 行（`check-slot.sh` の写しと空の `cos-skills.txt`）を足し、R6 の前から落ちていた 2 本が exit 0 になった。
     原因は計画の見立てのとおりで、別の原因は無かった。
  4. 偽の cargo・pnpm・node・celerisctl と偽の木の組み立て・`new_commit` を `scripts/selfdeploy/tests/fixtures/release-gate-tree.sh`
     に移した。`release_parallel_test_gate.sh` と `release_record_summary_survives_prune.sh` がこれを source する。偽の cargo は
     `release_parallel_test_gate.sh` の版で、2 本の期待値は変えていない（新しい試験は fixture の cargo のために `CARGO_LOG` を渡す）。
- `crates/`・policy の file は変えていない。本番の release.sh・verify.sh・promote.sh は走らせていない。要約の log を読む経路は無い。

## commit（件名）

- 「summary: gate・verify・promoted の要約を release-records.jsonl に追記し、掃除の後も残る試験を足す」
- 「summary: 要約の行を書かない既存の試験 4 本の unset を戻し、範囲 check の不合格を記録する」
- 「summary: release の試験の偽の道具と偽の木を fixtures/release-gate-tree.sh に寄せ、継いだ本番の log に書かないことを固める」
- 「summary: 進捗 summary.md を done にする」

## 実装前 brief（ADR 1 節の要約）

1. 根本原因: gate.json・verify.json・promoted.json は release dir の中にしか無く、`prune_releases` が dir ごと消す。
2. 責務 owner: 3 つの file を書く場所（release.sh・verify.sh・promote.sh）。要約を作って追記する関数は lib.sh に 1 つだけ置く。
3. 既存の再利用先: `SD_LOGS`・`sd_ts`・`sd_log`・python3（promote の log と同じ置き場）。試験の偽の木は共有の fixture。
4. 選択肢: 追記の log を採用した（B1）。3 file の写しを残す案（B2）、掃除で残す数を増やす案（B3）、DB に書く案（B4）は採らない（ADR 1.4）。
5. 新しい状態・分岐: 追記だけの写しの file を 1 つと、失敗を警告だけにする分岐を足した。flag・fallback・port は足していない。
6. 撤去対象: 2 本の試験が持っていた偽の道具と偽の木の写し（共有の fixture に置き換えた）。
7. 不変条件: `unpromoted-release-no-takeover`（要約の log を判断に読まない）と、追記で台本の結果を変えないこと。どちらも新しい試験の場合 6・7 で確かめる。

## (1) 7 本の unset と注釈

対象: browser_ledger_release_stages・release_parallel_test_gate・release_prune_stale_test_binaries・release_uses_scratch_lease・
release_web_bundle_requires_deps・release_web_required・verify_web_app_start。冒頭の 2 行は次のとおり（7 本とも同じ）。

```bash
# 理由: worker の環境は本番の CELERIS_LOGS_DIR を継ぎ、継いだままだと台本が偽の要約の行を本番の release-records.jsonl に足す。
unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR
```

unset すると `$SD_LOGS` は試験の一時の state の下（`backups/`）になる。前回の測定（base の版の試験を一時の log dir で走らせた）では、
この 7 本が 1〜6 行を書き、ほかの release の試験は 0 行だった。

## (4) 共有の fixture（`scripts/selfdeploy/tests/fixtures/release-gate-tree.sh`）

- source の前に `root` を決める。source すると `$root/bin/{cargo,pnpm,node,celerisctl}`・`$repo`（commit の無い git の木）・
  `new_commit`・`SD_SCRIPT_BUNDLE_GATE`・`SD_WEB_PNPM` ができる。偽の道具が読む env（`CARGO_LOG` は必須、`FAKE_SCRATCH`、
  `FAKE_NEXTEST_FAIL` など）は file の冒頭に書いた。
- 偽の木は `scripts/selfdeploy/*.sh`・`*.py`、空の `cos-skills.txt`、`test-parallel.sh`・`source-size-report.py`、
  `lib/test-tmp-lifecycle.sh`・`lib/check-slot.sh`、`repo-health/repo_health.py`、`tools/nextest/VERSION` を写す。
- 行数（`wc -l`）: release_parallel_test_gate.sh 246 → 135、release_record_summary_survives_prune.sh 259 → 178、fixture 135。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

どの command も、先に `export CELERIS_LOGS_DIR=$(mktemp -d) CELERIS_BACKUPS_DIR=$(mktemp -d)` をしてから走らせた。

- 計画の check 2（`scripts/selfdeploy/tests/*.sh` と `scripts/dev/test-release-script-gate.sh` のうち `(release|verify|promote)\.sh` を含む
  22 本を、`CELERIS_LOGS_DIR`・`CELERIS_BACKUPS_DIR` を空の一時 dir に向けて走らせ、その dir に何も書かれないかを `find` で見る）
  → exit 0（199 秒）。22 本とも exit 0（新しい試験・release_parallel_test_gate・browser_ledger_release_stages・
  release_web_bundle_requires_deps・test-release-script-gate を含む）。一時 dir に書かれた file は 0 件。
- 上に入らない selfdeploy の試験 12 本（`for f in scripts/selfdeploy/tests/*.sh`、grep に合わないもの）→ 12 本とも exit 0。
  browser_ledger_credential_evidence も exit 0 になった（中で browser_ledger_release_stages.sh を exec する）。
- `bash scripts/selfdeploy/tests/release_record_summary_survives_prune.sh` → exit 0、`release_record_summary_survives_prune: ok`。
- `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` → exit 0、`release_parallel_test_gate: ok`。
- `bash scripts/dev/test-release-script-gate.sh` → exit 0、`test-release-script-gate: ok`（本物の node・pnpm の (C) も ok）。
- `ls /local/celeris/state/logs/release-records.jsonl` → 「No such file or directory」（試験が本番の log に書いていない）。
- `git diff --name-only $CELERIS_WU_BASE HEAD -- crates/` → 空。`grep -rn release-records crates/` → 0 件。
- `bash scripts/dev/test-affected.sh` → exit 0（`test-affected: 試験対象の変更なし（docs のみ等）`。python の試験 126 件 OK、
  repo_health_gate の 7 件 OK）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash -n` で fixture と `scripts/selfdeploy/tests/*.sh`・`test-release-script-gate.sh` の構文 → ok。shellcheck はこの host に無く、走らせていない。
- 前回の run の反例（台本を壊した写しの木）: 通った gate の `sd_release_record` を消すと新しい試験が exit 1、lib.sh の `>>` を `>` にすると exit 1。

## 本番への移行

- config・API・CLI・DB の変更は無い。人の作業は要らない。
- この変更を含む release の後、`release.sh`・`verify.sh` を回すと `/local/celeris/state/logs/release-records.jsonl`（`paths.env` の
  `CELERIS_LOGS_DIR`）に行が増える。web の昇格（`POST /promote`）は `current` に同梱の promote.sh を走らせるので、promoted の行は
  この変更を含む版が `current` になった後の昇格から増える。
- 確かめ方（人が実行する。任意）: 次の release の後に `tail -n 3 /local/celeris/state/logs/release-records.jsonl`。
- 掃除で既に消えた release の記録は戻らない。

## 未解決

- `browser_ledger_default_target.sh` は、継いだ `CELERIS_LOGS_DIR` に `browser-ledger-<sha12>-<時刻>.log` を 1 件書く（`browser-ledger.sh`
  の log。release-records とは別）。この試験も `browser-ledger.sh` もこの WorkUnit では変えておらず、R6 の前からの挙動。
  計画の check 2 の対象（release・verify・promote の台本を起こす試験）には入らない。

## 提案

- 残りの偽の木の写しを共有の fixture `scripts/selfdeploy/tests/fixtures/release-gate-tree.sh` に寄せる後続の task を起こす。対象は
  release_prune_stale_test_binaries・release_uses_scratch_lease・release_web_required・browser_ledger_release_stages・
  release_web_bundle_requires_deps と `scripts/dev/test-release-script-gate.sh`。どれも release.sh・test-parallel.sh が読む file の
  一覧を別々に持ち、今回の `check-slot.sh`・`cos-skills.txt` のように、file が増えるたびに一部だけが古くなって落ちる。
  偽の cargo は試験ごとに出力が違う（web の段・credential・bundle の env など）ので、fixture の cargo に env の切り替えを足すか、
  試験の側で上書きする形にする。browser_ledger_release_stages.sh は同じ偽の木を 2 回組み立てているので、寄せるときに 1 つにする。
- `browser_ledger_default_target.sh` の冒頭でも `unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR` をする小さな task を起こす（上の「未解決」）。
