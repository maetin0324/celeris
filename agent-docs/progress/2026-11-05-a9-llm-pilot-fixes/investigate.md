---
title: A9 実 LLM pilot の不具合調査（原因と再現）
tasks: [01M6MH3ZCVYGBFX6ZF8TTE17NJ]
status: running
updated: 2026-11-05
---
# A9 実 LLM pilot の不具合調査

raw は `agent-docs/progress/2026-11-04-long-horizon/a9/llm/`（以下 `a9/llm/`）。試行は case 名と通し番号で指す（C06 試行 1、C08 試行 1）。
この節は inv-oracle-reach が書いた。仮説 3・4 は inv-stub-review が同じ file に足す。コードは変えていない。

## 仮説 1

仮説: oracle の文字列が公開側から導けない。**確認した（再現できた）。**

### 原因
- 隠し受入の正解は `tests/fixtures/long-horizon/oracle/<case>/hidden/expected.json` の sha256。
- 元の文字列は `oracle/<case>/scenario.json` の `content`（例 `minichunk C08 o1 verified: write+fsync+restart read-back ok\n`）。これは stub harness が書くためだけの値。
- 採点は `crates/campaign/src/long_horizon.rs` の `outcome_reached`（`out/<id>.txt` の sha256 と `hidden` を比べる）と、それを呼ぶ `grade`。内容が 1 byte でも違えば fail。
- 公開側（`tests/fixtures/long-horizon/public/<case>/case.json` と `repo-seed/`）にこの文字列も書式も無い。acceptance は `dorastiq-o1: hidden acceptance passes on the integrated tree` だけ（`scripts/dev/long-horizon-pilot/driver.py` の `self.request` がそのまま LLM に渡す）。`grep -rn "verified: write" tests/fixtures/long-horizon/public control-ts/src` は 0 件。
- したがって実 LLM は正解の内容を知り得ない。stub では `scenario.json` の `content` を使うので通る。pilot が実 LLM でだけ全滅する原因はこの非対称。

### raw の該当箇所
- `a9/llm/grade/grades.jsonl`: C06 試行 1 の `outcome_reached(o1)` は fail、C08 試行 1 の `outcome_reached(o1)`〜`(o4)` は 4 件とも fail。
- C06 試行 1 の `trials/<trial>/repo/out/o1.txt` は 11 byte の `kestrane-o1`（acceptance の接頭語だけ）、`o2.txt` も同様。sha256 は期待値と別。
- C08 試行 1 の `out/o1.txt`〜`o4.txt` は 755〜1662 byte の日本語の調査報告（先頭は `dorastiq-o1: 調査と実装` など）。内容として妥当でも hash は一致しない。
- 4 本とも out/ に file は在る。fail の理由は「到達していない」でなく「内容が oracle の固定文字列と違う」。

### 再現（LLM なし・決定的。コマンドは worktree 直下で実行）
1. 採点の再実行（raw をそのまま入力）:
   `celeris-campaign long-horizon-grade --tier llm_pilot --raw agent-docs/progress/2026-11-04-long-horizon/a9/llm --fixtures tests/fixtures/long-horizon --out <一時 dir>`
   → exit 0。出力の `grade/` は `a9/llm/grade/` と `diff -r` で差が無い（同じ 5 predicate が fail）。
2. 公開側から導けない文字列を与えれば通ることの確認: C08 試行 1 の `repo/` を一時 dir に写し、`out/o1.txt`〜`o4.txt` を `minichunk C08 <id> verified: write+fsync+restart read-back ok\n` に置き換えて sha256 を `expected.json` と比べた。→ 4 件とも一致（True）。C06 の `o1` も同じ書式で一致した。
3. 試験として組んだものではなく、上の 2 つの確認コマンドの結果である。一時 file は commit していない。

## 仮説 2

仮説: C08 の o2〜o4 が案件 tree に届かないのに到達になる。**この raw では「届かない」は再現しなかった。一方「tree を見ずに到達になる」経路は、コードから確認した。**

### raw で見えたこと
- C08 試行 1 の `a9/llm/ops/<trial>.json` の rules は `pc/outcome-open` と `pc/milestone-reached` が 4 組、最後に `pc/achieved`。4 本の root はすべて done（`facts/<trial>.json` の `roots[*].statuses` の末尾が `done`）。
- 最終 tree の `trials/<trial>/repo/out/` には o1〜o4 が 4 本とも在る。作業は非 git の作業場所（`driver.py` の `trial-repo`）へ直接書かれ、worktree を介さない。
- したがって「o2〜o4 が tree に届かない」は、この raw では起きていない。仮説が見た「届かない」は、内容が oracle と違う（仮説 1）ことの言い換えだった可能性が高い。ただし、元の観察の出所は未確認。

