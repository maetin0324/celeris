---
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
---
# ADR 2026-11-20: gate.json の cargo_test に nextest の件数を入れ、gate・verify・昇格の要約を $SD_LOGS/release-records.jsonl に 1 行ずつ追記で残す（アーキv4 H R6）

- 日付: 2026-11-20
- 状態: Accepted（実装前。この ADR を書いた WorkUnit `adr` は `crates/` と `scripts/` を変えていない。D1 は WorkUnit `counts`、
  D2〜D5 は `summary`、全体の検査と進捗の完了は `close` が行う）
- task: 01M7VJY8Q1MMAH87CSPQZWC7VY（アーキv4 H 01M7MZTKBPZPQE4XN451F8X84F の子、人の決定 unmet b の R6）
- 進捗: [2026-11-19-arch-v4-h-r6.md](../progress/2026-11-19-arch-v4-h-r6.md)。WorkUnit ごとの記録は同じ名の dir の `<unit key>.md`
- 材料: アーキv4 H の [final-report.md](../progress/2026-11-19-arch-v4-h/final-report.md) の「提案」の 9・10、
  [safety.md](../progress/2026-11-19-arch-v4-h/safety.md) の「release gate」節
- 関係: [ADR-0040](0040-self-improvement-deploy.md)（release・verify・promote の台本と release dir）、
  [ADR-0041](0041-self-improvement-loop-hardening.md) D3（promoted.json）・§8（並列の試験 runner）、
  [ADR-0058](0058-release-verify-breakdown.md)（gate.json の steps・verify.json の checks）、
  [ADR-0136](0136-local-hot-data-layout.md)（`CELERIS_LOGS_DIR` → `SD_LOGS`）、
  [ADR 2026-10-27 deployment-audit](2026-10-27-deployment-audit.md)（`sd_audit finish` が promoted.json を書く）、
  [ADR 2026-10-04 release-notes](2026-10-04-release-notes.md)、
  [ADR 2026-11-12 host-check-slots](2026-11-12-host-check-slots.md)（test-parallel.sh が読む `check-slot.sh`）、
  [ADR 2026-11-11 cos-skill-sync](2026-11-11-cos-skill-sync.md)（release.sh が読む `cos-skills.txt`）、
  [ADR 2026-10-20 full-tests-at-release-only](2026-10-20-full-tests-at-release-only.md)、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 行番号は、この ADR を書いた時点の HEAD（WorkUnit の base `$CELERIS_WU_BASE` と同じ内容。件名「integrate wu/r7-rules (phase fix1)」）の行。
- 範囲: `scripts/selfdeploy/` は高 risk の path class `selfdeploy`（不変条件 `unpromoted-release-no-takeover`）なので、この ADR を設計 brief とする。
  policy の file は変えない。`crates/` は変えない。本番の `release.sh`・`verify.sh`・`promote.sh` は走らせない（試験は一時 dir の fixture だけ）。
  API・CLI・DB の契約は変えない（gate.json は欄を足すだけ。`GET /releases` の応答の形は変えない）。

## 1. 実装前 brief

### 1.1 根本原因

**(a) gate.json の `cargo_test.passed` が nextest の通過数を含まない。**
`scripts/dev/test-parallel.sh` の集計の python（209〜221 行）は nextest の Summary 行の `:` の後を
`re.findall(r"(\d+) ([a-z ]+?)(?:,|$)", tail)` で割る。本物の nextest は遅い試験があると `8525 passed (3 slow), 22 skipped` と書く。
`passed` の後の ` (3 slow)` は `[a-z ]` にも `,` にも合わないので `8525 passed` の組が取れず、`(3 slow)` も `)` で合わない。
取れるのは `22 skipped` だけで、`npass` は 0 のまま。238〜240 行が `passed = npass + dpass`・`ignored = nskip + dign` と和を取るので、
gate.json の `cargo_test` は `passed` = doc test の通過数、`ignored` = nextest の skipped と doc test の ignored の和になる。

再現（python 3、この ADR を書いた時点の regex をそのまま当てた）:

```
>>> import re
>>> s = "Summary [ 812.345s] 8547 tests run: 8525 passed (3 slow), 22 skipped"
>>> re.findall(r"(\d+) ([a-z ]+?)(?:,|$)", s.split(":", 1)[1])
[('22', 'skipped')]
>>> s2 = "Summary [   0.020s] 7 tests run: 7 passed, 2 skipped"
>>> re.findall(r"(\d+) ([a-z ]+?)(?:,|$)", s2.split(":", 1)[1])
[('7', 'passed'), ('2', 'skipped')]
```

