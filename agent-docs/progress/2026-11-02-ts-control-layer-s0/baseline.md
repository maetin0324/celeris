---
title: S0 baseline — edit→検査→test→反映（policy と cos の 2 scenario）
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 baseline — edit→検査→test→反映（policy と cos の 2 scenario）

結論: 1 回の編集から release build までの Rust の段は、warm で policy 約 221 秒・cos 約 155 秒かかる。
制御を TS に移せば、このうち compile・link・clippy・release build（warm で policy 約 208 秒、cos 約 137 秒）が編集ごとには要らなくなる。
残るのは TS 側の型検査・試験（未計測）、反映、release gate の全体試験（約 9〜12 分）である。

生データ（1 行 1 段、schema `celeris.edit-activate-baseline/1`）:
- `baseline/policy.jsonl` — 小さな方針変更（WorkUnit baseline-tool）
- `baseline/cos.jsonl` — CoS のまとまった制御変更の warm 3 試行・prime・cold 1 試行・test-affected の範囲（この WorkUnit）
- `baseline/cos-link.jsonl` — cos の build・test-build の warm 3 試行。link の和集合（`link_union_secs`）を足した後の script で測った
- `baseline/cos-affected.jsonl` — cos の編集を当てた `test-affected.sh` の 2 回（build+実行、実行だけ）

## 再現手順
```sh
bash scripts/dev/edit-activate-baseline.sh --dry-run --scenario policy        # 手順だけ出す
bash scripts/dev/edit-activate-baseline.sh --scenario policy --mode warm --reps 3 --out agent-docs/progress/2026-11-02-ts-control-layer-s0/baseline/policy.jsonl
bash scripts/dev/edit-activate-baseline.sh --scenario policy --mode cold --reps 1 --out agent-docs/progress/2026-11-02-ts-control-layer-s0/baseline/policy.jsonl
bash scripts/dev/edit-activate-baseline.sh --validate agent-docs/progress/2026-11-02-ts-control-layer-s0/baseline/policy.jsonl
```
- 編集（policy）: `crates/task-core/src/model_policy.rs` の `RULES`（ADR-0069 D3 の lane 規則表）の `standard/default` の前に、
  当たらない規則（`when: |_| false`）を 1 件足す。試行ごとに nonce を変え、cache が前の試行を使い回さないようにした。
  編集は trap で戻し、元の内容と cmp で一致を確かめた（全 24 行で `reverted: true`。実行後 `git status` に crates/ の差分なし）。
- compile と link の分け方: linker を時間を記録する wrapper（中身は host の linker `/usr/bin/gcc` + wild）に差し替え、
  段の間の link の壁時計の和を `link_secs` とした。`compile_secs = secs - link_secs`（cargo の待ち・nextest の起動を含む）。
- warm は run の `CARGO_TARGET_DIR`。編集なしの 1 周（kind=prime）で温めてから計った。cold は `$TMPDIR` の使い捨て target
  （/local の空き 127.9GB ≥ 60GB なので実行し、終了後に消した）。

## 環境（policy の行。cos は下の「cos の環境」）
| 項目 | 値 |
|---|---|
| HEAD の件名 | `phase 1: question で終わった run の usage を runs と worker_finished に記録する` |
| 基準 | `${CELERIS_WU_BASE:-$(git merge-base HEAD main)}`（件名 `WU tracked scope base`） |
| toolchain | rustc 1.98.1（2026-09-01）、cargo 1.98.1（2026-08-05）、stable、`-Z` なし |
| cargo 設定 | `CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG=line-tables-only`、release は既定 profile（LTO なし） |
| CPU | AMD Ryzen Threadripper PRO 3945WX 12-Cores、論理 24 |
| sccache | 実行ファイルあり、server に届かない（wrapper が素の rustc に戻る）。hit 数は記録できず null |
| 負荷 | 段の開始時の loadavg(1 分) 1.1〜13.4。大半はこの計測自身の cargo。段の終了時の他の cargo/rustc 数 0〜1 |
| 空き容量 | target・/local とも 127.9GB |

## 段別の秒（policy）
warm は 3 試行の中央値（3 値）、cold・prime は 1 試行。