### 原因（controller は tree を見ずに到達にする）
- `control-ts/src/domains/project_controller.ts` の `satisfies`: root が `done`・`reviewer_passed`・`integrated` で、`outcome_ids` に当 Outcome がある（または acceptance 文が一致する）と到達にする。tree の内容は見ない。`mark_milestone`（`pc/milestone-reached`）と `pc/achieved` はこれに従う。
- `integrated` は `crates/task-ops/src/project_snapshot.rs` の `root_row` で作る: `task.status == Done && (task.repos.is_empty() || merged)`。repos が空なら無条件で真。
- `merged` も `IntegrationMerged`・`IntegrationNotApplicable`・`DeliverySkipped` のうち `reason.nothing_to_integrate()` のとき真。`nothing_to_integrate`（`crates/task-core/src/delivery.rs`）は `NotGit | NoCommits`。つまり「取り込む物が無い」は「取り込み済み」に数える（ADR 2026-11-04-project-controller 付記 A9）。
- pilot の作業場所は非 git の dir（`driver.py` の `project` 作成と `auto_integrate: True`）。root が tree に何も書かなくても、reviewer が合格し done になれば到達になる。採点側（`long_horizon.rs` の `outcome_reached`）だけが tree を見るので、controller の到達と採点の到達が食い違う。
- git の作業場所で commit が tree に載らない場合（`NoCommits` で見送り）も、同じ式で到達になる。こちらは実 daemon で再現していない（未確認）。コードを読んだ範囲の推測。

### 再現
- 食い違いの再現: 仮説 1 の手順 1。C08 試行 1 は controller が `pc/achieved`、採点は `outcome_reached` 4 件 fail（`grade/grades.jsonl`）。同じ raw で両者が分かれることを確認した。
- 「tree が空でも到達になる」ことの直接の再現は未実施。実 daemon（`scripts/dev/long-horizon-pilot-llm.sh --harness stub`）を起こして、stub の書き込みを止めるには harness の変更が要り、この task は crates と scripts を変えない。試験を一時的に書く案は、本 task の時間内に決定的に組めなかったので見送った。次の task（修正）で、`project_snapshot` の root_row に「repos 空の done root は integrated 真」を固定する試験を先に足す。
- 既存試験: `crates/celeris/tests/project_controller_e2e.rs` の `project_controller_core_s5_not_integrated_continues` は「done だが未取り込みの root は到達にしない」側の固定。repos 空・NotGit の側を固定する試験は見つけられなかった。

## 修正方針の候補（ADR 付記に渡す）
- 仮説 1: 課題の公開側に出力の書式（例: `out/<id>.txt` に書く 1 行の正確な内容）を明記するか、hidden を「内容の hash 一致」から「検査 script の合否」に変える。後者なら正解の文字列を公開しなくてよい。
- 仮説 2: 非 git・NoCommits の root は、tree に成果が在ることを確認できる範囲（例: out/<id>.txt の存在）でだけ到達にするか、到達に数えない。選択は ADR 付記（adr-findings）で決める。

## 仮説 3

仮説: C06 の effects が空で、real daemon pilot に effect stub が無い。**確認した（原因はコードで特定、採点の再実行で再現）。**

### 原因
- `scripts/dev/long-horizon-pilot/driver.py` の facts を組む箇所（`"roots": roots, "effects": []`）が `effects` を空配列に固定している。DB から effect を読む処理も、effect を起こす stub も、この pilot の経路に無い。`scripts/dev/long-horizon-pilot/stub_harness.py` は `out/<id>.txt` を書くだけで publish などの外部作用を起こさない。`scripts/dev/long-horizon-pilot-llm.sh` は driver を呼ぶだけで effect の配線を持たない。
- 採点側は `crates/campaign/src/long_horizon.rs` の `grade`。oracle の `effect_count`（C06 は `C06:o1:publish == 1`、`C06:o2:publish == 0`）を `facts["effects"]` の `logical_id` 件数と比べる（同 file の `effects_after_approval` の判定と、`double_effect` の集計も同じ欄を読む）。effects が常に空なので、effect を要る case（C06 の o1）は `effect_count(...) == 1` が必ず落ちる。実 LLM が何をしても通らない。
- effect を書く実装は scripted 試験の側にだけある。`crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs` の `fn effect` が、台本の root が `publish` の Outcome で合格する時点で effects に積む（仮想時刻、LLM 0）。つまり effect は「台本が自分で記録する値」で、real daemon の副作用から来る値ではない。pilot の driver にはこれに当たる経路が無い。
- C06 は承認待ち（`approval: approve` / `reject`）と認証待ちを含む case だが、pilot の facts は `human_ops` が `project_request` 1 件、`faults` が空で、承認の操作も欠けている。`effects_after_approval` は effect が 0 件なので自明に pass する（raw の predicate は pass、理由 `0 effects before the approval`）。通ったが意味は無い。

### raw の該当箇所
- `a9/llm/facts/` の C06 試行 1: `effects` が `[]`、`human_ops` は `project_request` の 1 件のみ。
- `a9/llm/grade/grades.jsonl` の C06 試行 1: `effect_count(C06:o1:publish) == 1` が fail（reason `0`）、`effect_count(C06:o2:publish) == 0` は pass（0 件で自明）。
- C08 には effect を要る Outcome が無く、この predicate は出ない。

