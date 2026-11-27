---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: r1-bench
status: done
updated: 2026-11-27
completed: 2026-11-27
---
# 戦略層 v1 親 r1-bench: R1 の no-progress の連続が Outcome の途中到達で 0 に戻るかを long-horizon の記録 44 trial で確かめる

## 結論

- 判定は **inconclusive**（`inconclusive: true`、`falsified: false`）。44 trial の全部を、事前に固定した規則で除外した。途中到達のある trial は 0 件で、3 件に届かない。反証の条件 F1・F2 はどちらも試せていない。
- 除外の理由: commit 済みの facts（`celeris.long-horizon-facts/1`）は、root の終端の時刻も、Outcome ごとの到達の時刻も持たない。
  - root の `statuses` は時刻の無い文字列の列（例: `["ready", "running", "done"]`）。root 78 件のうち、終端の時刻がある root は 0 件。
  - Outcome ごとの到達の欄は無い。`achieved_at` は、必要な Outcome が全部到達した案件単位の時刻だけ（32 trial で値あり、12 trial で null）。途中到達の時刻は出せない。
  - 時刻を `invocations`・`created_at`・`achieved_at` から補うのは推測になるので、objective どおり補っていない。
- R1 の no-progress の部分（到達で連続が 0 に戻ること）は、r1-replay（到達の記録が無かった）に続いて、実の記録ではまだ確かめられていない。確かめたのは合成の facts の試験だけ。
- falsified でないので plan_issue は申告しない。計画 v2 の次の段（r1-core・r1-ts で controller に shadow で載せ、shadow-bench で測る）は止めない。no-progress の部分を実の記録で試せるのは shadow-bench が最初になる（「提案」の 1）。
- LLM の呼び出しは 0 回、本番 DB の読み書きは 0 件、code の変更は 0 行。記録（long-horizon の dir）と子 0 の fixture は変えていない。1 回の実行は約 0.05 秒。

## 写像と判定（計画で事前に固定。結果を見て変えていない）

規則は `r1_rule.py` の `streak_after_reach` と `escalate` を importlib で読んで使う（写していない。`r1_rule.py` は変えていない）。

- 案件 = 親、root = 子。root が終端 status（done・failed・cancelled。`crates/campaign/src/long_horizon.rs` の `root_status_stays` と同じ集合）になった時点を 1 点とし、時刻の順（同じ時刻は logical_id の順）に並べる。
- 点ごとに出す値:
  - `outcome_reached`: 必要な Outcome が 1 件以上あり、全部がその点の時刻までに到達していれば真。
  - `terminal_children`: その点までに終端になった root の数。
  - `no_progress_streak`: 到達した必要な Outcome の数が最後に増えた時刻より後に終端になった root の数（増えていなければ `terminal_children`）。
  - `escalate`: K = 3 が判定。K = 2・4 は感度で、判定に使わない。
- 時刻を facts から取る形: root の `statuses` の要素が `{"status", "at"}` の object なら、最初の終端の要素の `at` を終端の時刻とする。Outcome ごとの到達は `outcomes`（`{"id", "required", "reached_at"}` の列）から取る。実の facts はどちらの形も持たない。
- 除外の理由の code（1 trial に複数つく）: `no_roots`・`no_terminal_root`・`root_terminal_time_missing`・`outcome_reach_time_missing`・`no_required_outcome`。r1-bench.json の各 trial の `exclusion` に code と文、`evidence` に根拠の数（root 数・終端の root 数・時刻のある終端の root 数・statuses の形・outcomes の欄の有無・achieved_at）がある。
- 判定:
  - 途中到達: 必要な Outcome の到達のうち、その時刻でまだ全部は到達していないもの。
  - F1: 途中到達のある trial で、到達の直後（到達の時刻までに終端になった root と、その時刻までの到達で計算した値）の `no_progress_streak` が 0 にならない到達がある。
  - F2: 必要な Outcome に全部到達した trial のうち、到達より前の点で K = 3 の発火がある trial が 2 件以上。
  - F1 か F2 に当たれば falsified。途中到達のある trial が 3 件未満なら inconclusive とし、falsified にしない。
  - arm ごとの誤検知: 全部到達した trial で、到達より前に発火した点。