| 段 | コマンド | warm 中央値 [3 値] | warm link | prime（編集なし初回） | cold | cold link（回数） |
|---|---|---|---|---|---|---|
| check | `cargo check -p task-core` | 16.8 [16.8, 16.8, 16.8] | 0 | 24.4 | 22.9 | 0.8（18） |
| build | `cargo build -p celeris` | 52.6 [55.3, 52.6, 52.5] | 1.36（1） | 63.4 | 61.9 | 3.6（37） |
| clippy | `cargo clippy -p task-core -- -D warnings` | 18.6 [18.6, 18.6, 18.6] | 0 | 18.8 | 18.7 | 0 |
| test-build | `cargo nextest run -p task-core --no-run` | 32.3 [32.8, 32.1, 32.3] | 0.37（1） | 36.6 | 35.5 | 0.39（1） |
| test-run | `cargo nextest run -p task-core` | 13.0 [13.0, 12.6, 13.1] | 0 | 13.4 | 13.2 | 0 |
| release-build | `cargo build --release -p celeris` | 87.7 [88.4, 87.0, 87.7] | 0.46（1） | 132.7 | 131.3 | 3.3（47） |
| 合計 | | 約 221 | 約 2.2 | 289 | 283.5 | 8.1 |

読み取り:
- task-core の 1 規則の変更で、warm でも celeris の debug build 52.6 秒・release build 87.7 秒かかる。link は 1 回 0.4〜1.4 秒で、
  時間の大半は task-core の逆依存 crate の再 compile（CARGO_INCREMENTAL=0）。
- cold と warm の差は依存の再 compile 分（build +9 秒、release +44 秒）で、sccache が効いていないため cold と prime はほぼ同じ。
- test は対象 crate（task-core）だけ。`test-affected.sh` は逆依存 crate も試験するので、実際の Phase 完了の試験はこれより長い（未計測）。

## 反映（activate）の推定 — 実測ではない
本番では promote できないので計っていない。`/local/celeris/state/releases/*/gate.json`（直近 5 件、読むだけ）の段の秒を
`kind: estimate` の行として記録した（warm・cold の 2 回の実行で同じ 5 件を 2 度書いている）。

| release gate の段 | 直近 5 件の秒 |
|---|---|
| cargo-test（全 workspace nextest） | 547.6, 575.6, 576.2, 625.7, 693.1 |
| cargo-clippy | 9.5, 41.4, 41.6, 41.7, 51.8 |
| cargo-build（celeris・celerisctl・credentiald release） | 1.1, 1.1, 1.2, 2.4, 92.6 |
| cargo-fmt-check | 7.3〜8.5 |
| web（install・typecheck・test・release の和） | 約 16〜34 |

推定: release gate の 1 回は約 11〜13 分で、cargo-test が 85% 前後を占める。cargo-build が 1〜2 秒の回は、共有 target が
同じ tree から作られていて再 build が要らなかった回と読める（未確認）。promote（daemon の差し替え・drain）の所要時間は
release 記録に無く、未計測。

## 未解決
- sccache の server が起きていない状態で計った。cache が効く状態の cold は未計測。cos で私設の server
  （`SCCACHE_DIR` を `$TMPDIR` の下）を起こそうとしたが、socket の path が長すぎて起動しなかった（`path must be shorter than SUN_LEN`）。
- policy の「同時に走る cargo/rustc の数」は段の終了時に数えた値。cos は段の途中も数えた（`env.during`、全段で 0）。
- test-affected の実行は fail-fast で止まった下限（`ops_r2_socket` の 1 件の失敗。原因は未確認）。policy の test-affected は未計測。

