---
title: S3 — TS だけの変更の高速検証経路（cargo 再 build・relink なし）
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 — TS だけの変更の高速検証経路

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- TS だけの編集を `bash scripts/dev/ts-verify.sh` で cargo を起動せずに検証できるようにした。`policy-ts`（`lane_rules.ts` の規則 1 件）は warm p50 5.031 秒・p95 7.406 秒、cold p95 11.525 秒で、目標（30 秒・60 秒・120 秒）を満たした。全 40 試行で cargo の起動 0 回、`link_count` 0。
- warm p50 は S0 の baseline（policy warm 中央値 221.0 秒）の 2.3%。ただし検証の中身と cold の条件が違う（「時間の内訳」の注意）。
- 合否は Rust の runner（`script-host-runner`）と `scripts/dev/` が決める。`control-ts/` の検証の道具・fixture・設定を変えると `ts-only` にならず、cargo の試験へ回る。
- release gate は変えていない。全体試験は減っていない。
- 本番の操作は無い。runner は検証専用で daemon の経路に入らず、本番への移行はない。
- 人に残る判断は無い。後続への申し送りは「未解決」と「提案」に書いた（cos の TS scenario は S4 の後に S6 で測る）。

設計は [ADR 2026-11-02-ts-fast-verify](../adr/2026-11-02-ts-fast-verify.md)（本文 (1)〜(7)、付記 wire・close）。境界 ADR への付記は [付記 S3](../adr/2026-11-02-ts-control-layer-boundary.md)。

## unit と commit
記録は `agent-docs/progress/2026-11-02-ts-control-layer-s3/<unit>.md`。

| unit | commit の件名 | 内容 |
|---|---|---|
| [adr-fast](2026-11-02-ts-control-layer-s3/adr-fast.md) | `phase s3: adr-fast: TS だけの変更の高速検証経路の設計 ADR` | 3 分類・段・runner・cache key・grader の保護・計測の定義 |
| [runner](2026-11-02-ts-control-layer-s3/runner.md) | `phase s3: runner: 検証専用の本物の host script-host-runner と 4 場合の試験` | `script-host-runner` の `run`・`activate`・`domains` |
| [verify-sh](2026-11-02-ts-control-layer-s3/verify-sh.md) | `phase s3: verify-sh: ts-verify.sh の分類と script-runner-cache.sh の cache key` | 分類（`--list`）・10 段・cache key |
| repair-build-1 | なし | 統合後の検査の repair。範囲外の `crates/task-dispatch` の試験が原因で plan_issue を返した（「未解決」3） |
| [wire](2026-11-02-ts-control-layer-s3/wire.md) | `phase s3: wire: ts-verify を本物の runner で通し、意図的破壊の試験と test-affected への接続` | 本物の runner への接続・`test-ts-verify.sh`・`test-affected.sh` の委譲 |
| [bench](2026-11-02-ts-control-layer-s3/bench.md) | `phase s3: bench: edit-activate-baseline.sh に TS 経路の scenario policy-ts と目標判定を足す` | scenario `policy-ts`・`--check-targets` |
| [measure-warm](2026-11-02-ts-control-layer-s3/measure-warm.md) | `phase s3: measure-warm: policy-ts の warm 20 試行を測り目標と照合する` | [policy-ts-warm.jsonl](2026-11-02-ts-control-layer-s3/policy-ts-warm.jsonl) |
| [measure-cold](2026-11-02-ts-control-layer-s3/measure-cold.md) | `phase s3: measure-cold: policy-ts の cold 20 試行を測り目標と照合する` | [policy-ts-cold.jsonl](2026-11-02-ts-control-layer-s3/policy-ts-cold.jsonl) |
| close | `phase s3: close: S3 の進捗まとめ・ADR と実装の突き合わせ・地図の確認` | この記録、ADR の付記 close、境界 ADR の付記 S3、地図の行 |

## 受け入れ条件ごとの証拠
結果は提出時の HEAD で実行した（基準は `$CELERIS_WU_BASE`）。「写し」は `$TMPDIR` に作業ツリーを写して commit した基準で、1 回だけ確かめたもの。試験には入っていない。

