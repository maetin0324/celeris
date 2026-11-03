---
title: S6 — 旧 Rust 経路と TS 経路の edit→activate の実測・自己改善 pilot・移行範囲の確定
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: blocked
updated: 2026-11-03
---
# S6 — edit→activate の実測・自己改善 pilot・移行範囲の確定

更新日: 2026-11-03（unit s6-report）。task 01M6FPPJN7A8H427MXFVYFY7H4（S5→S6）の S6 の報告である。S5 の記録は [S5 の進捗](2026-11-02-ts-control-layer-s5.md)、設計は [境界 ADR](../adr/2026-11-02-ts-control-layer-boundary.md) の (f)(g)・付記 S5-10・S5-11 と付記 S6。各 unit の記録と生データは [2026-11-02-ts-control-layer-s6/](2026-11-02-ts-control-layer-s6/) にある。

status を blocked にしたのは、旧 Rust 経路 `cos-digest` の試行が 14/20 で未達だからである（下の「未達」）。

## 結論
- TS の経路の edit→activate は、warm の p50 が 7.47〜7.52 秒、p95 が 7.60〜7.69 秒で、(f) の目標（p50 30 秒・p95 60 秒）と S6 の打ち切り基準（warm p50 78 秒）の内にある。cold の p95 は 7.795 秒（目標 120 秒）。
- 旧 Rust 経路は、`policy` が 20 試行で p50 234.481 秒・p95 236.162 秒。`cos-digest` は **14 試行で未達（14/20）**、14 試行の p50 は 344.491 秒・p95 は 346.576 秒。
- 決定の一致は、比較 fixture の 3 領域で全件一致（`lane_rules` 19,683/19,683、`route_priority` 2,026/2,026、`cos_digest` 2,088/2,088）。**本番の一致率は未計測**（本番は全領域 `rust` のまま）。
- 自己改善 pilot は非本番の daemon で 1 件通した（[pilot](2026-11-02-ts-control-layer-s6/pilot.md)）。本番は使っていない。
- **本番で標準稼働した領域は 0**。3 領域（`lane_rules`・`route_priority`・`cos_digest`）は本番で `rust` のままで、人が「本番への移行」の手順で上げるまで変わらない。
- 二重実装を整理する終了条件を境界 ADR の付記 S6 に書いた。今はどの領域も条件を満たさないので、Rust の参照実装は消さない。

## 未達
- `cos-digest` warm の試行が 14/20。1 試行が約 345 秒で、prime の約 415 秒と合わせて 1 run（3600 秒）に 7 試行しか入らなかった（[m-cosd-a](2026-11-02-ts-control-layer-s6/m-cosd-a.md)・[m-cosd-b](2026-11-02-ts-control-layer-s6/m-cosd-b.md)）。足りない 6 試行を推定で埋めていない。
- そのため `python3 -I scripts/dev/edit-activate-summary.py agent-docs/progress/2026-11-02-ts-control-layer-s6/*.jsonl` は exit 1（`cos-digest warm: 14 trials < 20`）。WU の条件 0 と task の条件 2 は未達である。
- 埋める手順: 測定 leaf を 1 本足し、`bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 6 --rep-start 15 --series m-cosd-c --out agent-docs/progress/2026-11-02-ts-control-layer-s6/cos-digest-warm-c.jsonl` を直列に走らせる（約 415＋6×345＝2,485 秒）。その後 `cos-digest-warm.jsonl` に行を変えずに連結し、summary を再実行する。s6-report の run の壁時計（3600 秒）では報告・検査と並べられないので、この run では測っていない。

## 生データと集計（旧 Rust 経路と TS 経路の p50・p95）
集計は `python3 -I scripts/dev/edit-activate-summary.py agent-docs/progress/2026-11-02-ts-control-layer-s6/*.jsonl`（この unit で作った。前半・後半の file と連結した file を同時に渡しても、同じ記録は 1 回だけ数える）。p50・p95 は最近傍順位。

