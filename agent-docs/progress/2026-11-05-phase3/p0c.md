---
title: P0-C long-horizon ベンチ v2（評価器の正当性）
tasks: [01M6N7XY9KX62X9TX696TEHXNF]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# P0-C long-horizon ベンチ v2（評価器の正当性）

完了日: 2026-11-05。親は 01M6N7WD1WXXW0A5FV7RVZ67B7。基準は `git merge-base HEAD main`。設計は ADR `agent-docs/adr/2026-11-05-phase3-p0c.md`。

## 結論
- 固定文字列の sha256 一致（`oracle/<case>/hidden/expected.json`）を消した。Outcome の到達は保護領域の検査 script `tests/fixtures/long-horizon/grader/check_outcome.py` の exit code だけで決まる（I5）。script は LLM の回答文・controller の主張・reviewer の判断を読まない。
- 公開の契約を全 10 課題の `public/<case>/repo-seed/README.md` と `case.json` の acceptance に書いた。`out/<id>.txt` に 4 欄（case・outcome・status・evidence）を行の形か JSON で書けば合格する。公開の手順 `python3 minichunk.py verify <case> <id>` でも、手で書いた別の表現でも通る。
- `bench_version` は 2。canary・trial salt を作り直した。v1 の raw は 1 byte も変えず、v2 の採点器は版の不一致で拒む（I6）。
- 全 10 課題 × 4 腕の replay を v2 で流し直し、再採点した。腕ごとの値は v1（a8b）と同じだった（下の表）。controller の achieved と oracle の不一致は 0 件（I12）。

## 共通受入との照合
- 修正済みは再実装していない。A9 findings D1（git の作業場所）・D3（effect stub）は main のまま使い、D2（hidden の作り）だけを実装した。
- 旧 milestones API・認可・Rust/TS の分離・migration には触れていない（P0-C は migration を足さない。p0a.md の予約どおり）。TS の変更は無い。
- 本番 promote・systemctl・本番 config/DB・人専用 API は使っていない。研究 K2/K4・IEEE Access の凍結評価の path は触れていない（`test-long-horizon-bench.sh` の D10 の検査が通る）。

## 変更（commit は件名で挙げる）
- `p0c: long-horizon bench v2 — 公開の契約と保護領域の検査 script で採点し、hash の oracle を消す`
  - fixture: `public/*/case.json`・`repo-seed/{README.md,minichunk.py}`（`verify` が 4 欄を書く、`check` を追加）、`oracle/*/{oracle.json,scenario.json}`（`bench_version` 2、台本の値を課題ごとに違う 10 通りの正解表現にした）、`oracle/holdout.json`（canary・salt）、`oracle/MANIFEST.sha256`、`grader/check_outcome.py`（新規）。`oracle/*/hidden/` を削除。
  - `crates/campaign/src/long_horizon.rs`: `Grader`（manifest 照合・`env -i`・`python3 -I -S`・空の cwd・木と fixtures の重なりの拒否・leak の走査）、`grade`・`metrics` は検査の結果（`Judged`）だけを読む、`BENCH_VERSION` の照合、試行ごとの `agreement` と `achieved_oracle_mismatch`、`leak_detected`、gate の追加条件。`outcome_reached` を削除。
  - `crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`: facts に `controller_achieved`（案件の phase）。
  - `scripts/dev/long-horizon-pilot/driver.py`: facts に `controller_achieved`・`leak_hits`（試行の home の走査）、判断の規則を `proposal.explanation` からも読む、stub の `--answers` を消した。`stub_harness.py`: 公開の手順だけで成果を書く。
  - `scripts/dev/long-horizon-pilot-llm.sh`: v1 の raw（`2026-11-04-long-horizon/`）の下への出力を拒む。実 LLM の既定の出力先を v1 の `a9/llm` から phase3 の `llm-v2/` に変えた（前は既定で `a9/llm` の facts 等を消していた）。
  - `scripts/dev/test-long-horizon-bench.sh`: contract の試験、commit 済み v2 raw と今の再生の `diff -r`、leak の針（script の印・hash・salt）と陽性対照、`hidden/` の残りと版の検査。
  - 試験: `crates/campaign/tests/long_horizon_public_contract.rs`（9 件に置き換え）、`long_horizon_bench.rs`（+7 件、計 24 件）。
  - 文書: ADR `2026-11-05-phase3-p0c.md`、bench ADR の付記 A8 の 4 に改訂の注記、`docs/architecture-map.md` に 1 行。
  - raw: `p0c/replay-v2/`（facts・arm-map・trials の 40 試行）、`p0c/grade-v2/`（grades.jsonl・summary.json）、`p0c/v1-fixtures/`（v1 の fixture の写し）、`p0c/v1-raw.sha256`（v1 の raw 256 file）、`p0c/logs/`。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
