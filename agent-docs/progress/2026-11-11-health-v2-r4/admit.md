---
title: "健全性v2 R4 admit: run 起動の可否の判断を dispatch_admission へ分ける"
tasks: [01M7BYQNFCE4Z8PFD95T6VQ37J]
status: done
updated: 2026-11-14
completed: 2026-11-14
---
# 健全性v2 R4 admit

完了日: 2026-11-14

## 結論

- `dispatch_one`（`crates/task-dispatch/src/dispatcher/dispatch_run.rs`）の起動可否の判断の順序と組み合わせを、純粋関数
  `dispatch_admission::admit(&AdmissionInput) -> AdmissionProposal`（`crates/task-dispatch/src/dispatch_admission.rs`）へ移した。
  admit は次に集める fact の要求（`Need`）・止め方（`Stop`）・起動（`Launch`）のどれかを返す。順序と決定の正本は
  [ADR 2026-11-14 dispatch-admission-order](../../adr/2026-11-14-dispatch-admission-order.md)（D1〜D5）。
- admit に移した止め方と順序（ADR 2026-11-14 D5 の 1〜30。前の gate が止めたら後ろの fact を求めない）:
  - 前半（admit-gate、1〜16）: 知識整理の入力 → 同じ task の run（1 本目だけ）→ 並列度の枠（`TaskWait::Capacity`）→ 打ち切ったばかりの
    run → eligible → disk の critical（`TaskWait::DiskCritical`）→ 担当の決定（1 本目だけ）→ execution gate と `plan_invalid` の atomic
    （1 本目だけ）→ WU の gate → 決定の待ち → 設計 review の待ち（1 本目だけ）→ 木の上限 → WU の gate の種類（段の統合・最終 review の開始）→
    planner run かの組み合わせと予算の上書き → 再試行の backoff → infra の backoff → browser の前提。
  - 置き場所（admit-place、17〜24）: 予算の上書きを task に当てる（`RunOverride`）→ write-set の gate → クラスタの解決（unroutable・
    道具が無い担当の blocked）→ cooldown → 並列度 → 接続の確認 → 自動接続 → 作業場所の dir → WU の用意の backoff と worktree →
    disk の見込み → scope env。
  - route（admit-launch、25〜30）: langmem の確認と lane の決定 → 分類の待ち → lane の引き上げの保留 → route（拒否は `note_route_refusal`）→
    credential の待ち → variant の pin（確かめられなければ `block_variant_task`）。
  - 並列 WU の 2 本目以降が飛ばす gate の集合（「1 本目だけ」）は移行前と同じ。置き場所と route の gate は 2 本目以降も見る。
- `dispatch_one` に残した副作用（ADR 2026-11-14 D4）: 待ちの記録・遷移・担当の書き戻し・接続（`set_cluster_connected`・
  `mark_cluster_unavailable`・`note_cluster_waiting`）・作業場所の用意（`prepare_work_unit_workspace`・`on_work_unit_prepare_failed`）・
  route の記録・credential の待ち（`credential_gate_hold`）・lease（`acquire_lease`・`acquire_work_unit_lease`）・`WorkerStarted` 等の
  event・`spawn_worker`。どれも admit が求めたとき、または止め方・起動として返したときだけ、移行前の順で行う。
- 個々の判定は既存の関数と owner の結果をそのまま使う（`RunLoad::admits`・`browser_prereq_applies`・`admit_write_set` の
  `WriteSetAdmission`・`judge_run`・`credential_gate_hold`・`variant_run_pin` など）。`dispatch_admission` に同名の関数の定義は無い。
  `dispatch_admission` は TaskStore・file・process・`Dispatcher` に触れない（下の証拠の grep）。
- `dispatch_one` の行数（`fn dispatch_one` の行から同じ字下げの閉じ括弧までを数えた）: 移行前 1089 行（30〜1118 行）→ admit-gate の後
  1022 行 → admit-place の後 1018 行 → 移行後 1017 行（35〜1051 行）。`dispatch_run.rs` は 1704 行 → 1652 行。
  `dispatch_admission.rs` は 875 行、`dispatch_admission/tests.rs` は 1138 行で `#[test]` 17 本。
  base は admit の task の最初の commit の前（integrate wu/settle-root の後）の木。
- ADR 2026-11-11 と `cex_` の試験と dispatcher の既存の試験は変えていない。
- `dispatch_admission.rs` は main の `scripts/dev/repo-health/profiles/celeris.toml` の `[[review.high_risk]]` の budget の class の
  正規表現（1 本）に当たる。

## 各 unit の要点

| unit | 要点 | 進捗 |
|---|---|---|
| admit-gate | ADR 2026-11-14 を書き、`dispatch_admission` を作った。capacity から browser の前提まで（D5 の 1〜16）を fact を 1 つずつ求める loop にした。`second_pass` の条件式・`shadow_human_explicit_compound`・`tree_child_compound` を消した。`AdmissionTaskValues::read` を dispatch_admission に置き、dispatcher module の本体の行を WU base 以下に戻した（1 回目の提出は repo health の worsened で落ちた） | [admit-gate.md](admit-gate.md) |
| admit-place | 予算の上書きを当てる位置から scope env まで（D5 の 17〜24）を移した。クラスタの比較（cooldown の期限・並列度・接続）と止め方ごとの分岐を消し、`>= spec.concurrency`・`until > now`・`Admission::Wait(` は `dispatch_run.rs` に無い。試験 8 本 → 13 本 | [admit-place.md](admit-place.md) |
| admit-launch | lane の保留から variant の pin まで（D5 の 25〜30）を移し、dispatch_one は適用だけになった。`if self.defer_for_classification(`・`if held`・`if self.credential_gate_hold(` は `dispatch_run.rs` に無い。planner run の `LaneDecision` の組み立てを `planner_lane` に移した。試験 13 本 → 17 本 | [admit-launch.md](admit-launch.md) |
| admit-record（この unit） | 統合後の HEAD で検査を流し、この進捗を書いた。コードと ADR は変えていない | この file |