### 0. 代表的な TS 変更の検証が Rust の再 compile・relink なしで通る
- 分類（写しで `bash scripts/dev/ts-verify.sh --list` と `bash scripts/dev/test-affected.sh --list`。各 file に空行を 1 行足した）:
  | 変えた file | class | test-affected の対象 |
  |---|---|---|
  | `control-ts/src/domains/lane_rules.ts` | `ts-only`（10 段、cargo 0 回） | `control-ts: ts-verify` の 1 行だけ |
  | `control-ts/src/host.d.ts`・`control-ts/scripts/compare.mjs` | `host-api` | `celeris`・`celeris-script`・`celerisctl`、control-ts の型検査と試験 |
  | `control-ts/fixtures/lane_rules/all-features.jsonl` | `host-api` | `task-core` と逆依存の 13 crate に `celeris-script` を足した 14 crate、web の型検査と単体、control-ts の型検査と試験 |
  | `crates/task-core/src/lib.rs` | `rust` | `task-core` と逆依存の計 13 crate、web の型検査と単体、`test-ts-verify` |
  | `crates/task-api/src/lib.rs` | `rust` | `task-api` と逆依存の計 7 crate、web の型検査と単体 |
  | `scripts/dev/ts-verify.sh` | `rust` | `celeris`・`celeris-script`・`celerisctl`、`test-ts-verify` |
  | `agent-docs/README.md` | `none` | なし |
- `bash scripts/dev/tests/ts-verify-classify.sh` → exit 0、45 passed。
- TS だけで cargo 0 回: `bash scripts/dev/test-ts-verify.sh` の場合 A（偽の cargo は exit 97）→ 記録の `cargo_invocations` 0、`link_count` 0、10 段すべて exit 0。場合 I（`test-affected.sh --list` が ts-verify に委ね、偽の cargo を呼ばない）。全体は exit 0、12 passed。
- 計測の全 40 試行でも cargo 0 回・`link_count` 0（下の条件 2）。

### 1. 意図的な破壊を高速経路が検出する
`bash scripts/dev/test-ts-verify.sh` → exit 0、12 passed。ADR (5) の 8 場合。
| 破壊 | 結果 | 試験か |
|---|---|---|
| 規則を 1 件壊す（`frontier/costly-and-unverifiable` の lane を standard に） | ts-verify exit 1（段 unit、cargo 0 回）。同じ bundle を runner に直に渡すと exit 1、mismatch 1,458 件 | 試験 B |
| `compare.mjs` を常に成功させる | `host-api` | 試験 C |
| fixture を壊れた出力に合わせる | `host-api`、cache の golden と `cmp` で違う | 試験 D。写しでは ts-verify は exit 0（従来の経路へ）で、`cargo nextest run -p task-core --test script_fixtures` が exit 100 |
| `package.json` の `typecheck`・`test` を `true` に | `host-api` | 試験 E |
| `bundle.config.json` から領域を消す | `host-api`、runner が exit 3 | 試験 F |
| `@ts-nocheck`・`.only(` の追加 | classify で exit 1 | 試験 G |
| 規則を壊し、試験の中身を無害に書き換える | `ts-only`、段 unit で exit 1（`compare.test.mjs` が落ちる） | 写し（wire の未解決を解いた） |
| 規則を壊し、試験 file を消す | `host-api`、ts-verify は exit 0（従来の経路へ）。従来の経路で動く `compare.test.mjs`・`compare.mjs` はどちらも exit 1、runner も mismatch 1,458 件で exit 1 | 写し |
| `src/domains/index.ts` から `route_priority` を消す | `ts-only`、段 unit で exit 1。runner は `exception` 2,026 件で exit 1 | 写し |

grader の保護: 合否の表は `script-host-runner.rs`（Rust）と `scripts/dev/` が持つ。`crates/script-runtime/tests/script_host_runner.rs` が一致・不一致・例外・宣言外と bundle の拒否を固定する（`cargo nextest run -p celeris-script --test script_host_runner` の結果は下の検査）。`scripts/dev/` 自身の書き換えは `rust` になり、`test-ts-verify.sh` が走る（`--list` で確かめた）。