## 結果

| 入力の set | trial |
|---|---|
| a8b/replay | 40 |
| a9/stub | 2 |
| a9/llm | 2 |

| arm | trial | 除外 | `no_roots` | `root_terminal_time_missing` | `outcome_reach_time_missing` | `achieved_at` あり |
|---|---|---|---|---|---|---|
| ctl.null | 10 | 10 | 8 | 2 | 10 | 2 |
| new | 13 | 13 | 0 | 13 | 13 | 10 |
| old.passive | 11 | 11 | 1 | 10 | 11 | 10 |
| old.proactive | 10 | 10 | 0 | 10 | 10 | 10 |
| 計 | 44 | 44 | 9 | 35 | 44 | 32 |

- 含めた trial 0 件、途中到達のある trial 0 件、全部到達した trial 0 件。点は 0 点で、K = 2・3・4 のどれでも発火は 0 点。
- F1: 当たらない（試せる trial が無い）。F2: 当たらない（K = 2・3・4 のどれも 0 件）。arm ごとの誤検知は全 arm で 0 trial・0 点（試せる trial が無いことによる 0 で、誤検知が無いことを示さない）。
- 参考（判定に使わない。時刻を推測しない数え上げ）: done の root を持つ Outcome が 2 件以上ある trial は 19 件（new 8・old.passive 5・old.proactive 5・ctl.null 1。case は C08 5・C09 4・C03 3・C04 3・C07 3・C06 1）。終端の時刻と Outcome ごとの到達の時刻が記録されていれば、これらが途中到達の候補になる。

## 子 0 の replay・R1（r1-replay）の結果との比較

| | r1-replay（子 0 の replay 12 点・研究 7 点） | r1-bench（long-horizon 44 trial） |
|---|---|---|
| 入力の時刻 | slice の cutoff と event の時刻はある | root の終端の時刻も Outcome ごとの到達の時刻も無い |
| Outcome の到達の記録 | 19 点のどれにも無い | 案件単位の `achieved_at` だけ（32 trial） |
| 点の数 | 19 点 | 0 点（44 trial を除外） |
| K = 3 の発火・誤検知 | replay は C-done で初めて発火、誤検知 2 点（C-done・D-mid）。研究は 11.17 で初めて発火、誤検知 3 点（11.17・11.18・11.19）。見逃しは 0 点 | 0 点（試せない） |
| no-progress の部分 | 全点で `no_progress_streak` = `terminal_children`。試せていない | 試せていない |
| 人の反証条件 | 当たらない（`falsified: false`） | inconclusive（`falsified: false`） |

r1-replay では、R1 は「K 個目の子が終端になったら発火する」数え上げと区別できなかった。r1-bench でも、到達で連続が 0 に戻ることを実の記録で示せなかった。R1 の誤検知の数（replay 2 点・研究 3 点）は r1-replay のまま変わらない。

## 限界

1. 実の記録では何も判定できていない。inconclusive は「R1 が残った」ことを意味しない。r1-replay の結果（反証条件に当たらない）に新しい根拠を足していない。
2. 時刻の取り方の形（`statuses` の object の `at`・`outcomes` の `reached_at`）は、実の facts に無い形を r1_bench.py が決めたもの。今後の記録がこの形で時刻を持つかは未確認。別の形なら `extract` を直す必要がある。
3. F1 は、到達の時刻での `streak_after_reach` の値を見る。到達の時刻より後に終端になった root を数えない関数の定義から、正しい実装では必ず 0 になる。F1 が検出するのは、到達を無視するなどの streak の実装の誤りだけ（試験の否定対照で確かめた）。R1 の誤検知の主な試験は F2 になる。
4. 必要な Outcome の集合は facts の `outcomes` の `required` から取る。実の facts では oracle（隠し受入）にしか無く、r1-bench は oracle を読まない。
5. 参考の 19 件は done の root を Outcome の到達とみなした数で、隠し受入の到達とは違いうる（判定に使っていない）。

