---
title: S5 cos-ts-scenario — edit-activate-baseline.sh に cos-ts と Rust 側の対 cos-digest を足す
tasks: [01M6FPPJN7A8H427MXFVYFY7H4]
status: done
completed: 2026-11-03
updated: 2026-11-03
---
# S5 cos-ts-scenario — cos-ts と cos-digest の scenario

完了日: 2026-11-03（unit cos-ts-scenario）。[境界 ADR](../../adr/2026-11-02-ts-control-layer-boundary.md) の付記 S5-10・S5-11・S5-12 に従う。

## 結論
- `scripts/dev/edit-activate-baseline.sh` に scenario `cos-ts`（TS の経路）と `cos-digest`（旧 Rust の経路）を足した。`--dry-run`・`--validate`・`--check-targets` が両方を扱う。
- 足した option: `--rep-start N`（後半の測定は 11 から）、`--series <leaf の key>`。
- 新しい行に足した欄: `env.identity`、`trial_load`、`trial_secs`、`over_300s`。`env.identity` は commit・clean・tree hash・rustc・node・pnpm・host・target の種別・series を持ち、TS の試行では `bundle_sha256`・`host_api`・`runner_sha256` も持つ。`trial_load` は試行の前後の 1 分 load average と、自分以外の cargo・rustc の数と、busy の印を持つ。
- 1 試行ずつの実測（warm、host home-dev）: cos-ts が 7.729 秒、cos-digest が 345.217 秒（300 秒を超えたので `over_300s: true`）。
- 既存の `policy`・`cos`・`policy-ts` は、編集の内容・段・判定の目標を変えていない。新しい行に欄が増えただけ。識別（identity）の無い古い記録は今までどおり `--validate` を通る。

## 編集の差分（同じ論理: reason が nonce 入りの印を含む run には command を出さない。当たらない）
cos-ts（`control-ts/src/domains/cos_digest.ts` の `decide`。anchor は `const { text, cards } = digestMessage(...)` の行で、その直前に入れる）:
```diff
 export function decide(input: CosDigestInput): CosDigestOutput {
+  // edit-activate-baseline probe <nonce>: a branch that is never taken
+  if (input.reason !== null && input.reason.includes("edit-activate-baseline-<nonce>")) {
+    return { commands: [] };
+  }
   const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);
```
cos-digest（`crates/task-core/src/chat/digest.rs` の `cos_digest_decide`。anchor は `let (text, cards) = cos_digest_message(` の行）:
```diff
 pub fn cos_digest_decide(input: &CosDigestInput) -> Vec<CosDigestCommand> {
+    // edit-activate-baseline probe <nonce>: a branch that is never taken
+    if input
+        .reason
+        .as_deref()
+        .is_some_and(|r| r.contains("edit-activate-baseline-<nonce>"))
+    {
+        return Vec::new();
+    }
     let (text, cards) = cos_digest_message(
```
`<nonce>` は試行ごとに違う 8 桁の 16 進数。

## 検証責任の揃え方
- TS の経路（cos-ts）: `ts-verify.sh` の 10 段（classify・runner-cache・install・typecheck・bundle・unit・golden・safety・integration・activate）。golden・integration は runner の `cos_digest` の golden（cases と pass_traces）を採点する。activate は候補 bundle を一時 state dir に配備し、新しい isolate に load して selftest と各領域の probe を呼ぶ。
- Rust の経路（cos-digest）: S0 と同じ 6 段（check・build・clippy・test-build・test-run・release-build）を、対象 crate を 2 つにして走らせる（`-p task-core -p task-dispatch`）。
  - task-core は付記 S5-10 の決定どおり対象に入れる。`tests/script_fixtures.rs` が fixture と Rust の出力の一致を見る。
  - task-dispatch は objective の指定で足した。`dispatcher/tests/cos_digest_*.rs` が digest の pass と Rust/TS の比較を見る。これで TS 側の golden・pass の採点に当たる検査が Rust 側にもそろう。
  - Rust の経路は 2 crate の全試験を走らせるので、TS の経路より検証が広い。付記 S5-10 のとおり、Rust 側を狭めて揃えることはしない。

## 300 秒を超えた件（cos-digest）
- 結果: 1 試行が 345.217 秒で、300 秒を超えた。
- 扱い: 付記 S5-10 の「時間」の決定に従った。行を捨てず、`over_300s: true` の印を付け、`--check-targets` の p50・p95 に含める。`--check-targets` は scenario ごとに `over_300s` の件数も出す。
- 原因: 対象 crate が task-core と task-dispatch の 2 つあること。段ごとの秒は test-build 80.2 秒、release-build 94.0 秒、build 55.5 秒、test-run 50.5 秒、clippy 34.4 秒、check 30.6 秒。
- 段を減らす・crate を外すことはしなかった。付記 S5-10 が、測定のために手順を変えないこと、Rust 側を狭めないことを決めているため。