| 経路 | scenario・mode | 試行 | p50（秒） | p95（秒） | 最小〜最大（秒） | 生データ |
|---|---|---|---|---|---|---|
| Rust（旧） | `policy` warm | 20 | 234.481 | 236.162 | 233.488〜236.383 | [policy-warm.jsonl](2026-11-02-ts-control-layer-s6/policy-warm.jsonl)（= [-a](2026-11-02-ts-control-layer-s6/policy-warm-a.jsonl)＋[-b](2026-11-02-ts-control-layer-s6/policy-warm-b.jsonl)） |
| Rust（旧） | `cos-digest` warm | **14（未達）** | 344.491 | 346.576 | 343.005〜346.576 | [cos-digest-warm.jsonl](2026-11-02-ts-control-layer-s6/cos-digest-warm.jsonl)（= [-a](2026-11-02-ts-control-layer-s6/cos-digest-warm-a.jsonl)＋[-b](2026-11-02-ts-control-layer-s6/cos-digest-warm-b.jsonl)） |
| TS | `policy-ts` warm | 20 | 7.519 | 7.694 | 6.747〜7.722 | [policy-ts-warm.jsonl](2026-11-02-ts-control-layer-s6/policy-ts-warm.jsonl) |
| TS | `cos-ts` warm | 20 | 7.470 | 7.604 | 7.053〜7.608 | [cos-ts-warm.jsonl](2026-11-02-ts-control-layer-s6/cos-ts-warm.jsonl) |
| TS | `cos-ts` cold | 20 | 7.622 | 7.795 | 7.052〜7.835 | [cos-ts-cold.jsonl](2026-11-02-ts-control-layer-s6/cos-ts-cold.jsonl) |

- どの試行も exit 0。`over_300s` は `cos-digest` の 14 試行すべて、busy（自分以外の cargo・rustc）は 0 件。TS の試行は全件 `cargo_invocations` 0・`link_count` 0。
- file 名は付記 S5-10 の表（`policy.jsonl`・`cos.jsonl`・`policy-ts.jsonl`・`cos-ts.jsonl`）でなく、測定 unit が置いた名前のまま使う。対応は付記 S6 の (1)。中身の `scenario` は S5-10 のとおり `cos-digest` で、S0 の `cos`（`job_wait_extend`）とは別の経路である。
- 検証の範囲は同じでない: Rust の経路は `task-core` の全試験と release build を含み、TS の経路は領域の試験・比較 fixture・golden・統合試験・activate までである（付記 S5-10。測定のために Rust 側を狭めていない）。
- S0 の baseline（policy 221.0 秒・cos 155.7 秒、3 試行）は S1〜S5 でコードが増える前の値で、比較の相手にしない（[m-policy-a](2026-11-02-ts-control-layer-s6/m-policy-a.md) の提案）。

## warm の短縮（言語移行の比較）
| 対 | Rust p50 | TS p50 | 差 | TS ÷ Rust |
|---|---|---|---|---|
| `policy` → `policy-ts` | 234.481 | 7.519 | −226.962 秒 | 3.2% |
| `cos-digest`（14 試行）→ `cos-ts` | 344.491 | 7.470 | −337.021 秒 | 2.2% |

- `cos-digest` の行は 14 試行の値で、20 試行が揃うまで確定しない。
- この表は言語移行の比較だけである。pilot の方針改善の数値（下の「自己改善 pilot」）は混ぜない（(g)）。

## S6 の打ち切り基準の判定
- 基準（(f)）: TS の経路の warm p50 が 78 秒を超えたら次の領域の移行を止める。
- 判定: 超えていない。`policy-ts` 7.519 秒、`cos-ts` 7.470 秒で、78 秒の 9.6%・9.6%。打ち切りはしない。

## TS の typecheck 秒
| 系列 | typecheck p50 / p95（秒） | bundle p50 / p95（秒） |
|---|---|---|
| `cos-ts` warm | 0.095 / 0.097 | 0.075 / 0.079 |
| `policy-ts` warm | 0.096 / 0.103 | 0.075 / 0.079 |
| `cos-ts` cold | 0.095 / 0.099 | 0.075 / 0.079 |

