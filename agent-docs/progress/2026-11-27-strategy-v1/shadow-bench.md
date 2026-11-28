---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: shadow-bench
status: done
updated: 2026-11-28
completed: 2026-11-28
---
# 戦略層 v1 親 shadow-bench: R1 を本物の controller の経路で shadow にして long-horizon bench C01〜C10 で測る

## 結論

- 判定は `falsified: false`・`inconclusive: false`（`shadow-bench.json`）。plan_issue は申告しない。
  - 10 case すべてで、必要な Outcome に全部到達した。途中到達のある case は 5 件（C03・C04・C07・C08・C09）で、3 件以上なので inconclusive ではない。
  - `pc/r1-stall` の発火は 0 点。誤検知（全部到達より前の発火）は 0 case。F2 は当たらない。
  - F1: controller が記録した途中到達 7 件で、次の snapshot の `no_progress_streak` はその間に終端になった root の数（7 件とも 0）と一致した。違反は 0 件。R1 の no-progress の部分（到達で連続が 0 に戻ること）を、実の controller の経路で初めて確かめた。
- 効果 0 を確かめた（10 case の合計）: daemon が起票した root 0、controller の問い 0、`project_controller:` の受信箱の源 0、CoS 入力 0、効果のある判断 0。判断は 254 件が記録され、うち 25 件が `suppressed`（効果を抑えた）。台本の木・人の操作・受信箱の通知の数は、同じ課題の `old.proactive` と一致した（試験の assert）。
- R1 を正の信号として試せたのは C10 の 1 件だけで、そこでは既存の guard が先に効いた。
  - C10（同じ失敗の繰り返し）では、33,300 秒に 3 件目の root が失敗した。次の tick（33,600 秒）で Rust の既存の guard `pc/guard/same-failure` が TS を呼ばずに escalate を出した（shadow なので抑えられた）。その後 controller は 90,300 秒まで評価しなかった。
  - 90,300 秒の snapshot では stall が escalate（終端 4・連続 4）だった。しかし root o1:4 が 90,000 秒に o1 を満たしていて到達が未記録だったので、TS は r1-ts の設計どおり記録（`pc/milestone-reached`）を先に出し、連続は次の snapshot で 0 に戻った。
  - 同じ記録に Python の規則（`r1_bench.judge_trial`、K = 3）を当てると、C10 は 3 件目の終端（33,300 秒）で発火し、全部到達（90,000 秒）より前なので定義上は誤検知 1 case になる。controller の経路で発火していても、F2 の 2 件には届かない。
- この bench の範囲では、R1 は既存の no-progress の仕組み（`pc/guard/same-failure`）に信号を足していない。反証はされていないが、R1 が役に立つことも示していない（「限界」）。
- r1-bench の予測との一致: r1-bench.json の同じ case の `old.proactive` の trial は 10 件とも除外されている（`root_terminal_time_missing`・`outcome_reach_time_missing`）ので、予測は無く、一致は判定できない（`r1_bench.matches: null`）。Python の規則と controller の発火（終端の子の数の集合）は 9 case で一致し、C10 だけ違う（上の guard による）。

## 実装前 brief

1. 根本原因: 本番の DB には Roadmap・Outcome の記録が 0 件で、R1 の shadow の記録が取れない。long-horizon bench には腕 shadow が無い。さらに `parent_checkpoint::stall` は strategy root の直下の子しか数えない。到達の根拠になるのは案件の root（`parent_id` の無い task）だけなので、台本の root をそのまま流しても R1 は計算されない（checkpoint が空になる）。
2. 責務 owner: bench の試験（`crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`）と、新しい子 module `long_horizon_shadow.rs`。R1 の値は Rust の `parent_checkpoint::stall`、規則は TS の `pc/r1-stall`、効果の抑制は Rust の門で、どれも変えない。
3. 既存の再利用先: `Trial`（本物の store・dispatcher・`TsProjectDecider`・仮想時刻）、`RealTs`（本物の bundle を本物の V8 で評価）、台本の CoS（`old.proactive` の `proactive_next`）、`r1_bench.judge_trial`（F1・F2 の写像。`shadow_bench.py` が importlib で読む）、`cos_operation_apply_at`（strategy の参照を持つ `task.create` の行。task-ops の `strategy_root` の fixture と同じ形）、`append_event_tx_at`（仮想時刻の event）。
4. 選択肢:
   - (a) 台本の root をそのまま流す → strategy root が無く R1 は計算されない。測定にならないので採らない。
   - (b) 台本の root を strategy root の子にする → 到達は案件の root しか根拠にならないので、途中到達が 0 件になり、no-progress の部分を試せない。採らない。
   - (c) strategy root を 1 件置き、台本の root 1 件ごとに直下の子（写し）を作り、root が終端になった仮想時刻に同じ終端へ遷移させる → 採用。R1 の写像（案件 = 親、root = 子。r1-bench と同じ）を本物の snapshot → TS → Rust の門に通せる。台本の root はそのまま案件の root なので、到達も本物の経路（`mark_milestone` の判断。shadow では `suppressed` を到達に数える）で記録される。
