---
title: "健全性v2 R4 admit-tidy: dispatch_one の記録の無い Ok(false) を無くし、module map に判断の module の行を足す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 admit-tidy

完了日: 2026-11-14

## 概要

最終 review の advisory 2 件を直した。

- `crates/task-dispatch/src/dispatcher/dispatch_run.rs` の `dispatch_one` で、admit の契約が守られていれば通らない 7 か所の
  分岐（log も待ちの記録も残さず `Ok(false)` を返していた）を、新しい free function `admission_contract_broken` の呼び出しに替えた。
  この関数は task id と欠けた fact か値の名を受け、`tracing::error!` で記録して `Ok(false)` を返す。task は ready のまま残り、
  次の tick で admit からやり直す。
- `crates/task-dispatch/src/dispatcher.rs` の module map（ADR-0082）に `dispatch_admission` と `review_decision` の行を足した
  （形は `run_settlement` の行と同じ。層は「判断」）。
- `dispatch_admission` の型（`AdmissionInput`・`AdmissionProposal`・`AdmissionStop`・`AdmissionLaunch`）と
  ADR 2026-11-14-dispatch-admission-order は変えていない。正当な待ち（`AdmissionStop` の後・lease の取り損ね）の `Ok(false)` も変えていない。

## 実装前 brief

1. 根本原因: admit-launch で `dispatch_one` を admit の fact の要求に組み替えたとき、admit が「前提の値を集めた後でだけ求める」
   fact（ClusterState・WorkUnit・LaneAssist・CredentialGate・VariantPin）と、起動の直前の dir・route・launch・pin の取り出しを
   let-else で書き、else を `return Ok(false)` にした。契約が崩れると task が理由の記録なしに ready のまま残る。
2. 責務 owner: `dispatch_one`（fact の値を持ち、admit の提案を適用する側。ADR 2026-11-14 D2・D4）。
3. 既存の再利用先: 既存の `tracing` の記録。試験は `dispatch_admission::` の試験（admit の順序と止め方）と `cex_` の反例試験。
4. 選択肢:
   - 案 A（採用。計画で決めた形）: `dispatch_run.rs` に free function `admission_contract_broken` を 1 つ置き、7 か所から呼ぶ。
   - 案 B: `dispatch_admission` の結果の型に値を持たせ、let-else を無くす。ADR 2026-11-14 D2（値は dispatch_one が持つ）に反するので採らない。
   - 案 C: `DispatchError` を返す。`dispatch_one` の `Err` は `dispatcher.rs` の tick の loop と `work_units.rs` の並列の loop で
     `?` として伝わり、その tick の後ろの task の dispatch を打ち切るので、1 つの task の契約違反がほかの task を止める。採らない。
5. 新しい状態・分岐・fallback・port: 無し。分岐の数は変わらず、7 か所の else の中身が記録に替わるだけ。
   `Dispatcher` に method は足していない（free function）。
6. 撤去対象: 7 か所の記録の無い `return Ok(false)`。header の古い説明 3 行（下の「repo health の行数」）。
7. 不変条件: admit の契約（fact を求める前と起動の前に、前提の値を dispatch_one が集めている）が守られている間、7 か所は通らず、
   起動・止め方の振る舞いは変わらない。確かめる試験は既存の `dispatch_admission::`・`cex_`・`structure_guard` の試験（54 本）。

## 変更

- `dispatch_run.rs`:
  - `admission_contract_broken(task_id: TaskId, missing: &str) -> Result<bool, DispatchError>`（新、free function）。
  - 7 か所の名（`missing`）: `ClusterState: cluster`・`WorkUnit: work unit or dir`・`LaneAssist: lane decision`・
    `CredentialGate: launch`・`VariantPin: launch`・`launch: task dir`・`launch: route, launch or variant pin`。
  - `WorkerStarted` の `role` と planner run の `extras.profile` の `if c { Some(x) } else { None }` を、同じ関数で既に使っている
    `bool::then_some`・`bool::then` の形にした（振る舞いは同じ。理由は下の「repo health の行数」）。
