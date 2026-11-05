---
title: A9 縮小版の実 LLM pilot の結果と 4 点の修正・再実行手順
tasks: [01M6MH1H0RTRBMZF1C113WBQRZ]
status: done
completed: 2026-11-05
updated: 2026-11-05
---
# A9 縮小版の実 LLM pilot の結果と 4 点の修正・再実行手順

完了日: 2026-11-05（WorkUnit rerun-doc）。この WorkUnit はコードを変えていない。実 LLM は起動していない。

## 結論
- 4 点のうち、1（課題と hidden）・2（tree に無い成果の到達）・3（C06 の effect の配線）は stub harness（LLM 0）の範囲で修正し、試験で固定した。4（reviewer の合格）は決定的な検査の優先と食い違い記録までで、課題側の command 検査が未実装のため一部が未解決。
- 実 LLM での確認は未実施。**C06 を実 LLM で回し直しても、effect の predicate は落ちる**（理由は「3」の未解決）。到達の誤判定が直ったかは C08 で見る。
- operator は下の手順で C06・C08 の new 腕を回し直す。実行は人が行う（worker の run の中では起こさない）。

## 実測（2026-11-05 00:08Z〜00:15Z、operator が実行、exit 0）
raw は `agent-docs/progress/2026-11-04-long-horizon/a9/llm/`（`grade/summary.json`・`grade/grades.jsonl`）。
- 2 試行とも controller の phase は achieved、root はすべて done（C06: 2 本、C08: 4 本。C08 の o1 は review で 1 度差し戻し）。
- 採点は 2 試行とも verdict fail、到達率 0（`outcome_reached` がすべて fail。`summary.json` の arms.new は reach_rate 0.0、verdict_pass 0、trials 2、daemon_roots 6）。
- token: C06 3,864、C08 14,557、計 18,421。wall: 98 秒、282 秒（計 380 秒）。
- LLM 起動: C06 は cos 4・reviewer 2・worker 2、C08 は cos 8・reviewer 5・worker 5。

## 4 点の原因・状態
詳細と再現は `2026-11-05-a9-llm-pilot-fixes/investigate.md`、方針は ADR `2026-11-05-a9-llm-pilot-findings`（D1〜D4）。

### 1. hidden の正解が公開の指示から導けない — 修正済み（一部未解決）
- 原因: `oracle/<case>/hidden/expected.json` の sha256 が、`scenario.json` の `content`（stub だけが知る固定文字列）を指す。採点は `crates/campaign/src/long_horizon.rs` の `outcome_reached`。公開の acceptance にその書式が無かった。
- 修正（fix-oracle）: 公開の acceptance に `python3 minichunk.py verify <case> <id>` を書き、公開の repo-seed の `minichunk.py` がその出力を作る。出力の sha256 は元の expected.json と全件一致（expected.json は不変）。試験 `crates/campaign/tests/long_horizon_public_contract.rs`（3 件）が、公開の手順だけで到達すること、別の出力は落ちること、公開側が oracle の文字列も hash も持たないことを確かめる。
- 未解決: D2 の決定 2（hidden を検査 script の合否にする）と bench_version の更新（現在 1）は未実施。C04・C09・C10 の公開側は手順を持たない。文面を変えた妥当な出力は今も落ちる。
- 未確認: 実 LLM が公開の手順に従うか。

### 2. tree に成果が無くても到達になる — 修正済み
- 原因: controller（`control-ts/src/domains/project_controller.ts` の `satisfies`）は root の `integrated` だけを見る。`integrated` は `crates/task-ops/src/project_snapshot.rs` の `root_row` で作り、非 git・取り込む物なし（`NotGit`・`NoCommits`）を取り込み済みに数えていた。今回の raw では成果が tree に無い状態は再現していない（内容が oracle と違うだけ）。
- 修正: repo 付き案件は `IntegrationMerged` だけを取り込み済みに数える（root-row）。pilot の作業場所を git にし、stub が成果を commit する（pilot-git）。試験は `root_row_tests`、`project_controller_core_s5b_no_commits_root_is_not_achieved` など（各 WorkUnit の進捗に記録）。
- 未確認: 実 daemon の repo 登録 API を通した実 LLM での通し。遠隔 repo の `IntegrationNotApplicable` を到達に数えるかは人の判断（現状は数えない）。

### 3. C06 の effect が空 — 修正済み（stub harness の範囲）／実 LLM では未解決
- 原因: `scripts/dev/long-horizon-pilot/driver.py` が facts の `effects` を `[]` に固定し、effect を起こす stub が無かった。
- 修正（fix-effects）: `pilot_effect.py`（承認待ちを `pending/` に置き、operator の decision を待って `effects.jsonl` に記録）を足し、driver が読む。stub harness（LLM 0）の C06 では effects が `C06:o1:publish` の 1 件、承認待ち 2 件が facts の faults・human_ops に入り、verdict pass。承認前に publish する陰性 stub は `effect_count(o2)` と `effect_before(approval)` が fail。
- **未解決**: `driver.py` は effect stub を stub harness の command にしか渡さない（`--harness config` の worker には `pilot_effect.py` の場所も使い方も渡らない）。実 LLM の C06 では effects が空のまま `effect_count(C06:o1:publish) == 1` が fail し、承認待ちも facts に出ない。承認は driver の擬似 operator で、daemon の承認 API を通さない。sandbox から command を呼べるかも未確認。