## cos — CoS のまとまった制御変更
### 再現手順
```sh
D=agent-docs/progress/2026-11-02-ts-control-layer-s0/baseline
bash scripts/dev/edit-activate-baseline.sh --dry-run --scenario cos
bash scripts/dev/edit-activate-baseline.sh --scenario cos --mode warm --reps 3 --out $D/cos.jsonl
bash scripts/dev/edit-activate-baseline.sh --scenario cos --mode cold --reps 1 --out $D/cos.jsonl
BASELINE_STEPS="build test-build" bash scripts/dev/edit-activate-baseline.sh --scenario cos --mode warm --reps 3 --out $D/cos-link.jsonl
BASELINE_STEPS="affected-test affected-rerun" bash scripts/dev/edit-activate-baseline.sh --scenario cos --mode warm --reps 1 --out $D/cos-affected.jsonl
bash scripts/dev/edit-activate-baseline.sh --validate $D/cos.jsonl   # cos-link・cos-affected も同じ
```
- 編集（cos）: `crates/task-api/src/cos/basis/job_wait_extend.rs` の `validate`（CoS 定型判断 `job_wait_extend` の API 検査）の
  D3 の上限判定の前に、33 行の制御を足す（job の状態を `match` で 3 種に数え、`if/else if` の 4 分岐で理由を決め、
  `BASIS_JOB_NOT_QUEUED` で拒否する）。直前の D2 の検査で全 job が Queued と決まっているので、実際には当たらない。
  試行ごとに nonce を変えた。編集は trap で戻し、全 step 行で `reverted: true`。実行後の `git status` に crates/ の差分なし。
- cold は /local の空き 124GB ≥ 60GB なので実行した（`$TMPDIR` の使い捨て target、終了後に消した）。

### test-affected.sh が選ぶ範囲（編集を当てた状態、基準 HEAD = この編集だけ）
`CELERIS_WU_BASE=HEAD bash scripts/dev/test-affected.sh --list` の出力（`kind: affected` の行、warm・cold で同じ）:
`celeris`, `celeris-campaign`, `celerisctl`, `llm-proxy`, `task-api`, `task-dispatch`, `task-worker`、
`web: typecheck+unit (理由: web の型の元になる crate の変更: task-api)`。
job_wait_extend の 1 file の変更で、7 crate の試験（nextest で 4903 件）と web の型検査・unit 試験が対象になる。

### 段別の秒（cos）
warm は 3 試行の中央値 [最小, 中央値, 最大]。prime（編集なしの初回）・cold は 1 試行。test-build の link 欄は `cos-link.jsonl` の値
（`cos.jsonl` の test-build 行は和集合の欄を足す前の script で測ったので、`compile_secs` が負になっている。読まない）。

| 段 | コマンド | warm 中央値 [最小, 中央値, 最大] | warm link（壁時計／和／回数） | prime | cold | cold link（和／回数） |
|---|---|---|---|---|---|---|
| check | `cargo check -p task-api` | 9.7 [9.3, 9.7, 10.2] | 0 | 36.9 | 37.6 | 1.6（34） |
| build | `cargo build -p celeris` | 25.9 [25.4, 25.9, 26.9] | 1.36／1.36／1 | 62.6 | 61.8 | 2.7（22） |
| clippy | `cargo clippy -p task-api -- -D warnings` | 10.6 [10.1, 10.6, 10.6] | 0 | 34.0 | 34.0 | 0 |
| test-build | `cargo nextest run -p task-api --no-run` | 46.4 [46.1, 46.4, 49.9] | 21.2 [20.3, 21.2, 22.1]／238〜311／103 | 74.6 | 76.8 | 262（105） |
| test-run | `cargo nextest run -p task-api` | 18.2 [18.1, 18.2, 18.6] | 0 | 35.9 | 36.1 | 0.6（7） |
| release-build | `cargo build --release -p celeris` | 44.6 [43.6, 44.6, 45.7] | 0.47／0.47／1 | 131.2 | 131.0 | 3.3（47） |
| 合計 | | 約 155 | 約 23 | 375 | 377 | |

`test-affected.sh`（編集を当てた warm、`cos-affected.jsonl`）:

| 段 | 秒 | exit | link（壁時計／和／回数） |
|---|---|---|---|
| affected-test（7 crate の試験 build + 実行 + doc test + web） | 291.3 | 100 | 58.2／493／372 |
| affected-rerun（同じもの。build 済み） | 154.4 | 100 | 0.4／0.4／8 |
| prime（編集なし。check-health だけ） | 16.1 | 0 | 0 |

- exit 100 は `celeris-campaign::ops_r2_socket socket_flag_rejects_non_sockets` の 1 件の失敗。nextest の fail-fast で 4903 件中
  1317 件目で止まったので、実行の秒は下限である。この失敗が編集と関係ないか（sandbox の中だけで落ちるか）は未確認。
- 差（291 − 154 ≈ 137 秒）が 7 crate の test binary の再 build（うち link の壁時計 58 秒）に当たる。

