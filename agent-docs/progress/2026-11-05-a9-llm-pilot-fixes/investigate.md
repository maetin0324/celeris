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