### 4. reviewer が成果の無い root を合格にする — 一部修正
- 原因: 合格適用は `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `on_review_finished`。pilot の acceptance は command 検査を持たず、reviewer だけの基準になる。reviewer の判断理由は raw に残らず、根拠は未確認。
- 修正（review-check）: 決定的な検査が落ちたら reviewer が合格でも done にしない（試験 `review_check_wins`）。食い違いを event に記録する。
- 未解決: pilot 課題に command の検査（`check-public-<id>.sh`）が無い。stub reviewer を「検査できない基準は pass にしない」にする D4 の 4 は未実施。

## commit（件名。hash は書かない。基準は `$CELERIS_WU_BASE`）
- `adr-findings: A9 実 LLM pilot の修正方針を ADR 付記に書く`
- `root-row: repo 付き案件は IntegrationMerged だけを取り込み済みに数える`
- `review-check: 決定的な検査が落ちたら reviewer の合格でも done にしない試験と食い違い event`
- `pilot-git: pilot の作業場所を git にし stub が成果を commit する`
- `fix-effects: real daemon の pilot に公開相当の effect stub と承認待ちを繋ぐ`
- `fix-oracle: C06・C08 の hidden を公開の手順から導ける形にする`

## operator の再実行手順（人が実行する。worker の run では実行しない）
前提: 上の commit をすべて含む main の worktree で行う。本番の DB・daemon・config・release には触れない（script は一時 home に daemon を起こす）。`CARGO_TARGET_DIR` を設定しておく。

1. 実行（前回と同じ形。`--out` を前回の取り込み先にしない。script は `--out` 配下の facts・arm-map・trials・ops・grade を消す）:
   ```
   LONG_HORIZON_PILOT_LLM_APPROVED=1 bash scripts/dev/long-horizon-pilot-llm.sh \
     --harness config --llm-config /local/celeris/data/scratch/a9-pilot/raw-20261105-llm-config.toml \
     --cos-route '[{ source = "claude_oauth", model = "claude-sonnet-5-5" }]' \
     --cases C06,C08 --arms new \
     --state-root <scratch> --out <scratch>/a9-pilot/raw-<日付>-2
   ```
   exit 0 なら全試行が終わり採点まで済んでいる（1 = 失敗、2 = 承認なし・使い方の誤り）。上限は 1 試行 1,000,000 token / 4 時間。
2. 取り込み先: `agent-docs/progress/2026-11-04-long-horizon/a9/llm-rerun/` に `cp -a`（`__pycache__` は写さない: `rsync -a --exclude __pycache__`）。前回の `a9/llm/` は上書きしない。写したあと `diff -r --exclude=README.md --exclude=__pycache__ <scratch の raw> <取り込み先>` が exit 0 になることを確かめる。README は前回の `a9/llm/README.md` にならい、case 名と通し番号で試行を指す（試行 id を書かない）。
3. 見る欄（`grade/summary.json` と `grade/grades.jsonl`）:
   - 到達: `arms.new.reach_rate`・`verdict_pass`。C08 は `outcome_reached(o1)`〜`(o4)` が pass か。fail なら `trials/<trial>/repo/out/<id>.txt` の内容と、`python3 minichunk.py verify <case> <id>` の出力を比べる（問題 1）。
   - 食い違い: facts の `roots[*].statuses` が done でも `outcome_reached` が fail なら、controller の到達と採点がまだ分かれている（問題 2）。ops の `pc/achieved` と照らす。
   - C06 の effect: `effect_count(C06:o1:publish) == 1`、`effect_count(C06:o2:publish) == 0`、`effects_after_approval`、`effect_before(approval, effect)`、facts の `effects`・`human_ops`・`faults`（問題 3）。**現状では effect_count(o1) が fail し、承認待ちが出ないと予想する。**これは未解決の配線で、LLM の失敗として数えない。
   - 費用: `tokens`・`wall_secs`・`handoffs`、facts の `invocations`（cos・reviewer・worker の起動数）。
   - reviewer: C08 o1 のような差し戻しが出たか、reviewer の合格後に採点が fail した root が無いか（問題 4）。
4. 結果を `2026-11-05-a9-llm-pilot-fixes.md` の続き（または新しい進捗ファイル）に書き、前回との差（到達率・token・wall）を表にする。

## 未解決
- 3: 実 LLM の worker に effect stub を渡す配線（command の場所・使い方・書き込み権）と、daemon の承認 API を通す承認。
- 4: pilot 課題の command 検査と stub reviewer の D4-4。reviewer の判断理由を facts か ops に残す変更。
- 1: hidden の script 化（D2 の 2）、bench_version の更新、C04・C09・C10 の公開手順。
- `agent-docs/progress/2026-11-04-long-horizon/a9/llm/` に `diff -r --exclude=README.md <raw>` を取ると exit 1 になる。raw 側の `trials/<trial>/repo/__pycache__`（pyc）は、取り込みで写していない（件名 `wu/fix-pyc-note: README に __pycache__ の pyc を写さない旨を書く`）ためである。task の受け入れ条件 0 を満たすには `--exclude=__pycache__` を足すか、pyc を取り込む必要がある。
- 各 WorkUnit の試験は統合後の木で全体を再実行していない箇所がある（fix-arrival の進捗を参照）。

## 提案
- C06 の実 LLM 確認の前に、effect stub を worker に渡す task を立てる（C06 の公開 acceptance に publish の手順を書く案、または harness の PATH に command を置く案。後者は公開の指示から導けるかの判断が要る）。
- 再実行の結果を見てから、4 の課題側 command 検査（`check-public-<id>.sh`）の task を立てる。
- pilot の raw の取り込みで `__pycache__` を除く script（rsync の定型）を `scripts/dev` に置く。