### cos の環境
| 項目 | 値 |
|---|---|
| HEAD の件名 | `phase 1: edit→activate の計測 script と小さな方針変更（policy）の baseline`（policy と同じ tree の上） |
| toolchain・CPU・cargo 設定 | policy と同じ（rustc 1.98.1、Threadripper PRO 3945WX 論理 24、`CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG=line-tables-only`） |
| sccache | server に届かない（全行 `reachable: false`）。素の rustc で計った |
| 負荷 | 段の開始時の loadavg(1 分) 1.4〜39.3、段の途中の最大 46.7。どれもこの計測自身の cargo による |
| 同時 cargo | 段の途中 5 秒ごとに、この計測と違う `CARGO_TARGET_DIR` の cargo/rustc を数えた（`env.during`）。全段で最大 0 |
| 空き容量 | /local 119〜128GB |

## 2 scenario の段別比較（warm 中央値／cold、秒）
| 段 | policy（task-core の規則表） | cos（task-api の CoS 定型判断） | TS 化で編集ごとに |
|---|---|---|---|
| check | 16.8／22.9 | 9.7／37.6 | 消せる（TS の型検査に置き換わる。未計測） |
| compile（build の link 以外） | 51.2／58.3 | 24.5／59.0 | 消せる |
| link（build・release の壁時計） | 1.4＋0.5／3.6＋3.3 | 1.4＋0.5／2.7＋3.3 | 消せる |
| clippy | 18.6／18.7 | 10.6／34.0 | 消せる（TS の lint に置き換わる。未計測） |
| test build（link の壁時計） | 32.3（0.4）／35.5 | 46.4（21.2）／76.8 | 消せる |
| test 実行（対象 crate） | 13.0／13.2 | 18.2／36.1 | 消せない（TS の試験に置き換わる。移した判定の試験だけになる） |
| release build | 87.7／131.3 | 44.6／131.0 | 消せる（host を変えない限り） |
| test-affected（逆依存込み） | 未計測 | 291（下限）、うち再 build 137 | 無関係な crate の試験（celeris-campaign・llm-proxy 等）と再 build は消せる |
| 反映（activate） | 推定のみ（下の節） | 同じ | 消せない（generation の切り替えに置き換わる。未計測） |
| release gate の全体試験 | 548〜693 | 同じ | 消せない（gate の規則を変えない限り） |

読み取り:
- cold と warm の差は依存 crate の再 compile で、cos で約 222 秒（377 − 155）、policy で約 62 秒。cold と prime がほぼ同じなのは
  sccache が効いていないため（incremental も `CARGO_INCREMENTAL=0` で切ってある。渡された値のまま測る規則なので変えていない）。
  sccache が効く状態の cold は計れなかった（下の「未解決」）。
- warm は policy の方が長い。task-core の逆依存の方が多く、celeris の再 compile の範囲が広いため（build 52.6 秒 対 25.9 秒）。
- link は build・release では 1 回 0.5〜1.4 秒で小さい。cos の test-build は 103 個の test binary を並列に link し、壁時計 46 秒の
  うち 21 秒が link。test-affected では 372 回の link で壁時計 58 秒。test の数が多い crate ほど link が効く。
- ばらつきは小さい。warm の最小と最大の差は各段で 1.5 秒以内（cos の test-build だけ 3.8 秒）。

### TS 化で消せる部分・消せない部分（見積り）
- 消せる（TS の方針・CoS 判定だけを変える編集なら）: Rust の check・compile・link・clippy・test build・release build。
  warm で policy 約 208 秒（221 − 13）、cos 約 137 秒（155 − 18）。test-affected の逆依存 crate の試験（cos で下限 291 秒）も、
  判定を TS に移せば、判定の試験と host API の契約試験だけに絞れる。
- 消せない: TS の型検査・lint・試験（未計測。S1 の PoC で測る）、反映（generation の切り替え）、release gate の全体試験
  （約 9〜12 分。gate の規則を変えない限り残る）、Host API（Rust 側）を変える編集の Rust の段全部。
- 見積りの前提: TS は daemon が V8/deno_core で読み込み、Rust の再 build なしに入れ替えられること（ADR で決める。未確認）。
