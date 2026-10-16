---
title: "MLSys 本測定: 代表 case の定義（dev/final 分離・snapshot 固定）"
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
unit: cases
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# cases（WU cases）

## 状態の区分

| 区分 | 内容 |
|---|---|
| 実装済み | `crates/campaign/src/cases.rs`（case ローダ・検証・本測定選択）、`lib.rs` に mod 1 行、`tests/cases_load.rs`、case 5 件（real 4 + control 1） |
| mock 検証済み | ローダの拒否試験（hash 不一致・split 混在・control の混入・final 値の dev 参照・必須欄欠落）。一時 directory の写しを壊して確かめる |
| 実経路検証済み | 外部原票の sha256（K3 `main-43223.tgz`、K4 `main-v3-n2-43706.tgz`、bundle）と初期 commit の tree / `git archive` hash を、ローカルの実ファイルから計算して固定した（計算は run 中に実施） |
| 未実施 | 受入 command の実行（pre-fix で落ち fix で通る確認）、Sirius での実験、submodule の固定、grader 本体（WU grader）、A/B/C の分離配送（WU isolation） |

## 配置（acceptance 0・2）
- `crates/campaign/cases/{dev,final}/<id>/{case.json,materials/,kb/}` — 試行に渡してよい公開部。
- `crates/campaign/grader/{dev,final}/<id>/private.json` — 隠し受入 command・既知欠陥・誤指摘 trap・参照値。`cases/` の外。
  final の private は `grader/final/` にだけあり、dev 側の木（`cases/dev`・`grader/dev`）から参照しない。ローダがこれを検査する
  （final の case id・欠陥 id・private hash・初期 commit が dev 側 file に現れたら拒否）。
- case.json の `private_ref` が private の hash を固定する。

| case | split | kind | 内容 |
|---|---|---|---|
| k1-bf-fsync-campaign | dev | real | BenchFS `4115dbd8`（tree `5bda9947`）、fsync 経路、4 セル実験（参照 job 42588–42591 は private） |
| k3-locusta-ab-analysis | dev | real | 原票 `main-43223.tgz`（sha256 `bdb155d4…`）からの再分析 |
| k2-bf-xattr-restart-campaign | final | real | BenchFS `086c4618`（tree `56d2f021`）、再起動後 size 復元 |
| k4-cmp4-validity-analysis | final | real | 原票 `main-v3-n2-43706.tgz`（sha256 `128057cf…`）の有効性分析 |
| k0-fixture-control | dev | control | 既存 fixture。本測定・最終合格に使えない（ローダが拒否） |

dev と final は初期 commit・archive・原票 hash を共有しない（試験 `real_cases_pin_distinct_inputs_between_splits`）。
manifest 型は `Split`・`InputRef`・`Budget`・`SnapshotRef` を再利用した。

## 検証
- `cargo test -p celeris-campaign --test cases_load`: 11 passed, 1 ignored（`update_pins` は hash 作り直し用）。
- 固定 hash の作り直し: `CAMPAIGN_UPDATE_CASES=1 cargo test -p celeris-campaign --test cases_load -- --ignored`。
- workspace の test/clippy/fmt の結果は下の「全体検査」。

## 未解決事項（inventory §3 から引き継ぎ。人の判断が要る）
- R1 node-h 上限の承認（K1/K2 の実 job）。R2 submodule 3 件の固定元（`lib/pluvio@9ecd51ee` ほか、`not our ref` の記録あり）。
  case.json の `initial.fetch` は submodule を「未固定」と明記している。
- R3 K2 の隠し試験の 086c4618 への移植は未確認（private `portability_unverified`）。
- R5 K3 の統計量は暫定で「反復ごとの比の平均」に固定した（private に `provisional` と明記）。人の確認待ち。
- K1/K2 の取得手順は「campaign asset store から tar を取得」。tar は bundle から `git archive` で作る手順を private の
  provenance に書いた。asset store の実体（bundle・tgz を置く場所と、試行へ tar だけを渡す配送）は WU isolation の担当。
- Pluvio 単体の実資産は見つからなかった（inventory）。Pluvio は BenchFS の submodule としてのみ含まれる。
- KB snapshot は一般事実だけの手書き（解答・過去結果を除く）。KB 本物のページを snapshot に使う場合は人の確認が要る。

## 提案
- grader（WU grader）は private の `expected` と `known_defects`／`false_claim_traps` を読み、成果物から独立に再計算する。
  mock の Passed を最終合格にしない規則は grader 側で担保する（case 側は `kind=control` でローダが選択を拒否）。

## 全体検査
- `bash scripts/dev/test-parallel.sh`: passed 5394, failed 0, ignored 14、exit 0。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p celeris-campaign --all-targets -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- 参考: `clippy --workspace --all-targets` は `celeris` の lib test で失敗する（この unit の変更範囲外）。