### 0. bench_version 2 で全 10 課題の replay 再採点と陰性、複数の正解表現が合格・不十分な解が fail
- `bash scripts/dev/test-long-horizon-bench.sh --out $TMPDIR/bench` → exit 0。replay 40 passed、grade 24 passed、contract 9 passed。CLI の grades.jsonl は試験の出力と bytes 一致、commit 済み `p0c/replay-v2` と再生の `diff -r` は差分 0。生記録 `p0c/logs/test-long-horizon-bench.log`。
- 複数の正解表現（K13）: `long_horizon_public_contract_accepts_multiple_representations_in_all_cases` が全 10 課題・全 Outcome で 6 通り（行・順序違い・`=` と空白・報告文に混ぜた bullet と引用符と大文字・全角 `：` と CRLF・JSON）を合格にする。台本の値も課題ごとに 10 通りで、`..._matches_scenario_in_all_cases` が 4 通り以上の形を確かめる。公開の手順だけで届くことは `..._reaches_from_public_steps_in_all_cases`。
- 不十分な解（K14）: `..._rejects_insufficient_outputs_in_all_cases` が全 10 課題・全 Outcome で、空の木と 13 種（空 file・v1 の固定行・自己申告の文・`incomplete`・別課題の case・別 Outcome・status failed・別課題の evidence・evidence 欠け・status の矛盾・JSON の非文字列・UTF-8 でない・64 KiB 超）を落とす。symlink と dir は `..._rejects_symlinks_and_dirs`。v1 の実 LLM の木（a9/llm）は v2 の契約も満たさない（`..._v1_llm_trees_do_not_meet_the_contract`）。
- 再採点（K42）: `long_horizon_bench_v2_rescoring_compares_arms_on_same_init_and_oracle` が 40 試行で、課題ごとに 4 腕の `init_digest` が 1 つ、同じ oracle、`new` が全課題 pass、対照 `ctl.null` が `outcome_reached` で落ちること、空の木なら全課題の `new` が落ちることを確かめる。

v2 の集計（`p0c/grade-v2/summary.json`）。v1（`2026-11-04-long-horizon/a8b.md` の表）と同じ値。closeout（P0-B・fix-replay の後の HEAD）で今の replay の grade-v2 と bytes が一致することを確かめた（下の「closeout」節）:

| 腕 | oracle 合格 | 到達率 | autonomous_reach | avoidable | 無駄起票 | double effect | controller achieved | 不一致 |
|---|---|---|---|---|---|---|---|---|
| new | 10/10 | 1.00 | 1.00 | 0 | 0 | 0 | 8/10 | 0 |
| old.passive | 3/10 | 1.00 | 0.30 | 10 | 0 | 0 | 0/10 | 0 |
| old.proactive | 8/10 | 1.00 | 0.90 | 1 | 1 | 0 | 0/10 | 0 |
| ctl.null | 2/10 | 0.18 | 0.20 | 0 | 0 | 0 | 0/10 | 0 |

### 1. v1 の raw が改変されず別に保存され、grader の隔離と leakage 否定試験がある
- `long_horizon_bench_v1_raw_is_kept_unmodified`: `p0c/v1-raw.sha256`（main の状態で取った 256 file）と今の file を照合、v1 の facts は `bench_version` 1、v2 は 2、v1 の fixture の写しがある。
- `long_horizon_bench_v1_raw_is_rejected_by_bench_version`（K16）: a9/llm・a8b/replay・a9/stub を v2 で集計すると `bench_version` で Err。
- `long_horizon_public_contract_grader_is_isolated_from_the_worker`: 改ざんした script は manifest 照合で開けない、木と fixtures の重なりは Err、木の罠（`json.py`・`hashlib.py`・`re.py`・`minichunk.py`・`site.py`）は実行されない、script の環境は空（python が足す `LC_CTYPE` だけ）で cwd は空の dir、exit 2・壊れた出力・exit と出力の矛盾は Err。
- `long_horizon_public_contract_no_seed_or_answer_leak_to_worker`（K15）: 全 10 課題の public と seed の写しと v2 の 40 試行の木に、script の印・hash・本体の行・canary・salt・全課題の evidence の値・台本の値が無い。陽性対照（script・canary を写しに置く）は当たる。
- `long_horizon_bench_leak_invalidates_the_trial`: 木に script・canary、facts に `leak_hits` があれば `leak_detected` で分母から外れ、gate が落ちる。
### 2. 合否が LLM の回答を読まず決定的な script で決まる
- `long_horizon_bench_verdict_ignores_llm_answers_and_claims`: C04・C06・C08・C09・C10 で、facts に回答文・reviewer の判断・root の要約を足しても grade の bytes が同じ。木が空なら、自己申告と controller の achieved があっても fail。
- 独立した judge: `long_horizon_public_contract_independent_judges_agree_on_replay_trees` と上の各試験で、保護領域の script・公開の `minichunk.py check`（seed の実装）・試験の Rust 実装の 3 つの合否が全ケースで一致する。
- `long_horizon_bench_achieved_oracle_mismatch_is_counted`（K23）: C03 の `new` の木の o2 を壊すと `achieved_oracle_mismatch` が 1、gate が落ちる。
### 3. `test -f agent-docs/progress/2026-11-05-phase3/p0c.md` → exit 0