## 実測（各 scenario 1 回、`--reps 1`、warm。測定の本番は s6-measure）
提出前の commit「phase s5: cos-ts-scenario: edit-activate-baseline.sh に cos-ts・cos-digest、--rep-start、S6 の識別と試行の load を足す」の作業ツリー（clean）で測った。series は `cos-ts-scenario`。生データは run の artifacts（`raw-cos-ts.jsonl`・`raw-cos-digest.jsonl`）にだけ置いた。S6 の生データの集計に混ぜないため、この dir には置いていない。

| scenario | 1 試行の秒 | 内訳 | 識別 |
|---|---|---|---|
| cos-ts（warm） | 7.729（ts-verify の total 7.4 前後） | classify 0.116・runner-cache 0.175・install 0.002・typecheck 0.093・bundle 0.076・unit 1.633・golden 0.803・safety 0.039・integration 4.403・activate 0.130。class ts-only、cargo の起動 0、link 0 | tree（`HEAD:control-ts`）・bundle_sha256・runner_sha256 は記録に在る、host_api 1.1.0、target_kind none |
| cos-digest（warm） | 345.217（`over_300s: true`） | check 30.649・build 55.519・clippy 34.354・test-build 80.168・test-run 50.491・release-build 94.036。全段 exit 0 | tree（`HEAD:crates/task-core`・`HEAD:crates/task-dispatch`）は記録に在る、target_kind warm-shared |

- 共通の環境: rustc 1.98.1、node v24.14.1、pnpm 12.6.0、host home-dev。
- cos-ts の試行: 前後の 1 分 load は 1.74→1.76、自分以外の cargo・rustc は 0（busy false）。
- cos-digest の試行: 前後の 1 分 load は 12.3→12.72、自分以外の cargo・rustc は 0（busy false）。load が高いのは、自分の cargo（release-build 等）の直後だったため。
- cos-digest の prime（集計に入れない）: check 39.3・build 64.5・clippy 33.6・test-build 83.6・test-run 62.7・release-build 140.4 秒。release の target が無い状態から始めた。
- 判定:
  - `--validate`: cos-ts は 3 行・cos-digest は 52 行（prime 6・step 6・estimate 40）で、どちらも problems 0。
  - `--check-targets`: cos-ts は warm の p50 = p95 = 7.729 で目標の内、exit 0。cos-digest は trials 1・over_300s 1・p50 = p95 = 345.217 で、exit 0（Rust の経路には上限が無い）。

## 試験
- `bash scripts/dev/tests/edit-activate-cos-ts.sh`（新規。引数なし）: 103 passed, 0 failed。偽の cargo・偽の ts-verify・偽の runner cache と git fixture を使い、次を確かめる。
  - dry-run（2 scenario と既存の 3 scenario）
  - anchor が本物の file に 1 回だけ在ること
  - 実行の後に編集が戻ること（成功・失敗の両方）
  - `--rep-start 11` で rep 11・12 になること
  - `-p task-core -p task-dispatch` で呼ばれること、prime は編集なしで試行は編集ありになること
  - `env.identity`・`trial_load`・`over_300s` の中身
  - `--validate` が新しい欄の欠け・型の誤りを拒むこと（27 通り）。識別の無い S0 の行は通すこと
  - `--check-targets` が識別（tree・toolchain）の混ざり・段の欠けで落ちること、300 秒を超えた試行を集計に入れること
- `bash scripts/dev/tests/edit-activate-policy-ts.sh`: 90 passed, 0 failed。直した点は 2 つ。
  - 「cos-ts の TS scenario は無い」（exit 2）を、未知の scenario `lane-ts` の確認に替えた。
  - 偽の ts-verify が、`TS_VERIFY_KEEP_WORK` を受けたときに activate の出力を残すようにした。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh`: exit 0。「試験対象の変更なし」。変えたのは scripts/dev の script と shell 試験だけで、shell 試験は test-affected から呼ばれない。上の 2 本を別に走らせた。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## 未解決事項
1. cos-digest の 1 試行は約 345 秒かかる。m-cosd-a・m-cosd-b は 1 leaf に 10 試行を割り当てているが、次の計算で 3600 秒に収まらない見込み。
   - warm の prime: 約 300〜420 秒（release の target が在れば短い）
   - 試行: 10 × 345 秒
   - 付記 S5-11 の 6（時間切れ）が働けば、試行数は「未達（N/10）」になる。
2. ts-verify の段 typecheck は 0.093 秒だった。tsc の incremental の効果か、型検査の範囲が狭いのかは確かめていない（未確認）。overhead unit で確かめる。
3. `trial_load.other_cargo_rustc` は `/proc/<pid>/environ` を読める process（同じ user）だけを数える。ほかの user の cargo・rustc は数えない。

## 提案
- m-cosd を 5 試行ずつの 4 leaf（rep 1〜5・6〜10・11〜15・16〜20、`--rep-start` で番号を続ける）に分ける。または、1 leaf の試行数を 8 にして leaf を 3 本にする。どちらでも、a・b の連結の規則（付記 S5-10）は件数が増えるだけで、そのまま使える。
- s6-report は `--check-targets` の `over_300s` の件数を報告に載せる。