### cache key と golden の無効化
`bash scripts/dev/tests/script-runner-cache-key.sh` → exit 0、30 passed。host の source（`script-runtime`・`task-core`・ルート `Cargo.toml`・未追跡 file・未 commit の編集）などで key が変わること、golden が meta と違えば `path` が exit 1、Rust の golden が fixture と違えば `ensure` が exit 1、golden の元の crate の閉包が一覧と違えば `ensure` が exit 1 になることを固定した。

### 2. S0 の baseline との時間の内訳
`bash scripts/dev/edit-activate-baseline.sh --validate <file>`・`--check-targets <file>` を warm・cold の両方で再実行した → どれも exit 0、`problems` 0。warm は `trials` 20・`p50` 5.031・`p95` 7.406、cold は `trials` 20・`p95` 11.525。

単位は秒。policy-ts は 20 試行の p50、baseline は [S0 の baseline](2026-11-02-ts-control-layer-s0/baseline.md) の policy（warm は 3 試行の中央値、cold は 1 試行）。段の対応づけはこの task の判断で、同じ処理ではない。

| policy-ts の段 | warm p50 | cold p50 | baseline の段 | warm | cold |
|---|---|---|---|---|---|
| classify・runner-cache・install | 0.276 | 0.429 | なし | なし | なし |
| typecheck | 0.082 | 0.087 | check | 16.8 | 22.9 |
| 対応なし（TS の経路に lint の段は無い） | なし | なし | clippy | 18.6 | 18.7 |
| bundle | 0.072 | 0.080 | build（celeris debug） | 52.6 | 61.9 |
| unit + golden + safety + integration | 4.227 | 4.688 | test-build + test-run | 45.3 | 48.7 |
| activate | 0.072 | 0.076 | release-build | 87.7 | 131.3 |
| 全体（p50） | 5.031 | 5.570 | 合計 | 221.0 | 283.5 |

| 指標 | policy-ts | 目標 | baseline | 比 |
|---|---|---|---|---|
| warm p50 | 5.031 | 30 以下 | policy warm 221.0 | 2.3% |
| warm p95 | 7.406 | 60 以下 | なし（3 試行では出せない） | なし |
| cold p95 | 11.525 | 120 以下 | policy cold 283.5・cos cold 377.2 | 4.1%・3.1% |
| link の回数 | 全試行 0 | 0 | policy warm 試行あたり 3 回・cold 103 回 | なし |

比べるときの注意:
- policy-ts の秒に Rust の全試験・release gate は入らない。baseline の試験とは範囲が違う。
- cold の条件が違う。policy-ts は `control-ts/node_modules` だけを消し、runner の cache は当たる。baseline は cargo target を作り直す。runner の作り直しは `ensure` で約 60 秒で、この 40 試行に入っていない。
- 反映はどちらも本番で測っていない。policy-ts の activate は一時の state dir への配備と load で、baseline は release build まで。
- 試行数は policy-ts 20 に対し baseline は warm 3・cold 1。時期と host の負荷も違う。遅い試行の原因は未確認（measure-warm・measure-cold の記録）。

### 3. full regression を削らず、API・Rust の変更で full が走る
- Host API・Rust の変更で cargo の試験が残る: 条件 0 の表。`bash scripts/dev/test-ts-verify.sh` の場合 H（`host.d.ts`・schema で `celeris-script` を選ぶ）、`bash scripts/dev/tests/test-affected-list.sh` → exit 0（`test-affected-list: ok`）。wire の記録で `celeris-script` と逆依存の nextest 915 passed。
- release gate を変えていない: `git diff --stat "$(git merge-base HEAD main)" HEAD -- scripts/selfdeploy/ .config/nextest.toml` → 差分なし。`scripts/selfdeploy/release.sh` は ts-verify を呼ばない。全体試験は release gate だけで走る（[全体試験は release だけ](../adr/2026-10-20-full-tests-at-release-only.md)）。
- nightly の gate はこの repo に無い（ADR 付記 close-7）。