### Phase 完了の検査
- `bash scripts/dev/test-affected.sh` → exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、3368 tests run: 3368 passed、9 skipped、bundle SKIP 0 件（`p0c/logs/test-affected.log`）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-long-horizon-pilot.sh --out $TMPDIR/pilot` → exit 0（dispatch 7・api 3 passed、stub daemon の C08・C06 の raw が v2 の採点で ok）。
- `python3 scripts/dev/check-architecture-map.py` → exit 0。

## 失敗したケースと直したこと
- 最初の再生で `achieved_oracle_mismatch: 9` で gate が落ちた。replay の `achieved_at` は台本の木が揃った時刻で、旧腕にも値が入る。controller の主張として `controller_achieved`（案件の phase）を facts に足して直した。
- 判断の表の `proposal_json` の規則は `proposal.explanation.rule_id` にあり、driver は最上位の `explanation` だけを読んでいた。両方を読むようにした（実 daemon での確認は P0-F）。
- worktree の target で V8 の link が落ちた（`rusty_v8` が無い）。`$CARGO_TARGET_DIR/debug/build/v8-*` を消して作り直した。
- `long-horizon-pilot-llm.sh` の文に未作成の dir の path を書いたら check-doc-links が落ちた。文を直した。

## closeout（2026-11-05、提出時の HEAD で実行）
基準は `git merge-base HEAD main`（main の件名「p1g: main への rebase（merge_base の局所修復）の検査の結果を書く」）。コードと raw は変えていない。
- `bash scripts/dev/test-long-horizon-bench.sh --out "$TMPDIR/bench5"` → exit 0。replay 40 passed、grade 24 passed（`long_horizon_bench` の D8 gate を含む）、contract 9 passed。今の再生の raw と commit 済み `p0c/replay-v2` の `diff -r` は差分 0。
- 今の再生の grade（`$TMPDIR/bench5/summary/{grades.jsonl,summary.json}`）と commit 済み `p0c/grade-v2/` の `cmp` → 2 file とも一致（exit 0）。上の表はこの grade の値。
- CLI 採点 `cargo run -q -p celeris-campaign --bin celeris-campaign -- long-horizon-grade --tier replay --raw agent-docs/progress/2026-11-05-phase3/p0c/replay-v2 --fixtures tests/fixtures/long-horizon --out "$TMPDIR/cli6"` → exit 0（40 trials）。`grades.jsonl` は `p0c/grade-v2/grades.jsonl` と bytes 一致。CLI の `summary.json` は `"tier": "replay"` の 1 行だけ多い（CLI が tier を書くため。`grade-v2` は試験の出力）。他の行は一致。
- 腕ごとの値（今の grade）: new は oracle 10/10・controller_achieved 8/10・avoidable 0・oracle_pass_without_achieved 2。old.passive は 3/10・0/10・avoidable 10。old.proactive は 8/10・0/10・avoidable 1・無駄起票 1。ctl.null は 2/10・0/10・avoidable 0・到達率 0.18。achieved_oracle_mismatch は全腕 0。
- 文書検査: `python3 scripts/dev/check-architecture-map.py` → exit 0（917 path）。`bash scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0。`bash scripts/dev/check-adr-numbers.sh` → exit 0（376 file）。`sh scripts/dev/progress-index.sh --check` → exit 0。
- Phase 完了の検査（commit「p0c: closeout で bench script・CLI 採点・文書検査を再実行した結果を書く」の後に実行）: `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` → exit 0（3412 passed、9 skipped、bundle SKIP 0 件）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 未解決
- `new` の C06・C10 は oracle が pass で controller が achieved でない（`oracle_pass_without_achieved` 2 件）。原因は未確認（C06 は o2 の公開が却下される課題、C10 は同じ失敗を繰り返す課題で、どちらも controller が achieved 以外の phase で終わったと推定）。I12 の欠陥（achieved なのに fail）ではないが、controller が到達を認めない側の食い違いとして P0-B・P1-G に渡す。
- real daemon の実 LLM の試行では、worker の sandbox が repo の `tests/fixtures/long-horizon/grader/` を読めるかを確かめていない（A9 は ops-r2 の sandbox を使っていない）。事後の検出（`leak_hits`）は入れたが、読めないことの保証（ADR bench D4 の L2）は P0-F の実測の前に要る。
- `gate` は replay だけで動く。llm_pilot は不一致を数えて報告するだけ。
- 実 LLM pilot の v2 raw（P0-F）は未実施。