5. 新しい状態・分岐・fallback・port: production には無し（tests/ の中だけ）。試験には腕 `Arm::Shadow`（mode `shadow`・台本は proactive）、`Trial.shadow`（strategy root・写し・時刻の記録）、`RealTs.checkpoints`（TS に渡った snapshot の checkpoint の写し）を足した。
6. 撤去対象: 無し。
7. 不変条件と試験（`long_horizon_shadow_` の 10 件）:
   - 効果 0: daemon が起票した root・controller の問い・`project_controller:` の受信箱の源・CoS 入力・効果のある判断が 0 件、判断が 1 件以上、`suppressed` が 1 件以上。
   - 台本の木・人の操作・invocation・actor・発言・問い・CoS の拒否・`achieved_at`・受信箱の通知の数が、同じ課題の `old.proactive` と一致する（controller の効果が台本に届いていない）。
   - 同じ seed の 2 回で facts と記録の bytes が一致する。追記専用・持ち主の無い running 0（他の腕と同じ）。
   - 既存の試験で検出できない理由: 既存の 40 件は mode `off`（old.*）と `enabled`（new・ctl.null）だけで、mode `shadow` の効果の抑制と、strategy root のある snapshot を本物の TS に通す経路を見ない。
   - shadow で効果が出たら試験の失敗になる（反証ではなく不具合）。

## 変更

- `crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`: `Arm::Shadow`、`RealTs.checkpoints`、`Trial.shadow`、hook 6 か所（`shadow_start`・`shadow_track`・`shadow_status`・`shadow_observe`・`shadow_reach`・`adopt_controller_roots` の `shadow_owns`）、`run` から step の loop を `steps` に分けた。既存の腕では hook は何もしない。既存の腕の assert・`bench`・oracle・public の fixture は変えていない（`bench` は腕 shadow を受け付けない assert だけ足した）。
- `crates/task-dispatch/src/dispatcher/tests/long_horizon_shadow.rs`（新規）: strategy root と写し、記録 `celeris.long-horizon-shadow/1`、試験 10 件。
- `agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh`・`shadow_bench.py`・`shadow-bench.json`・`shadow-bench/raw/shadow/C01〜C10.json`・この file。

記録の欄（raw）: 判断の行ごとの時点（試行の始めからの秒）・rule id（行の `validation`。TS の規則・Rust の guard・門の置き換えの最終の id）・TS の規則 id・guard の名前・行動の種類・key・効果の状態、root の遷移と時刻、Outcome ごとの最初の到達（台本の木の判定）、TS に渡った strategy root の `stall`（値が変わった時点だけ。大域の event id の `at` は落とす）、効果の数。

## 結果（`shadow-bench.json`）

| case | 終端の root | 途中到達 | 全部到達 | `pc/r1-stall` | snapshot の escalate | guard | Python の規則（K = 3） |
|---|---|---|---|---|---|---|---|
| C01 | 1 | - | 3,900 | 0 | - | - | 発火なし |
| C02 | 2 | - | 11,400 | 0 | - | - | 発火なし |
| C03 | 2 | 65,100 | 69,000 | 0 | - | achieved-quiet | 発火なし |
| C04 | 3 | 7,800 | 11,700 | 0 | - | - | 発火なし |
| C05 | 1 | - | 12,600 | 0 | - | - | 発火なし |
| C06 | 1 | - | 263,100 | 0 | - | - | 発火なし |
| C07 | 2 | 7,500 | 11,400 | 0 | - | - | 発火なし |
| C08 | 4 | 3,900・7,800・11,700 | 15,600 | 0 | - | achieved-quiet | 発火なし |
| C09 | 2 | 5,400 | 9,300 | 0 | - | - | 発火なし |
| C10 | 4 | - | 90,000 | 0 | 90,300 | same-failure | 3 件目（33,300 秒）で発火 |