各 unit は、一時的に admit の本体を書き換えて順序を入れ替えると試験が落ちることを確かめ、元に戻した（例: credential の待ちと
variant の pin の入れ替えで 6 本、cooldown と並列度の入れ替えで 2 本、再試行と infra の backoff の入れ替えで 2 本が落ちた。
詳細は各 unit の進捗）。

## 変更（件名）

- admit-gate: `phase admit: dispatch_one の前半の起動可否の順序と組み合わせを dispatch_admission::admit へ移す`、
  `phase admit: admit-gate の進捗に検査の結果を書く`、
  `phase admit: AdmissionTaskValues の読み出しを dispatch_admission へ移し、dispatcher module の行を base 以下に戻す`、
  `phase admit: admit-gate の進捗に再提出の検査の結果を書く`
- admit-place: `phase admit: write-set・cluster・作業場所・disk の起動可否の順序と組み合わせを dispatch_admission::admit へ移す`、
  `phase admit: admit-place の進捗に検査の結果を書く`
- admit-launch: `phase admit: lane の保留・route・credential・variant の pin の起動可否を dispatch_admission::admit へ移す`、
  `phase admit: admit-launch の進捗を書く`
- 統合: `integrate wu/admit-launch (phase admit)`
- admit-record（この unit）: この進捗ファイルだけ。

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

統合後の branch（integrate wu/admit-launch の後）の HEAD で実行した。

- 計画の check（純粋さ・旧い判断の不在・試験）: exit 0。
  - `dispatch_admission.rs`・`dispatch_admission/` に、check が grep する永続化・file・process・`Dispatcher`・`dispatcher` module の字面は
    無い（grep exit 1）。`admits`・`account_run_limit`・`eval_admission`・`check_assignee`・`browser_prereq_applies`・
    `reservations_overlap` の定義も無い。
  - `admit`・`AdmissionInput`・`AdmissionProposal` は根の file にあり、`dispatch_run.rs` が `dispatch_admission::admit(` を呼ぶ。
    `acquire_lease`・`spawn_worker(`・`WorkerStarted` は `dispatch_run.rs` に残る。
  - `dispatch_run.rs` に、`second_pass` の条件式・`shadow_human_explicit_compound`・`tree_child_compound`・`>= spec.concurrency`・
    `until > now`・`Admission::Wait(`・`if self.defer_for_classification(`・`if held`・`if self.credential_gate_hold(` は無い。
  - `dispatch_admission` の `#[test]` は 17 本（12 本以上）。
  - `cargo nextest run -p task-dispatch -E 'test(cex_) | test(/^dispatch_admission::/) | test(structure_guard_)'`: 53 passed、1809 skipped。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
  `bash scripts/dev/check-health.sh`: exit 0（repo_health_gate: all checks passed）。
- `bash scripts/dev/test-affected.sh`: exit 0。repo_health_gate は all checks passed、「試験対象の変更なし（docs のみ等）」。
  この unit はコードを変えていないので crate の試験は走らない。統合後のコードの試験は admit-launch の test-affected
  （3917 passed、11 skipped）と、上の nextest の 53 passed に拠る。
- main の profile の high_risk の正規表現を `crates/task-dispatch/src/dispatch_admission.rs` に当てた: budget の class の 1 本が当たる。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`: exit 0。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ agent-docs/adr/`: exit 0（コードと ADR は不変）。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 path はこの進捗ファイルの 1 件、範囲外 0。

## 未解決事項

- cex-review の反例 2（[cex-review.md](cex-review.md) の「反例」の 2）は、この task の範囲外として残した。計画の無い（atomic）task は、
  credential の検証が一時失敗しても、秘密 file があるだけで run を起こす。原因は `credential_gate_hold`
  （`crates/task-dispatch/src/dispatcher/credential_gate.rs`）が `is_registered` が真なら開いた credential_wait の検証状態を見ずに通すこと
  で、atomic task は WU の Blocked 行を持たない。owner は credential_gate.rs で、admit は `credential_gate_hold` の結果を組み合わせるだけ
  である。この task は「個々の判定を書き直さない・権限と予算の判定と順序を変えない」ことが条件なので、判定そのものを直す変更は入れていない。
  credential_gate.rs は高 risk の path class `credential` に当たり、直すには設計 brief と設計 review が要る。
- planner run の system の側の lane の記録（`planner/system-<tier>`）を直接確かめる試験は無い（移行前も無かった。admit-launch.md）。

## 提案

- 反例 2 を直す別 task を作る。内容: `credential_gate_hold` が atomic task でも開いた credential_wait の検証状態を確かめ、検証の成功と
  resume を経るまで run を起こさないようにする。`cex_credential_wait_holds_task_and_unit_across_retry_deadline`（試験 11）に atomic task の
  変種の assert を足す（再現手順は cex-review.md の反例 2 と同じ）。path class `credential` の設計 brief を先に書く。admit の順序（D5 の 29）は
  変えない。