- `dispatcher.rs`:
  - module map に `dispatch_admission` と `review_decision` の行を足した。
  - header の「Phase 5（ADR-0007）」の 3 行は、reviewer を「`Standard` tier のプロバイダ」で選び「`review.rs` が起動する」と書いていた。
    今は `review_spawn` の `pick_reviewer` が通常の run と同じ `select_route` で選んで起動し、判定は `review_verdict` が適用する
    （provider の層は ADR 2026-10-11 で無くなった）。今の形の 1 行に書き直した。

commit: 「admit-tidy: dispatch_one の契約違反の分岐を記録し、module map に判断の module の行を足す」

## repo health の行数

最初の版（置き換えと行の追加だけ）は `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` が exit 1 だった。
`size.responsibility-growth` の worsened 3 件と、`source-size` の fail 1 件である。

| 指標 | base | 最初の版 | 提出の版 |
|---|---|---|---|
| `Dispatcher::dispatch_one` の関数の行 | 1017 | 1019 | 1011 |
| dispatcher module の本体の行（空行・注釈・use・mod 宣言を除く） | 31605 | 31611 | 31603 |
| `dispatcher.rs` の file の本体の行（source-size の production、許容 2036） | 2036 | 2038 | 2036 |

増えた分の内訳: dir の let-else が rustfmt で 1 行から 3 行になった（+2）、関数の定義（+4 の本体の行）、module map の 2 行。
提出の版では、`then_some`・`then` の 2 か所（各 -4）と header の古い説明の書き直し（3 行 → 1 行）で、どの指標も base を超えない。
行を別 file に移したり、書式だけで詰めたりはしていない。

## 試験を足さない理由

- 7 か所は admit の契約が守られている間は通らない。admit は ClusterState を `resolve_cluster` が remote のクラスタを返した後、
  WorkUnit を WU の run で dir を解決した後、LaneAssist を lane を決めた後、CredentialGate・VariantPin を route を受けた後にだけ求め、
  dir を解決できなければ `NoTaskDir`、route を受けられなければ `RouteRefused`、pin を確かめられなければ `VariantPinFailed` で止める。
  この順序と止め方は `dispatch_admission/tests.rs` の試験が固定しており、変更の後も通る（下の証拠）。
- 分岐に入ると `tracing::error!` を出して `Ok(false)` を返すだけで、store にも待ちの記録にも書かない。試験で確かめられる違いは
  log の行だけだが、この crate には tracing の出力を捕まえる道具が無い。道具を足すのはこの unit の範囲を超える。
- 7 か所の else は前も後も `Ok(false)` を返すので、`dispatch_one` の戻り値と store の状態は変わらない。既存の
  `cex_`・`dispatch_admission::` の試験が通ることで、契約と起動の振る舞いが変わっていないことを確かめた。

## 証拠（提出時の HEAD で実行。範囲の基準は `$CELERIS_WU_BASE`）

- 計画の check 0（module map の 2 行と古い行の無いこと）: exit 0。
- 計画の check 1（`fn admission_contract_broken` があり、`else { return Ok(false)` の let-else が無い）: exit 0。
  `grep -n admission_contract_broken dispatch_run.rs` は呼び出し 7 行と定義 1 行。
- `cargo nextest run -p task-dispatch -E 'test(/dispatch_admission::/) | test(/cex_/) | test(/structure_guard/)'`: exit 0、54 passed。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"`: exit 0（verdict pass、fail_grade 0、new 0、worsened 0）。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check`: 結果は下の「検査の結果」。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が `dispatcher.rs`・`dispatch_run.rs`・この file だけ）: 結果は下の「検査の結果」。

### 検査の結果

（提出前に書く）

## 未解決事項

- 無し。

## 提案

- 7 か所のどれかが本番で通ったら、`dispatch admission contract broken` の error log の `missing` で fact の名が分かる。
  そのときは admit の順序の試験（`dispatch_admission/tests.rs`）に、その fact の前提を欠いた入力を足す。