本番の 4 release（1c69c955f0fb・bb772ffca8ec・7014d24222c4・1680de5cd1b4）の gate.json は `passed` 3・`ignored` 24 で、
doc test の通過 3 と、nextest の skipped 22 と doc test の ignored 2 の和に一致する（safety.md の「気づいた点」）。
nextest の件数（8515〜8525 passed）は `gate-logs/cargo-test.log` の Summary 行にだけある。

この誤りを試験が見逃した理由: `scripts/selfdeploy/tests/release_parallel_test_gate.sh` の偽の cargo（46・50 行）は
`7 passed, 2 skipped` の形だけを出し、`(N slow)` の注記の付いた Summary 行を出さない。158 行の dict の一致はこの形でしか試していない。

**(b) release dir の掃除で gate・verify・昇格の記録が消える。**
`scripts/selfdeploy/release.sh` の `prune_releases`（641〜668 行）は current・previous・今回の sha・検証済みの新しい 3 件だけを残し、
他の release dir を `rm -rf` する。gate.json・verify.json・promoted.json は release dir の中にしか無いので、一緒に消える。
gate が落ちた release の `.build/<sha12>/gate.json` も残る保証が無い（H の時点で 2026-11-17 以降の sha の dir は無かった）。
H は B・C・D を最初に含んだ 3 release（48b700ede57f・f85df0209504・eb1244bb5e8d）の gate と verify を記録で確かめられず、
promote の log と台本の検査の順から「ok だったと推定」するしかなかった（final-report.md の「リリースゲート: B・C・D の最初の release」）。
promote の log（`$SD_LOGS/promote-<ts>.log`）は掃除で消えないが、gate と verify の結果を持たない。

### 1.2 責務の owner

- (a) の owner は `scripts/dev/test-parallel.sh` の集計の python（Summary 行の解析と `CELERIS_TEST_SUMMARY` の json）。
  `release.sh` の `cargo_test_summary_json`（270 行）はこの json を gate.json に写すだけなので変えない。
  欄の形を固定する試験は `scripts/selfdeploy/tests/release_parallel_test_gate.sh`（158 行の dict の一致と 210 行の legacy の形）。
- (b) の owner は、3 つの file を書く場所そのもの:
  - gate.json: `release.sh` の `write_gate_json`（300 行）の呼び出し。成功は 605 行（`$STAGE/gate.json`）の後の 634 行
    `mv -T "$STAGE" "$REL"`、失敗は 419 行と 489 行（`$FAILED_DIR/gate.json`）。
  - verify.json: `verify.sh` の 739〜771 行（`$REL/verify.json`。ok が真でも偽でも書く）。
  - promoted.json: `audit.sh` の `sd_audit finish`（66〜83 行。promote と、`restore-db` でない rollback で書く）。
  要約の行を作って追記する関数は `scripts/selfdeploy/lib.sh` に 1 つだけ置き、上の 3 か所から呼ぶ。

### 1.3 既存の再利用先

- 集計: test-parallel.sh の既存の `npass`・`nskip`・`dpass`・`dign` の変数（新しい集計の経路を作らない）。
- 置き場: `lib.sh` の `SD_LOGS`（52 行。`CELERIS_LOGS_DIR`、無ければ `$SD_BACKUPS`。ADR-0136）。promote・rollback・relocate-db の log
  （`promote-<ts>.log` など）と同じ dir で、release dir の掃除の対象外。
- 読み書きの道具: `lib.sh` の `sd_json_get`・`sd_ts`・`sd_log`、python3（gate.json・audit が既に要る）。
- 試験の形: 既存の `release_parallel_test_gate.sh` の偽の cargo・`run_release`・`gate_is`。fixture の不足の埋め方は
  `release_web_required.sh`（92 行の空の `cos-skills.txt`、100 行の `check-slot.sh` の写し）と同じにする。

### 1.4 選択肢

- (a)
  - 案 A1（採用）: Summary 行の解析を `passed` の後の `(N slow)` などの括弧の注記を許す形に直し、`cargo_test` に
    `nextest_passed`・`nextest_skipped`・`doc_passed`・`doc_ignored` を足す。`passed`・`failed`・`ignored` は nextest と doc test の和のまま
    （意味は「両方の和」で変えない。今までは解析の誤りで nextest の分が抜けていた）。
  - 案 A2（不採用）: `passed` の意味を nextest だけに変える。gate.json を読む人・release notes の文面との意味の食い違いが起き、
    doc test の数がどこにも残らない。
  - 案 A3（不採用）: Summary 行をそのまま文字列で gate.json に写す。読む側が再び解析することになり、同じ誤りを 2 か所に持つ。