時刻は試行の始めからの秒（仮想時刻）。判断の rule の内訳は raw にある（例: C08 は `pc/root-in-flight` 4・`pc/milestone-reached` 4・`pc/checkpoint/evaluate` 1・`pc/achieved` 1・`pc/guard/achieved-quiet` 1）。

## 限界

1. bench は台本で、本番の案件と分布が違う。課題は 10 件・seed 1 つ・腕 shadow 1 つで、R1 が発火しうる停滞（到達の前に root が 3 件以上終端になる）は C10 の 1 件しかない。誤検知の率は出せない。
2. strategy root と写しの子は bench の作り物。実の案件では strategy root の子（実験）と Outcome を満たす root は別の task で、子が Outcome を満たしても到達には数えられない。この写像では「root 1 件の終端 = 子 1 件の終端」になる。
3. strategy root には戦略の文書の bytes が無い（参照の行だけ）。それでも `pc/checkpoint/evaluate` が 6 件出た（shadow で抑えた）。文書がある場合の checkpoint の挙動は試していない。
4. 腕 shadow は strategy root と写しを DB に足すので、controller が見る snapshot は `old.proactive` の DB と同じではない（台本の木と人の操作は同じ）。
5. 途中到達の時刻は台本の木の判定（`out/<id>.txt` の内容）で、採点器の隠し受入ではない。controller が記録した到達（`mark_milestone`）は、台本の到達の次の tick（300 秒後）になる。
6. C10 で R1 が出なかった理由（guard が先に効き、その後の評価が 90,300 秒まで無かった）は記録から読んだもので、R1 だけを外した対照は流していない。

## 再現

repo の root で実行する（LLM・network・本番 DB を使わない）。

```sh
sh agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh record   # raw と shadow-bench.json を書き直す
sh agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh check    # raw を $TMPDIR に書いて commit 済みと比べる（exit 0/1）
python3 -I agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py verify   # raw から再計算して byte 一致
```

- bundle は `$TMPDIR` に作る。`pnpm -C control-ts bundle` と同じ `build-bundle.mjs` の `buildBundle` を、出力先だけ変えて呼ぶ（pnpm の script は `control-ts/dist` に書くので使わない）。
- 試験は `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で流すので、bundle が無ければ skip せずに落ちる。
- 試験名は `long_horizon_shadow_` で始まり、`scripts/dev/test-long-horizon-bench.sh` の filter（`long_horizon_bench_`）に入らない（40 件の採点の数は変わらない）。

## 試験と検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `sh agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh record`: exit 0（nextest 10 passed、`run: cases=10 fires=0 midway=5 inconclusive=False falsified=False`）。
- `sh agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.sh check`: exit 0（nextest 10 passed・`compare: ok`。実行後の `git status --short` は 0 行）。
- `python3 -I agent-docs/progress/2026-11-27-strategy-v1/shadow_bench.py verify`: exit 0（`verify: ok`）。
- `bash scripts/dev/test-affected.sh`: exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、nextest 4,038 passed・11 skipped、doc test の失敗 0、script bundle の SKIP 0 件、repo-health verdict=pass・fail_grade=0・new 0・worsened 0。
- `cargo clippy --workspace -- -D warnings`: exit 0（警告は vendor の deno_core の既存の 1 件だけ）。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`・`bash scripts/dev/check-doc-links.sh`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は上の「変更」の file だけ。`tests/fixtures/long-horizon`・`crates`（`crates/task-dispatch/src/dispatcher/tests/` を除く）・`control-ts`・`web`・`scripts`・`docs`・`Cargo.toml`・`Cargo.lock` の差分は 0 file。
- LoC: production の差分は 0 行（変更は試験の dir と進捗の dir だけ）。試験は `long_horizon_bench.rs` +約 50 行・`long_horizon_shadow.rs` 新規、進捗の script は `shadow_bench.py`・`shadow_bench.sh` 新規（行数は `git diff --numstat "$CELERIS_WU_BASE" HEAD` で見られる）。

## 未確認

- 本番の案件での R1 の発火と誤検知（本番の DB に Roadmap・Outcome の記録が無い）。
- R1 が既存の guard（`pc/guard/same-failure`・`pc/no-progress`）より先に、または guard が捕まえない停滞で役に立つ場面。この bench には無かった。

## 提案

- 決定 after-shadow の材料: R1 は反証されず、shadow で効果 0 だったが、この bench では既存の guard に信号を足していない。本番 shadow に進めても、本番に Roadmap・Outcome の記録が無い間は R1 の到達の部分が働かない（到達が数えられず、連続は終端の子の数と同じになる）。