- 段は `control-ts/node_modules/.bin/tsc --noEmit -p control-ts/tsconfig.json`（`scripts/dev/ts-verify.sh` の 440 行目）。`control-ts/package.json` の TypeScript は 7.0.2、`tsconfig.json` に `incremental` は無い。0.1 秒以下なのは 7 系の native 実装によると推定するが、確かめていない（[m-ts-warm](2026-11-02-ts-control-layer-s6/m-ts-warm.md) の未解決）。
- 1 試行の時間の大半は integration（約 4.4 秒）と unit（約 1.6 秒）である。

## JS 実行・境界・queue の overhead
[overhead](2026-11-02-ts-control-layer-s6/overhead.md) の測定（script-bench、各 1000 回×3 run の最大、ms、p50 / p99）。

| 領域 | 全体 | queue | 境界（入） | JS 実行 | 境界（出） | 応答の起床 |
|---|---|---|---|---|---|---|
| `lane_rules` | 0.064 / 0.094 | 0.017 / 0.028 | 0.008 / 0.013 | 0.013 / 0.033 | 0.003 / 0.005 | 0.018 / 0.027 |
| `route_priority` | 0.086 / 0.233 | 0.017 / 0.027 | 0.018 / 0.030 | 0.020 / 0.155 | 0.005 / 0.007 | 0.018 / 0.027 |
| `cos_digest` pass（op 21 回） | 1.731 / 2.246 | 0.017 / 0.075 | 0.009 / 0.020 | 1.476 / 1.928 | 0.197 / 0.264 | 0.019 / 0.029 |

- (f) の判定 1 回の遅延（`host_selftest`）は p50 0.087 ms・p99 0.129 ms で、閾値（1 ms・5 ms）の内。
- tick の遅れ（付記 S5-8）は非本番 daemon の 59 tick で p95 4.0 ms・最大 5.6 ms、`tick_budget` 0 件（許容 200 ms）。
- `cos_digest` の op の答えは記録から返したので、本番の store の読み出しの時間は入っていない。

## RSS・isolate 数
- baseline（`[script]` なし）の daemon は 45.9〜46.6 MiB。
- generation 1 個（isolate 1 個）で +22.5〜23.2 MiB（閾値 100 MiB）、generation 2 個（active と draining、isolate 2 個）で +26.0〜27.3 MiB（閾値 200 MiB）。
- 旧 generation の退役で isolate は 1 個に戻るが、RSS は 0.3〜0.5 MiB しか減らない。退役を繰り返したときの推移は未確認。

## cold build の増分
- `celeris` の cold release build: 131.3 秒（S0）→ 141.1 秒（+9.8 秒、閾値 +60 秒）。各 1 試行。
- warm の `cargo build -p celeris` の link: 1.36 → 1.795 秒（+0.44 秒、閾値 5 秒）。
- release binary: 79.6 → 146.6 MiB（+67.0 MiB、閾値 80 MiB、余裕 13.0 MiB）。
- `task-dispatch`・`task-api` の test-build の増分（閾値 2 秒）は**未計測**。
- TS の経路の cold（`node_modules` を捨てる定義）の増分は install の 0.2 秒（cold p50 7.622 秒 − warm 7.470 秒 = 0.152 秒）。pnpm の store が空の初回 clone と runner の作り直しは測っていない（[m-ts-cold](2026-11-02-ts-control-layer-s6/m-ts-cold.md)）。

## full regression の総コスト
- release gate の記録（`/local/celeris/state/releases/*/gate.json` を測定 script が読むだけで写した `kind=estimate` の行。release 2026-10-31T23:12:48Z、ok）: 10 段の合計 **648.145 秒**。内訳は `cargo-test` 575.619、`cargo-clippy` 41.670、`web-pnpm-test` 10.658、`cargo-fmt-check` 7.276、`source-size-report` 6.234、`web-pnpm-release` 3.630、`web-pnpm-typecheck` 1.359、`cargo-build` 1.135、`web-pnpm-install` 0.562、`web-config` 0.002 秒。
- TS の変更も出荷の前にはこの release gate を通る。(f) のとおり TS 化はこの費用を短縮の対象にしていない。S5 を含む release の gate はまだ無いので、TS 制御層の試験が gate に足した秒は未計測（次の release の `gate.json` で `cargo-test` を比べる）。