- (b)
  - 案 B1（採用）: 書いた時点で `$SD_LOGS/release-records.jsonl` に 1 行 1 JSON の要約を追記する。promote の log と同じ dir で掃除の対象外。
  - 案 B2（不採用）: 掃除で消す release dir の 3 file を別の dir へ写して残す。file が増え続け、写しと正本の 2 つの file 形が並ぶ。
  - 案 B3（不採用）: 掃除の残す数を増やす。消える時期が後ろにずれるだけで、記録が消えることは変わらない。release dir は数 GB の binary を持つ。
  - 案 B4（不採用）: 本番 DB の表や events に書く。台本が DB に書く経路を新しく作ることになり（`lib.sh` の安全規則は DB を読むだけと決めている）、
    `crates/` も変わる。
  - `promote_failed.json` と `rolled_back.json` は要約の対象にしない（task の範囲は gate・verify・promoted の 3 つ。昇格の失敗は promote の log に残る）。

### 1.5 新しい状態・分岐・fallback・port

- 新しい状態: `$SD_LOGS/release-records.jsonl`（追記だけの写しの file）。正本ではない（D3）。
- 新しい分岐: 追記の失敗を警告だけにする分岐（D4）。台本の結果（exit code・gate.json・verify.json・promoted.json）は変えない。
- fallback・新旧切替の flag・port: 無し。`SD_RELEASE_RECORDS=0` のような切る flag は置かない。

### 1.6 撤去対象

- (a): test-parallel.sh の Summary 行を割る旧い regex（同じ変更で置き換える）。gate の試験の偽の cargo の `(N slow)` の無い Summary 行だけの期待値
  （158 行の dict）は、新しい欄と `(N slow)` の注記の付いた行を含む期待値に広げる。
- (b): 無し（新しい追記。旧い仕組みを置き換えない）。docs/ops/selfdeploy.md の置き場の表に `release-records.jsonl` を足す。

### 1.7 不変条件と確かめる試験

- `unpromoted-release-no-takeover`（[docs/ops/selfdeploy.md](../../docs/ops/selfdeploy.md)）: 昇格・takeover・引き渡しの判断は release dir の
  gate.json・verify.json・promoted.json と current の symlink だけで決まる。要約の log を読む経路を足さない（D3）。
  確かめる試験: 既存の `crates/celeris` の `unpromoted_release`・`instance_handoff`・`releases::tests`（この task は `crates/` を変えないので
  差分で確かめる: `git diff --name-only <base> HEAD -- crates/` が空）と、台本の `grep -n release-records scripts/selfdeploy/{promote,verify,release,rollback,status}.sh`
  が追記の呼び出しの行だけであること。
- gate の判定は変わらない: `passed`・`failed`・`ignored` の意味（nextest と doc test の和）と、`failed` が 1 以上なら gate が落ちることは同じ。
  確かめる試験: 広げた `release_parallel_test_gate.sh`（D1）。
- 要約の追記は台本の結果を変えない: 追記先に書けなくても release・verify・promote の exit code と 3 つの file は同じ（D4）。
  確かめる試験: `release_record_summary_survives_prune.sh`（D5）の書けない場合。
- 既存の prune・promote の試験を落とさない: `scripts/selfdeploy/tests/` の promote_* と release_prune_* を `close` で走らせる。

## 2. 決定

### D1. gate.json の `cargo_test` に nextest と doc test の内訳を足す

- `scripts/dev/test-parallel.sh` の Summary 行の解析を直し、`passed` の後に括弧の注記（`(3 slow)` など）が付いても通過数を取る。
  `failed`・`timed out`・`exec failed`・`skipped` の扱いは変えない。
- `CELERIS_TEST_SUMMARY` の json（＝gate.json の `cargo_test`）に 4 つの欄を足す:
  `nextest_passed`（nextest の通過数）・`nextest_skipped`（nextest の skipped）・`doc_passed`（doc test の通過数）・`doc_ignored`（doc test の ignored）。
  `CELERIS_TEST_SKIP_DOC=1` で doc test を回さないときは `doc_passed`・`doc_ignored` は 0。
- `passed`・`failed`・`ignored` は今までどおり nextest と doc test の和（`passed = nextest_passed + doc_passed`、
  `ignored = nextest_skipped + doc_ignored`）。欄の意味は変えず、解析の誤りで抜けていた nextest の分が入るようになる。