## 次の gate
- closeout の時点で P0-C の残りは無い。task の最終 review と配送（main への取り込み）が次の gate。
- P0-F: 人の許可の後、`LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh --harness config …` を ops-r2 の sandbox の中で流し、`achieved_oracle_mismatch` と `leak_detected` を報告する。
- P0-E: root の command check に `python3 minichunk.py check <case> <id>` を使える（公開の自己検査。採点には使わない）。

## 配送の修復（2026-11-05、delivery-repair）
- 症状: 配送の release 準備（`/local/celeris/state/releases/.deliveries/01M6N7XY9KX62X9TX696TEHXNF/672b77ac…/prepare.log`）は fmt・試験・script bundle・clippy・source-size まで exit 0 で、`cargo build --release` の途中に `Terminated` で終わった。
- 原因: 準備は旧 daemon `38127f8a64fe` の子だった。09:17 の promote で旧 daemon が draining になり、09:33:09 に手元の run が 0 になって終了した（journal の「drained; exiting 0」）。unit の停止で準備も同じ秒に止まった。P0-C のコードの不備ではない。
- 修正: commit「delivery: drain 中の daemon は自分が起動した release 準備の終了を待つ」。ADR `agent-docs/adr/2026-11-05-drain-waits-for-release-prepare.md`。試験 `prepare_child_counts_as_in_flight_until_it_exits`。
- 本番への移行: なし（config・DB の変更は無い）。この修正が入る release までは、promote が準備の途中に重なると同じ失敗が起きうる。配送の再準備は celeris が行う。

- repair-1（merge_base、2026-11-05）: 検査済みの merge candidate と main が食い違ったため、task branch を最新の main（件名「p0b: attempt 3 の検査の結果を書く」）へ rebase した。衝突なし（task の 5 file と main の差分の file は重ならない）。提出時の HEAD で `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0、`env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` exit 0（978 passed、0 skipped、bundle SKIP 0 件）。

## P0-B の後の replay の修正（2026-11-05、fix-replay）
- 症状（review の差し戻し）: P0-B を含む main の上で `bash scripts/dev/test-long-horizon-bench.sh --out "$TMPDIR/bench2"` は exit 1（「the committed v2 raw differs from this replay」）。new 腕の 9 facts（C01〜C08・C09 の new）が変わった。controller_achieved は 8/10 から 0/10、人の nudge（avoidable）が C03・C04・C07・C08 などで増え、C06 は `fired 1 faults, expected 2` だった。
- 原因（再現で確認）: P0-B の I4 は、repo の無い root を `ArtifactProduced` か合格の artifact_exists/knowledge_page が無いと取り込み済みにしない。bench の台本の worker は `out/<id>.txt` を書いて `review_pass` の done にするだけで、成果物の event を残していなかった。controller は root を到達と数えず次の Outcome を起票しなかったので、台本の人の nudge が起票を代わりに起こした。C06 は o2 の承認待ちに届かなかった。
- 直し方: 製品コード・gate・oracle は変えていない。replay の台本の `review_pass` の経路だけで `ArtifactProduced`（`out/<id>.txt`、sha256 は書いた内容）を残す。`review_fail` の経路には付けない（commit「p0c: replay の台本の worker が成果物の ArtifactProduced を残す（P0-B の I4 に合わせる）」、ADR 付記「P0-B（I4）の後の replay の fixture」）。
- 結果: 修正後の replay の raw は commit 済みの `p0c/replay-v2` と `diff -r` で一致した。`grade-v2` の `grades.jsonl`・`summary.json` も `cmp` で一致したので、raw・grade・MANIFEST は書き換えていない。new 腕は oracle 合格 10/10、controller_achieved 8/10（C06・C10 は未達。変更前と同じで、原因は未確認）、avoidable 0、fired 16・recovered 16。
- 検査（提出時の HEAD、main 「p1g: main への rebase（merge_base の局所修復）の検査の結果を書く」へ rebase した後に実行）:
  - `bash scripts/dev/test-long-horizon-bench.sh --out "$TMPDIR/bench4"` → exit 0（replay 40 passed、grade 24 passed、contract 9 passed）。`long_horizon_bench` の 24 件はこの grade 段で走った。
  - `git diff $(git merge-base HEAD refs/heads/main) -- agent-docs/progress/2026-11-04-long-horizon/` → 差分 0 byte。
  - `env -u CELERIS_WU_BASE bash scripts/dev/test-affected.sh` → exit 0（3412 passed、9 skipped、bundle SKIP 0 件）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

## 提案
- `scripts/dev/long-horizon-pilot-llm.sh --harness config` は、ops-r2 の sandbox で起動していなければ断る検査を足す（P0-D・P0-F の範囲）。