## 決定一致率
| 出所 | 結果 |
|---|---|
| 比較 fixture（この unit が提出時の HEAD で `node control-ts/scripts/compare.mjs <領域>` を実行） | `lane_rules` 19,683/19,683、`route_priority` 2,026/2,026、`cos_digest` 2,088/2,088（`cases.jsonl` 2,066＋`pass_traces.jsonl` 22）。一致率 100% |
| TS の経路の測定（ts-verify の golden・integration） | `policy-ts`・`cos-ts` の 60 試行すべて exit 0 |
| 試験 | S5 の e2e・dispatcher の試験（[S5 の進捗](2026-11-02-ts-control-layer-s5.md) の条件 0・4）。e2e は記録の `outcome` を検査する |
| pilot の compare | 改善前と候補で、fixture 2,066 件の差分 0。改善が当たる入力 15 件で差分 9 件（意図した方針の変更で、言語移行の不一致ではない） |
| 本番 | **未計測**。本番は全領域 `rust` で compare の記録が無い |

- 本番の一致率を取る人の手順: [S5 の進捗](2026-11-02-ts-control-layer-s5.md) の「本番への移行」の 4.（登録）と 5.（compare）を行い、5. の `sqlite3 -readonly` の 2 本で `outcome` ごとの件数を数える。一致率は `match ÷ (match + mismatch + invalid_output + ts_error)`（`unavailable` は分母に入れない。付記 S2-4）。

## task 成功率
`celerisctl show 01M6FPPJN7A8H427MXFVYFY7H4`（2026-11-03、この run の途中で読んだ）の event から数えた。
- WorkUnit: 計画の 32 unit のうち 30 が done、s6-report（この unit）が running、integrate-s6-report が pending。
- WorkUnit の check の失敗 8 回（adr-s5・bundle-skip・store-gen・api-canary・script-inbox 3 回・canary-dispatch）、自動の replan 8 回、修理の unit 2 個（core-fix・guard-fix）。どれも後の run で通った。
- worker run: 終わった 46 run の outcome は done 31・work_unit_retry 7・replan 5・continue 2・error 1（計画の検査で弾かれた planner の run）。end は Completed 32・Yielded 4・記録なし 10。
- 測定の試行の成功: 全 scenario で exit 0 の試行 94/94（20＋14＋20＋20＋20）。未達は試行の失敗ではなく時間切れ（`cos-digest` 14/20）。

## 人の介入
- task の event に人の comment・回答・決定の event は 0 件（同じ `celerisctl show` の出力の event 名で数えた）。
- 受信箱・他の経路での人の操作は、この run から読めないので未確認。
- 本番の mode・deploy は人の手順として残してあり、まだ誰も実行していない（下の「本番への移行」）。

## LLM 利用量
- TS の経路と pilot の `ts-pilot.sh` は LLM を呼ばない（llm source `fake`）。
- task の worker run（終わった 46 run の `WorkerFinished.usage` の和、この run は入っていない）: input 2,042・output 1,225,831・cache read 138,519,212・cache creation 6,310,373 token、`cost_usd` の和 83.76（claude_oauth の subscription で、harness が出した換算値。請求額ではない）。

## 失敗時の復旧
- 自動の戻し: script-guard が (f) の停止基準で `canary`・`ts` を `rust` に戻し、受信箱 `script_control` に出す（S5 の条件 4 の試験）。
- e2e（`script_generation_e2e`）: 新旧併存→切替→途中 error→rollback→再起動で、event の欠落 0・二重の副作用 0。
- pilot: canary→rust の rollback は 1.015 秒（in-flight 0 の退役まで）。再起動後の復元は canary で 0.304 秒、rollback の後は rust のまま 0.275 秒。
- known-good への戻しは手動（S5 の「本番への移行」9.）。自動で前の TS の generation に戻す経路は無い。
- 開発の流れの復旧: WorkUnit の check の失敗 8 回は、自動の replan と修理の unit（core-fix・guard-fix）で直った。