### 再現（LLM なし・決定的）
1. `grep -n '"effects": \[\]' scripts/dev/long-horizon-pilot/driver.py` → 1 件。
2. 採点の再実行: `celeris-campaign long-horizon-grade --tier llm_pilot --raw agent-docs/progress/2026-11-04-long-horizon/a9/llm --fixtures tests/fixtures/long-horizon --out <一時 dir>` → exit 0。`effect_count(C06:o1:publish) == 1` が fail、reason `0`。
3. 対照は `long_horizon_bench.rs` の scripted 試験。こちらは `fn effect` が積むので同じ predicate が pass する。両者の差は effect を書く主体が無いことだけ。
4. real daemon で effect stub を足した動作は未確認（この task は scripts を変えない）。

## 仮説 4

仮説: reviewer が tree に成果の無い root を合格にする。**部分的に確認した。成果が不十分な root が done になったことは raw で確認した。reviewer が何を根拠に合格にしたかは未確認。**

### raw で見えたこと
- C06 試行 1 の root 2 本（o1、o2）は `statuses` が `ready, running, reviewing, done` で、reviewer の合格で done になった。`out/o1.txt` は 11 byte の接頭語だけ（仮説 1）で、`oracle` の期待内容とも、publish の effect とも合わない。C06 の o2 は oracle 上「承認が reject で publish しない」Outcome なのに、`out/o2.txt` を書いて done になっている。
- C08 試行 1 の o1 は `reviewing` を 2 回通って done（1 回目は差し戻し、2 回目に合格）。reviewer が不合格を出す場合もあり、常に合格にするわけではない。
- 同じ raw に reviewer の verdict 本文（`review.json` の reason）は無い。`facts` の `invocations` は起動の時刻と役割だけで、判断理由を持たない。したがって「何を見て合格にしたか」は raw から読めない（未確認）。

### 原因（コードから確認した範囲）
- 合格の適用は `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `on_review_finished`。全基準の verdict が pass（`outcome.all_pass()`）なら ReviewPass を記録して done に進む。tree に成果物が在るかを決定的に確かめる処理は、この経路に無い。判定は reviewer の verdict だけで決まる。
- reviewer への指示は `crates/task-worker/src/claude_code/prompt.rs` の `build_review_prompt`。acceptance を並べ、疑わしければ不合格にするよう書く。ただし pilot の acceptance は `<case>-<id>: hidden acceptance passes on the integrated tree`（仮説 1）で、reviewer は hidden を持たず、この文を検査できない。さらに `check` は Reviewer で、決定的な検査（`## Deterministic checks` 節）は空になる。実 LLM の reviewer は「検査できない基準」を、読んだ範囲の印象で合格にしうる。
- pilot の作業場所は非 git の dir で、reviewer が見るのは `out/` の内容だけ。取り込み状態も、tree への到達も、controller は reviewer の合格と done で `integrated` 真にする（仮説 2）。
- stub harness の reviewer（`stub_harness.py` の `kind == "review"` の枝）は全基準を無条件に pass にする。scripted 試験（`long_horizon_bench.rs`）の reviewer も台本で合格する。したがって LLM 0 の経路では、この不具合は出ない。実 LLM でだけ現れうるのに、検査できる経路が無い。

### 再現
- 「reviewer が合格にする」ことの再現は、raw の `statuses`（done 到達）の確認までで、reviewer の判断の再現は未実施。fake LLM で再現できるのは「検査できない基準を stub reviewer が合格にする」ことまで（`stub_harness.py` の仕様どおり）。実 LLM の reviewer が同じ入力で合格にするかは、LLM 呼び出しが要るため、この task（LLM を起動しない）では未確認。
- 決定的な側の確認案（次の task）: `on_review_finished` に対して、`out/<id>.txt` が無い（または期待と違う）tree で reviewer が全 pass を返した入力を与え、done にならないことを固定する試験を足す。既存の試験でこの条件を固定するものは、`review_verdict` 周辺では見つけられなかった。

## 修正方針の候補（仮説 3・4。ADR 付記に渡す）
- 仮説 3: pilot の driver に effect の記録を配線する。real daemon の外部作用は publish を模した stub（記録だけ書く）として置き、facts の `effects` を DB／stub の記録から読む。承認操作と認証待ちの `human_ops`・`faults` も同じ経路で facts に入れる。または、effect を要る case（C06）は real daemon pilot の対象から外し、scripted 試験だけで採点する。選択は ADR 付記（adr-findings）で決める。
- 仮説 4: reviewer の判断だけに頼らない。acceptance に決定的な検査（tree の `out/<id>.txt` の存在など）を持たせ、`## Deterministic checks` として reviewer に渡すか、その検査が落ちたら合格を適用しない。reviewer の verdict 本文（reason）を facts か ops に残し、次の pilot で判断の根拠を読めるようにする。