### WorkUnit close の条件
- 0. この記録に受け入れ条件ごとの証拠を書いた。
- 1. ADR との違いを [付記 close-1〜9](../adr/2026-11-02-ts-fast-verify.md) と境界 ADR の付記 S3 に書いた。
- 地図: `python3 scripts/dev/check-architecture-map.py` が全 path の実在を確かめた（結果は下）。ts-verify の行に計測の script とその試験、ADR (7)・付記を足した。
- 2. 検査（提出時の HEAD。close unit の差分は文書だけ）:
  - `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0。
  - `python3 scripts/dev/check-architecture-map.py` → exit 0、849 件のパスを確認した。
  - `bash scripts/dev/ts-verify.sh --list` → exit 0、`class: none`（この unit の変更は文書だけ）。
  - `bash scripts/dev/test-ts-verify.sh` → exit 0、12 passed、0 failed（場合 A は 5.148 秒）。
  - `cargo nextest run -p celeris-script --test script_host_runner` → exit 0、5 passed。
  - `bash scripts/dev/test-affected.sh` → exit 0、`試験対象の変更なし`（文書だけの変更。文書検査は実行された）。
  - `cargo clippy --workspace -- -D warnings` → exit 0。
  - `cargo fmt --all -- --check` → exit 0。
  - 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → この記録、2 つの ADR、`docs/architecture-map.md` の 4 file だけ。

## 未解決
1. **検証の道具を変えた変更で、作り直した bundle を runner に通さない場合がある**（ADR 付記 close-6）。`control-ts/scripts/**`（`build-bundle.mjs` など）・`host.d.ts`・`package.json` の変更では key が変わらず、`test-ts-verify.sh` が走らない。`test-affected.sh` は control-ts の試験と `celeris-script` の cargo の試験を走らせるが、`crates/celeris` の本物の host の試験は古い dist で SKIP することがある（S2-12 (4)）。
2. **S2-7 の capability の拒否が daemon に無い**（境界 ADR の付記 S3）。今は `Grant` で実効の権限を絞っており、`lane_rules`・`route_priority` は op を呼べない。
3. `integrate-build` の検査で repair-build-1 が走り、範囲外の `crates/task-dispatch` の試験（`integration_ready_race.rs:293` の `expired_lease_during_stage_integration_is_extended_not_reclaimed`）を原因として plan_issue を返した。負荷下の flaky と推定しているが、再現は未確認。S3 では直していない。
4. runner の値の検査の上限は `task-dispatch::script_policy` の写しで、一致を固定する試験が無い（手で比べて同じ。ADR 付記 close-9）。
5. runner の cache が外れた状態の cold は測っていない。60.6 秒に cold の最大 14.352 秒を足すと約 75 秒で 120 秒以下だが、足し算の推定である。
6. cos の TS scenario は測っていない。S4 の後に S6 で測る。条件 2 の cos の比は編集の中身が違う参考値である。
7. 実際の activate（DB の generation の行と `compare` への CAS）を含む edit→activate は測っていない（境界 ADR の付記 S3）。S6 で測る。

## 提案
1. **S6**: cos の TS scenario を、S4 の後に `edit-activate-baseline.sh` へ足して warm・cold を各 20 試行測る。本物の activate を含めた秒も同じ場で測る。
2. **S4 の前に決める**: S2-7 の拒否を daemon に実装するか、S2-7 からこの文を消すか。op を使う最初の領域（digest）で許可表が空でなくなる前に決める。manifest に領域ごとの宣言が無いので、実装するなら manifest の形の変更を伴う。
3. **別 task**: `test-affected.sh` で `control-ts/` の `src/` 以外の変更と `host.d.ts` の変更にも `test-ts-verify.sh` を走らせる（未解決 1）。
4. **別 task**: `crates/celeris/tests` に、runner の値の検査と `classify_lane_rules`・`classify_route_priority` の一致を固定する試験を置く（未解決 4）。
5. **別 task**: `integration_ready_race.rs:293` の flaky の推定を、時計の差し替えで確かめて直す（未解決 3）。