- 固定する試験は既存の `scripts/selfdeploy/tests/release_parallel_test_gate.sh`。新しい試験の file は足さない。
  - 偽の cargo の通る場合の Summary 行を `7 passed (1 slow), 2 skipped` の形にし、158 行の dict の期待値に 4 つの欄を足す
    （`nextest_passed` 7・`nextest_skipped` 2・`doc_passed` 1・`doc_ignored` 0。`passed` 8・`ignored` 2 は変わらない）。
    変えた意味（注記付きの行で nextest の通過数が取れること、内訳の欄）を試験の注釈に書く。
  - 210 行の legacy（`SD_GATE_TEST_RUNNER=cargo-test`）の形は `cargo test --workspace` の集計なので内訳の欄を持たない。dict の完全一致はそのまま残し、
    内訳の欄が無いことをそのまま固定する（注釈に書く）。
  - fixture の不足: この試験は base でも通らない。fixture の偽の木が `scripts/dev/lib/check-slot.sh` を写さない（115 行は
    `test-tmp-lifecycle.sh` だけ）ので test-parallel.sh の 55 行の `.` が落ち、cargo-test の段で gate が落ちる。`check-slot.sh` を写すと次に
    `scripts/selfdeploy/cos-skills.txt` が無く（106 行は `*.sh`・`*.py` だけを写す）`sd_bundle_cos_skills` で落ちる。
    この ADR を書いた WorkUnit が base で走らせて、2 つとも確かめた（`release_parallel_test_gate.sh` は exit 1、
    「check-slot.sh: No such file or directory」、写した後は「could not bundle the CoS skills (cos-skills.txt)」）。
    WorkUnit `counts` は `release_web_required.sh` と同じく `check-slot.sh` を写し、空の `cos-skills.txt`（注釈の 1 行だけ）を置く。
    試験の不変条件は変えない（偽の木を今の release.sh・test-parallel.sh が要る file に追いつかせるだけ）。

### D2. gate.json・verify.json・promoted.json を書いた時点で `$SD_LOGS/release-records.jsonl` に 1 行追記する

- 置き場: `$SD_LOGS/release-records.jsonl`（`lib.sh` の `SD_LOGS`。本番は `paths.env` の `CELERIS_LOGS_DIR`＝`/local/celeris/state/logs`、
  未設定なら `$SD_BACKUPS`）。promote の log と同じ dir で、`release.sh` の掃除（release dir と `.build/<sha12>/`）は触らない。
- 形: 1 行 1 JSON（JSON Lines、UTF-8、改行で終わる）。追記だけで、書き換え・削除・回転はしない。
- 共通の欄: `at`（追記した時刻、UTC の ISO 8601。`sd_ts`）・`kind`（`gate`・`verify`・`promoted` のどれか）・`sha12`・`ok`（bool）・
  `file`（正本の path。release dir の中の path、または失敗した gate の `.build/<sha12>/gate.json`）。
- kind ごとの要約（正本の file の値を写す。読めない欄は `null`）:
  - `gate`: `ok` は gate.json の `ok`。`sha`・`failed_step`・`cargo_test`（`runner`・`passed`・`failed`・`ignored`・`nextest_passed`・
    `nextest_skipped`・`doc_passed`・`doc_ignored`・`nextest_exit`・`doctest_exit`・`userns`）・`steps`（段の名前と exit の組の配列）。
  - `verify`: `ok` は verify.json の `ok`。`live_ok`・`schema_version`・`checks`（検査の id と ok の組の配列）。
  - `promoted`: `ok` は promoted.json の `status` が `succeeded`。`operation`（`promote`・`rollback`）・`mode`・`from`・`promoted_at`・`actor`・
    `policy_ref`・`included_tasks`（`included.tasks` の task id の配列）。
- 追記の時点:
  - gate（成功）: `release.sh` の `mv -T "$STAGE" "$REL"`（634 行）の後。`file` は `$REL/gate.json`。release dir ができる前の失敗で
    「ok の gate の行があるのに release dir が無い」写しを作らない。
  - gate（失敗）: `release.sh` の `write_gate_json "$FAILED_DIR/gate.json"`（419 行・489 行）の後。`ok` は false。
  - verify: `verify.sh` が `$REL/verify.json` を書いた後（768 行の後）。ok が偽でも追記する。
  - promoted: `audit.sh` の `sd_audit finish` が promoted.json を書いた後。promote と、`restore-db` でない rollback の両方
    （どちらも promoted.json を書く。`operation` で分かる）。