## 提案

1. no-progress の部分を実の記録で試すには、root の終端の時刻と Outcome ごとの到達の時刻が要る。計画 v2 の shadow-bench（R1 を本物の controller の経路で shadow にして C01〜C10 で測る）が、点ごとに controller の snapshot（終端の子の数・到達した Outcome の数・`outcome_reached`）を時刻つきで残せば、この script の F1・F2 をそのまま当てられる。shadow-bench の計画で、snapshot の記録をこの形にそろえるかを決める（決めるのは計画の承認で人）。
2. long-horizon の facts の export（`celeris.long-horizon-facts/1`）に root の状態の遷移の時刻を足すかは、この WorkUnit の範囲外（code の変更 0 の条件）。必要なら別の計画で扱う。

## 再現

repo の root で実行する（stdlib だけ。network・LLM・DB を使わない）。

```sh
python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py run      # r1-bench.json を書く
python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py verify   # 再計算と byte 一致・k=3・欄・除外の理由（exit 0/1）
python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_bench.py     # 17 件
```

- `r1_bench.py` は `r1_rule.py` を importlib で path から読む（`r1_rule.py` が子 0 の `replay.py`・`research_case.py` を読むので、それらも読み込まれる。slice は作らない）。r1-bench.json の `rule.r1_rule_sha256` と `inputs.r1_result_sha256` が、使った `r1_rule.py` と r1-result.json の版を示す。
- 入力の 44 個の facts は schema が `celeris.long-horizon-facts/1` であること、arm-map の case・seed（a8b は trial も）が facts と一致することを確かめてから使う。各 trial の facts の sha256 を `facts_sha256` に、全体の digest を `inputs.facts_digest_sha256` に書いた。
- 試験 `test_r1_bench.py` の守る不変条件:
  - 写像: 途中到達で streak が 0 に戻り、その後に終端になった root だけを数える。同じ時刻の root の順と、必要でない Outcome を数えないこと（合成の facts）。
  - 除外: 実の形の facts（文字列の `statuses`、`outcomes` の欄なし）は `achieved_at` があっても除外する。roots 0 件・終端の root なし・時刻の無い root が 1 件でもある・必要な Outcome 0 件の各理由。
  - F1: 到達を無視する偽の streak の関数を差し込むと当たる（否定対照）。正しい関数では当たらない。
  - F2: 到達の前に発火した trial が 1 件では当たらず、2 件で当たる。途中到達の trial が 3 件未満なら F2 に当たっても falsified にしない。
  - arm ごとの誤検知の数え方。規則の関数が `r1_rule.py` から来ること（`r1_bench.py` に定義が無い）。
  - `verify` は k の書き換え・除外の理由の欠け・欄の欠けで落ち、実の 44 trial で r1-bench.json と byte で一致する。
  - 既存の試験では検出できない理由: `r1_bench.py` は新しい script で、`test_r1_rule.py` は子 0 の slice の写像だけを試し、long-horizon の facts の写像・除外・F1・F2 を見ない。

## 検査（提出時の HEAD で実行）

- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py run` → exit 0。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_bench.py verify` → exit 0（再計算が r1-bench.json と byte で一致。trials=44・included=0・midway=0・inconclusive=True・falsified=False）。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_bench.py` → exit 0、17 件 OK。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py verify` → exit 0（r1-result.json と byte 一致。`r1_rule.py` は変えていない）。`python3 -I agent-docs/progress/2026-11-27-strategy-v1/test_r1_rule.py` → exit 0、12 件 OK。
- 記録・fixture・code の差分: `git diff --stat "$CELERIS_WU_BASE" -- agent-docs/progress/2026-11-04-long-horizon agent-docs/progress/2026-11-21-strategy-v1-0 crates control-ts web scripts docs Cargo.toml Cargo.lock agent-docs/progress/2026-11-27-strategy-v1/r1_rule.py agent-docs/progress/2026-11-27-strategy-v1/r1-result.json` → 0 行。