## 自己改善 pilot
- [pilot](2026-11-02-ts-control-layer-s6/pilot.md): `cos_digest` の見出しに出す run の失敗の理由を 200 文字で切る改善を、作業ツリーで編集→ts-verify（7.8 秒、cargo 0 回）→候補 bundle→一時 DB の daemon に配備・登録→compare（operator の token）→canary（試験 daemon の人の token、canary_bps 5000）→再起動で復元→rust へ rollback→再起動で復元、の 16 段で通した。全体 14.9 秒。
- 本番は使っていない。canary の前提の計数（成功 200 件）は一時 DB に synthetic に書いた（判定のコードと数値は変えていない）。評価器・hard safety gate は弱めていない。
- 分かったこと: 二重実装の間に TS だけで方針を変えると、改善が当たる入力で script-guard が不一致として数え、自動で rust に戻る。方針改善を出すには Rust の参照実装にも同じ変更を入れるか、ADR に正規化の規則を足す必要がある。

## 標準稼働した領域
- 本番: **0 領域**。`lane_rules`・`route_priority`・`cos_digest` は本番で `rust` のまま。人が「本番への移行」の手順で上げるまで変わらない。
- 非本番: 3 領域とも試験・e2e で `rust→compare→canary→ts` を通る。pilot は `cos_digest` を一時 daemon で canary まで上げて戻した。

## 残る Rust 領域
- 永続状態（SQLite・events）、scheduler（dispatcher の tick）、認可（API の主体・token）、実行（worker・harness）、TS の出力の再検証と安全条件、script-guard、generation の登録・切替・drain は Rust のまま（境界 ADR (a)。移さない）。
- 判定の領域: TS の実装があるのは上の 3 領域だけ。CoS の定型判断（`job_wait_extend` の validate。S0 の `cos` scenario）を含め、他の判定はすべて Rust だけで実装されている。
- 3 領域の Rust 参照実装（`crates/task-core/src/model_policy.rs` の `match_rule`、`llm_route::route_priority_decide`、`crates/task-core/src/chat/digest.rs` の `cos_digest_decide`）は残す。消す条件は付記 S6。

## 既知の制約
1. `cos-digest` の試行が 14/20（上の「未達」）。
2. 本番の決定一致率は未計測。canary の前提（純粋判定 1000 件、CoS 200 件）は本番の compare でしか満たせない。
3. `celerisctl script mode` は `canary`・generation の登録・停止の解除を持たず、help の文面が古い（S5 未解決 1）。canary・ts は人が `curl` で API を呼ぶ。
4. file の無い hash を登録すると `invalid` になり、同じ hash を登録し直せない（S5 未解決 2）。配備を登録より先にする。
5. `script_tick_wait` の log 行が本体に無い（S5 未解決 4）。本番の canary で tick の遅れを読むには log 行を足す変更が要る。
6. script-guard の見回りは `script_compare_log` を毎回全件読む（S5 未解決 6）。
7. 方針改善を TS だけで出すと自動で rust に戻る（pilot の「分かったこと」）。
8. release binary の閾値の余裕が 13.0 MiB。`task-dispatch`・`task-api` の test-build の増分は未計測。
9. 退役を繰り返したときの RSS、要求を抱えた drain の RSS は未確認。
10. 受信箱 `script_control` の項目が CoS の triage にも流れ、人への知らせが 2 回になる（S5 未解決 8。扱いは人の確認待ち）。

## 操作手順
- 開発（TS の判定を変える）: `bash scripts/dev/ts-verify.sh`（10 段、cargo 0 回）。改善の流れの通し: `bash scripts/dev/ts-pilot.sh --out <dir>`、検査 `--validate <dir>/record.jsonl`。
- 測定: `bash scripts/dev/edit-activate-baseline.sh --scenario <policy|cos-digest|policy-ts|cos-ts> --mode <warm|cold> --reps N [--rep-start K] --series <名> --out <jsonl>`、検査 `--validate`・`--check-targets`、まとめ `python3 -I scripts/dev/edit-activate-summary.py <jsonl>...`。測定は付記 S5-11 の規則（直列・clean な作業ツリー・重い作業と並べない）で行う。
- 本番の昇格・監視・rollback: [S5 の進捗](2026-11-02-ts-control-layer-s5.md) の「本番への移行」1.〜10.（人が実行）。