- 実装は `lib.sh` の関数 1 つ（例: `sd_release_record <kind> <正本の path>`）。要約の行は python で正本の file を読んで作る
  （正本と違う値を台本の変数から組み立てない）。1 行を 1 回の `write` で `O_APPEND` で書く（同時に走る release と promote の行が混ざらない）。

### D3. 正本は release dir の file。要約の log は書いた時点の写し

- release dir（`$SD_RELEASES/<sha12>/`）がある間は、その中の gate.json・verify.json・promoted.json が正本。失敗した gate は
  `.build/<sha12>/gate.json` が正本。`release-records.jsonl` はそれを書いた時点の写しで、後から正本が変わっても（再 verify など）追記の行は直さない。
- `crates/celeris/src` の `releases.rs`（419〜421 行・619 行で release dir の 3 file を読む）・`delivery.rs`（758〜759 行・864 行）・
  `release_notes.rs`（898 行）も、台本（`release.sh`・`verify.sh`・`promote.sh`・`rollback.sh`・`status.sh`）も、
  昇格・takeover・引き渡し・release notes の判断に `release-records.jsonl` を読まない（`unpromoted-release-no-takeover`）。
  昇格してよいかは今までどおり release dir の verify.json の `ok`、verify してよいかは gate.json の `ok` だけで決まる。
- 正本と写しが食い違えば release dir の file を信じる。写しが役に立つのは、掃除で release dir が消えた後に gate・verify・昇格の結果を人が確かめるときだけ。
- 写しの file が無い・壊れている・行が欠けていても、どの判断も変わらない。

### D4. 追記の失敗は警告だけ

- `$SD_LOGS` を作れない・書けない・正本の file が読めない・python が落ちたときは、`sd_log` に warning を 1 行出して続ける。
  release・verify・promote・rollback の exit code、gate.json・verify.json・promoted.json の中身、掃除は変わらない。
- 理由: 写しは人の事後の確認のためで、昇格の判断に使わない（D3）。写しのために release や昇格を止めない。

### D5. 試験 `scripts/selfdeploy/tests/release_record_summary_survives_prune.sh`

- bash で引数なしで走り、成功で exit 0、失敗で非 0 と `FAIL:` の行。一時 dir の fixture の release dir と偽の道具だけを使い、本番の path・DB・
  network・systemd に触れない。冒頭で `CELERIS_BACKUPS_DIR`・`CELERIS_LOGS_DIR`・`CELERIS_CONFIG` を unset して一時 dir に向け直す
  （本番の `paths.env` を環境から継がない。`promote_live_abort.sh` 201 行と同じ注意）。
- 確かめること:
  1. gate（成功と失敗）・verify・promoted の 3 種の行が、正本の file を書いた時点で `release-records.jsonl` に 1 行ずつ追記され、
     `at`・`kind`・`sha12`・`ok` と kind ごとの要約が正本の値と一致する。
  2. 掃除（`prune_releases`。`SD_RELEASE_PRUNE` を 0 にしない）で release dir が消えた後も、その sha の行が残る。
  3. 追記先に書けないとき（`$SD_LOGS` を書けない file にするなど）でも台本の exit code と正本の file は変わらず、warning が出る（D4）。
- 足す理由（注釈に書く）: 既存の prune の試験（`release_prune_production_scale.sh`・`release_prune_stale_test_binaries.sh`・
  `prune_tests_stay_in_tmp.sh`）は cargo の target の掃除を見るもので、release dir の掃除（`prune_releases`）の後に要約が残るかを見ない。
  `release_parallel_test_gate.sh` 等の release の試験は `SD_RELEASE_PRUNE=0` で掃除を飛ばす。

## 3. 採らない案

1.4 の A2・A3・B2・B3・B4。どれも理由は 1.4 に書いた。

## 4. 本番への影響

- config の欄・API・CLI・DB の変更は無い。gate.json の `cargo_test` に欄が 4 つ増える。`crates/` の試験の外で
  `cargo_test` を読む code は無い（この ADR を書いた時点で `git grep -n cargo_test -- crates` の試験の外の行は 0。`releases.rs`・`delivery.rs`・
  `release_notes.rs` の `read_json` は `serde_json::Value` で読む）。
- 本番では、次に人が回す `release.sh`・`verify.sh`・`promote.sh` から `release-records.jsonl` に行が増える。台本は `paths.env` を読むので、
  人が素の shell で回しても置き場は `CELERIS_LOGS_DIR` になる（`lib.sh` 25〜30 行）。人の手順は進捗の「本番への移行」に書く。
- 掃除で既に消えた release（B・C・D の最初の 3 release など）の記録は戻らない。この変更の後に作る release から残る。
