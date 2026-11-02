---
title: S0 baseline — edit→検査→test→反映（小さな方針変更 policy）
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
status: done
updated: 2026-11-02
---
# S0 baseline — edit→検査→test→反映（小さな方針変更 policy）

生データ: `baseline/policy.jsonl`（1 行 1 段、schema `celeris.edit-activate-baseline/1`）。
CoS のまとまった制御変更（scenario `cos`）は WorkUnit baseline-cos が同じ script で測る。

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

## 環境（全行で同じ）
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
- sccache の server が起きていない状態で計った。cache が効く状態の cold は未計測。
- 「同時に走る cargo/rustc の数」は段の終了時に数えた値で、段の途中の値ではない。