## 次の優先対象
1. `cos-digest` warm の残り 6 試行（上の「未達」の手順）。S6 の数値を確定する。
2. 本番で 1 領域を `compare` にして一致率を取る（人の判断。候補は副作用の無い純粋判定の `lane_rules`）。
3. `celerisctl script` に canary・generation の登録と退役・停止の解除を足す（S5 提案 1）。
4. `tick_loop.rs` に `script_tick_wait` の log 行を足す（S5 提案 3）。
5. 方針改善を TS で出す手順（Rust 参照も直して golden を作り直すか、正規化の規則を ADR に書く）を決める（pilot の提案）。
6. 次に移す領域の選定は、本番で 1 領域が `ts` に上がって付記 S6 の条件の観測が始まってからにする。

## 二重実装を整理する終了条件
境界 ADR の付記 S6 の (3) に書いた。要点: 本番でその領域が `ts` のまま 30 日続き、その間の自動の戻し 0 件・説明のつかない不一致 0 件・安全違反 0 件で、golden を Rust の出力から凍結した file に移し、Rust 参照の代わりの fail-closed の既定を試験で固定し、人が承認したとき、その領域の Rust 参照実装を消す。Rust の再検証・安全条件は消さない。今はどの領域も満たさない。

## 本番への移行
- S6 で足す本番の変更は無い。S6 の unit（測定・pilot・overhead・この報告）は本番の mode・deploy・config・DB を変えていない。IEEE Access・K2・K4 の環境にも触れていない。
- 本番で上げる手順は [S5 の進捗](2026-11-02-ts-control-layer-s5.md) の「本番への移行」1.〜10. で、この unit が次を確かめた:
  - 4. の `script-deploy` は `crates/script-runtime/src/bin/script-deploy.rs`、package 名は `celeris-script` で、手順の `cargo run --release -p celeris-script --bin script-deploy` と合う。
  - canary・ts は人の token で `curl` を呼ぶ手順になっている（celerisctl は canary を持たない。既知の制約 3）。
  - pilot は同じ流れ（登録→compare→canary→rollback→再起動の復元）を一時 daemon で通した。本番の 6. の前提（compare の実 pass 200 件・純粋判定 1000 件）は pilot では synthetic に書いたので、本番では書き換えずに実際の件数を待つ。
- 本番の一致率の取り方は上の「決定一致率」。二重実装を消すのは付記 S6 の条件を満たし、人が承認した後だけ。

## 証拠（提出時の HEAD で実行）
- `python3 -I scripts/dev/edit-activate-summary.py agent-docs/progress/2026-11-02-ts-control-layer-s6/*.jsonl` → exit 1（`cos-digest warm: 14 trials < 20`）。他の 4 系列は 20 試行・全件 exit 0。
- `node control-ts/scripts/compare.mjs lane_rules|route_priority|cos_digest` → 3 本とも全件一致。
- 文書の検査: `bash scripts/dev/check-doc-links.sh` → ok、`bash scripts/dev/check-adr-numbers.sh` → ok（357 files）、`sh scripts/dev/progress-index.sh --check` → ok、`python3 scripts/dev/check-architecture-map.py` → OK（880 件）。
- `bash scripts/dev/test-affected.sh` → exit 0（試験対象の変更なし。docs と scripts/dev の python だけ）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は境界 ADR・この file・`scripts/dev/edit-activate-summary.py` の 3 つ。

## 未解決
1. `cos-digest` warm 14/20（未達）。
2. 本番の決定一致率は未計測。
3. typecheck が 0.1 秒以下の理由は未確認。
4. `task-dispatch`・`task-api` の test-build の増分、S5 を含む release の gate の秒は未計測。

## 提案
- 計画に測定 leaf m-cosd-c（6 試行）を足し、その後に summary を再実行する unit を置く。
- 付記 S5-10 の file 名の表を、実際の名前（`*-warm.jsonl`）に合わせる（付記 S6 の (1) で対応を書いた）。
