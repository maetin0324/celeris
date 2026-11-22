---
title: 戦略層 v1 子 5 shadow-case — 子 0 の replay の E-mid までの記録で、少数の実験→戦略の選択→逆戻りの拒否を shadow ケースの試験で示す
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# shadow-case（WorkUnit。子 5 の段 shadow）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md) の D7.1・D7.3。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。production のコードは変えていない（試験と fixture だけ）。

## 結論

- `crates/task-api/tests/strategy_review_shadow.rs`（5 件）が、子 0 の E-mid（cutoff 2026-11-18T20:36:35.314647904Z、子 A〜C が done）の 3 file だけを入力に、
  本物の `POST /api/v1/cos/operations` と gate（`cos::strategy_gate`）と store を通して D7.3 の筋書きを最後まで再生した。
  初回の子 A〜H の compound は `strategy_experiment_phase` で止まり、open の候補ごとの atomic の識別実験 3 件だけが通った。実験の結果（E-mid の slice の中の文）を
  証拠に引いた v2 が固定計画の候補を反証で却下して別の候補を選んだ後にだけ、本実装の compound が通った。再評価の v3 は、却下した候補を選ぶと
  `strategy_revisit_rejected`、失敗した実験 task の参照を落とすと `strategy_failure_unread` で止まった。
- 戦略を選ぶまでに起票した task は **5 件**（文書の task 2・識別実験 3）。アーキv4 の実際の初回の起票は **子 8 件**（fixture の `created` の event から測った。
  親の作成 14:50:43Z の 11 秒後に A〜H の 8 件）。
- `crates/task-api/tests/strategy_review_regression.rs`（2 件）が、案件に承認済みの実験段階の版と起票済みの実験があっても、strategy の無いバグ修正の
  `task.create` が `execution` を省いたまま通り、応答・`cos_operations` の行・監査の event が戦略の無い案件と（id・時刻・event の番号を除いて）同じことと、
  Critic の対象が戦略の文書を作った task だけにあることを示した。
- shadow で gate の欠陥は見つからなかった。gate と `task_core::strategy` は変えていない（repair の申告も無い）。

## 実装前 brief

1. 根本原因: 子 5 の規則（D2〜D4）は短い試験用の文書で 1 つずつ確かめただけで、アーキv4 の途中の記録から候補を立てて版を重ねる一続きの流れで、初回の固定計画が
   止まり、実験の結果を見て選び、却下に戻らないかは確かめていなかった。症状は「子 A〜H を最初に全部起票した」アーキv4 の失敗。
2. 責務 owner: 規則は `task_core::strategy`（`validate_document`・`check_execution`）と実行の入口 `task_api::cos::strategy_gate`。この WU は試験だけを書く。
3. 既存の再利用先: 子 0 の `replay.py build` の E-mid の slice と `manifest.sha256`、`tests/common`（`admin_env`・`cos_run_bearer`・`post_op`・`new_task`・`count`）、
   `task_ops::workspace::local_dir`（gate の `Reader::file` と同じ解き方）、`task_core::strategy` の `artifact_name`・`approval_question`・`approval_decision_key`、
   `task_core::strategy_critic::critic_subjects`、回帰では task-core の `strategy/fixtures/baseline-v1.json`。
4. 選択肢: (a) gate の単体試験（`strategy_gate/review_tests.rs`）に足す → 範囲（ADR D8）の外で、fixture を crate の外から読む統合の形にならないので採らない。
   (b) 子 0 の `replay.py` を試験から呼ぶ → 試験が python に依存し、答えの file のある dir を辿るので採らない。(c) 採用: 3 file を fixture に写し、manifest の行で byte 単位の
   一致を確かめ、統合試験から本物の API を叩く。
5. 新しい状態・分岐・fallback・port: 無し（試験と fixture だけ）。
6. 撤去対象: 無し（旧い仕組みを置き換えていない）。
7. 不変条件: 下の「試験」の表。

## 変更

- `crates/task-api/tests/fixtures/strategy_review_shadow/`（新）: `goal.md`・`events.jsonl`・`slice.json`（子 0 の `replay.py build --out "$TMPDIR/replay"` の
  `E-mid/` から写した。sha256 は下の表）。
- `crates/task-api/tests/strategy_review_shadow.rs`（新）: `strategy_review_shadow_` の 5 件。
- `crates/task-api/tests/strategy_review_regression.rs`（新）: `strategy_review_regression_` の 2 件。
- ADR の付記（引いた文の出典の直しと、X(v) に本実装の task が入ることの記録）。

commit（件名）:
- strategy-v1 5 shadow-case: 子 0 の E-mid の記録で実験→選択→逆戻りの拒否を示す shadow ケースと、strategy の無い task.create の回帰の試験
- strategy-v1 5 shadow-case: WU の進捗と ADR の付記

## 入力（fixture）

| file | sha256（`replay/manifest.sha256` の `E-mid/<file>` の行と一致） | 大きさ |
|---|---|---|
| `events.jsonl` | `1ab5c613328afa88f8f5d7276de29bd799a1cbc1fbe5b7ad01abb83a41376e0e` | 870 行、201,119 byte |
| `goal.md` | `ba0f77dd1490c837f38c42172e7a7b1a0ae26d86abca0f9bfd5e25e32cd0270b` | 12,912 byte |
| `slice.json` | `31990c040aa658d96951dcca81b70b5111a1810647017cde96e0df27d0c3f009` | `id: E-mid`、`cutoff: 2026-11-18T20:36:35.314647904Z`、`cost_so_far_usd: 171.0589` |

- 試験は最初に、dir の file の集合がちょうどこの 3 つ、各 sha256 が manifest の行と一致、`slice.json` の id・cutoff・event の数、全 event の ts が cutoff 以下を確かめる
  （`fixture_problems`。最後の行の ts が cutoff ちょうど）。manifest は `include_str!` で子 0 の正本を読む（写しを持たない）。
- 試験 file は採点用の正解 file の名前を書かず、その path を開かない（`grep -c` の結果は 2 file とも 0。fixture の dir にも無い）。
- 案件の `request` は `goal.md` の全文。v1〜v3 の `source_revision.request_sha256` はその sha256。

### 証拠に引いた文（試験が slice の中の実在を確かめる）

| 文 | 出典 | 使った所 |
|---|---|---|
| 「未達/要判断として書いたもの: handlers ⇄ cos」 | `events.jsonl` の D#1446（`worker_finished` の `outcome`、2026-11-18T19:55:39Z） | 固定計画の反証 `e-fixed-refuted` と境界の辺の支持 `e-boundary-named` |
| 「新 17 件・198・756・1,827」 | `events.jsonl` の B#1119（`review_verdict` の `reason`、2026-11-17T21:55:57Z） | 固定計画の反証 `e-fixed-refuted` |

ADR D7.3 は 2 つ目を「C の進捗の 17・198・756」と書いたが、C の進捗の file（`snapshot/agent-docs/progress/2026-11-17-arch-v4-c.md` の 82 行目）は 3 file の外にある。
`events.jsonl` の中で同じ値を書いた文は B#1119（B の後の値。C の進捗の「17・198・756・1,827（B の後…）」と同じ）だけなので、これを引いた（ADR に付記した）。

## 筋書きと各段の結果

すべて CoS の run の bearer で `/api/v1/cos/operations` に送った。拒否はどれも 422 で `codes` がちょうどその 1 つ、task の数が変わらないことを毎回確かめた（`refused`）。
「起票」は `cos_operations` の `task.create`・`applied` の行の累計。文書の task は CoS が strategy 無しで起票し（D6 の手順 1）、試験が run の代わりに文書を成果物に書き、
人（`human:shadow`）の承認の決定を積み、status を `done` にした（final review の Critic の LLM はここでは動かさない）。

| 段 | 要求 | 結果 | 起票 |
|---|---|---|---|
| 1 | v1 の文書の task（strategy 無し）。v1 = 候補 3（`fixed-plan-a-to-h`＝子 A〜H の固定計画、`cut-boundary-edges`、`recount-baseline`）、selected 無し | 200。人の承認・done・Critic の対象 1 件 | 1 |
| 2 | `[アーキv4 A〜H] 固定の計画を全面起票`（compound、candidate `fixed-plan-a-to-h`） | 422 `strategy_experiment_phase` | 1 |
| 3 | 同じ compound（candidate 無し） | 422 `strategy_experiment_phase` | 1 |
| 4〜6 | open の候補ごとの識別実験（atomic、`max_wall_secs` は実験の予算 2・4・3 時間ちょうど） | 200 × 3 | 4 |
| 7 | `fixed-plan-a-to-h` の 2 件目の実験 | 422 `strategy_experiment_phase` | 4 |
| 8 | 実験の結果を成果物 `result.md` に書き終端に: 固定計画 done、境界の辺 done、数え直し **failed** | — | 4 |
| 9 | v2 の文書の task（strategy 無し）。v2 = 固定計画を `rejected`（`rejected_by: [e-fixed-refuted]`）、`cut-boundary-edges` を selected、終端の実験 3 件を全部証拠で参照 | 200。承認・done・Critic の対象 | **5（戦略を選ぶまで）** |
| 10 | v2 で candidate `fixed-plan-a-to-h` の compound | 422 `strategy_revisit_rejected` | 5 |
| 11 | v2 で本実装の compound（candidate 省略） | 200。監査の `payload_json.strategy` が v2 の参照 | 6 |
| 12a | 本実装を failed にし、v3 の文書の task を起票。v3 = 却下した固定計画を selected（終端の 4 件は全部参照）→ compound | 文書の task 200、compound 422 `strategy_revisit_rejected` | 7 |
| 12b | 本実装を done にし、v3 の文書の task を起票。v3 = 同じ候補のまま、失敗した数え直しの実験 task の参照を落とす → compound | 文書の task 200、compound 422 `strategy_failure_unread` | 7 |

- 段 1〜11 の拒否は 4 件（実験段階 3・却下した候補 1）。12a・12b は別の試験で、それぞれ拒否が 1 件増える。
- 試験の出力（`--no-capture`）:
  ```
  strategy_review_shadow: 戦略を選ぶまでに起票した task = 5（文書の task 2・識別実験 3）。アーキv4 の初回の起票 = 子 8 件
  strategy_review_shadow: 拒否 4 件（実験段階 3・却下した候補 1）、本実装を含む起票 6
  ```

## 試験

| 試験 | 守る不変条件 |
|---|---|
| `strategy_review_shadow_fixture_is_e_mid_only` | fixture はちょうど E-mid の 3 file で manifest と一致、全 event が cutoff 以下、引く 2 文が slice の中に実在、アーキv4 の初回の起票は子 8 件 |
| `strategy_review_shadow_rejects_input_with_lines_after_cutoff` | 負の試験: cutoff の 1 ns 後の行を混ぜた `events.jsonl` を、その行（`#999999`）を名指しして拒否し、manifest の sha256 とも食い違う。cutoff ちょうどの行は拒否しない。余分な file を置いた dir も拒否 |
| `strategy_review_shadow_experiments_first_then_select` | 段 1〜11。初回の固定計画が止まり、実験 3 件だけが通り、結果を引いて選んだ版だけが compound を通す。v2 が実験 task 3 件を全部参照し、却下の根拠が反証の証拠 |
| `strategy_review_shadow_revisit_rejected_after_failure` | 段 12a。本実装の失敗の後に却下した候補に戻る版は `strategy_revisit_rejected` だけで止まる |
| `strategy_review_shadow_dropped_terminal_experiment_is_unread` | 段 12b。失敗した実験 task を読まない版は `strategy_failure_unread` だけで止まる |
| `strategy_review_regression_bug_fix_without_strategy_unchanged_by_experiment_phase` | 案件に承認済みの実験段階の版と実験があっても、strategy の無いバグ修正（`execution` 省略）は通り、応答・`cos_operations` の全列・監査の `cos_operation` の event が戦略の無い案件と同じ（id・時刻・`event_id` を伏せて比べる。`request_hash` は伏せずに一致）。同じ案件で strategy を付けた compound は 422（gate が効いている対照） |
| `strategy_review_regression_no_critic_subject_without_strategy_artifact` | Critic の対象は文書を作った task だけ。strategy の無いバグ修正の task と、strategy を参照して作った実験 task（成果物は `result.md`）は対象 0 件で、保存された受け入れ条件は要求のまま |

既存の試験で検出できない理由: `strategy_gate_review_`（experiment-gate）は短い試験用の文書で規則を 1 つずつ確かめ、`strategy_review_contract_` は純関数だけを見る。
どちらも子 0 の記録から立てた 3 候補で実験 → 選択 → 本実装 → 再評価と版を重ねる流れと、起票の数を見ない。子 2 の `strategy_gate_absent_field_keeps_task_create_unchanged` は
戦略の無い案件しか見ず、critic-review の試験は手で組んだ events しか見ない。

### 変更前に落ちること（gate の規則を外した変異）

`crates/task-api/src/cos/strategy_gate.rs` の `out.extend(strategy::check_execution(doc, &facts));` を一時的に外して（D2〜D4 の入口の規則が無い gate。子 2 までの gate と同じ判定）
`cargo nextest run -p task-api --test strategy_review_shadow --test strategy_review_regression --no-fail-fast` を実行した。exit 100、`7 tests run: 2 passed, 5 failed`。
log は run の成果物 `tests-mutation-no-check-execution.log`。確かめた後に `git checkout -- crates/task-api/src/cos/strategy_gate.rs` で戻した（差分 0）。

```
FAIL strategy_review_shadow_experiments_first_then_select           strategy_review_shadow.rs:370:9  left: 200  right: 422
FAIL strategy_review_shadow_revisit_rejected_after_failure          strategy_review_shadow.rs:370:9  left: 200  right: 422
FAIL strategy_review_shadow_dropped_terminal_experiment_is_unread   strategy_review_shadow.rs:370:9  left: 200  right: 422
FAIL strategy_review_regression_bug_fix_without_..._experiment_phase strategy_review_regression.rs:193:9  left: 200  right: 422
FAIL strategy_review_regression_no_critic_subject_without_...        strategy_review_regression.rs:193:9  left: 200  right: 422
```

shadow の 3 件は段 2（子 A〜H の compound）が通ってしまい落ちた。回帰の 2 件は gate が効いていることの対照（strategy 付きの compound の 422）で落ちた。
fixture の 2 件は gate に依らないので通る。回帰の本体（strategy の無い要求が変わらないこと）は base でも通る回帰の守りである（ADR D7.2 の回帰と同じ扱い）。

## 検査（提出時の HEAD で実行）

- `cargo nextest run -p task-api --test strategy_review_shadow --test strategy_review_regression`: `7 tests run: 7 passed`。`cargo nextest list -p task-api` で
  `strategy_review_shadow_` 5 件・`strategy_review_regression_` 2 件。
- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-0/replay/replay.py verify`: exit 0（`365 files reproduced byte-for-byte`、`12 slices, 0 leaks`、否定対照 N1〜N3 を検出）。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest `6256 tests run: 6256 passed (11 slow), 28 skipped`（experiment-gate の提出時の 6249 から新しい 7 件が増えた）、
  web `Test Files 105 passed (105)`・`Tests 745 passed (745)`、`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-api --tests -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `grep -c answer_key crates/task-api/tests/strategy_review_shadow.rs crates/task-api/tests/strategy_review_regression.rs`: 0・0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は fixture の 3 file・試験 2 file と、この進捗・ADR の付記だけ。

## LoC（`git diff --numstat` 基準 `$CELERIS_WU_BASE`、進捗と ADR を除く）

| 区分 | file | 追加 | 削除 |
|---|---|---|---|
| production | なし | 0 | 0 |
| test（Rust） | `crates/task-api/tests/strategy_review_shadow.rs`（新） | 1,016 | 0 |
| test（Rust） | `crates/task-api/tests/strategy_review_regression.rs`（新） | 415 | 0 |
| fixture | `tests/fixtures/strategy_review_shadow/` の 3 file（子 0 の slice の写し） | 916 | 0 |

## 未確認と観察

- Critic（reviewer）の LLM の判定はこの試験では動かしていない。文書の task は status を直接 `done` にした（ADR D7.3 のとおり）。判定の質は子 6・子 8 の pilot で確かめる（未確認）。
- 実験 task の結果は試験が書いた（run は動かしていない）。結果の文は E-mid の slice の中の文で、slice の外の知識は使っていない。実際の CoS が同じ 3 候補を立てるかは未確認。
- 観察: X(v)（D2.3）は版 v を参照した `task.create` の全部なので、v2 で起票した本実装の task も X(v2) に入る。したがって再評価の v3 は、本実装の task が終端
  （done・failed・cancelled）になってからでないと `strategy_experiment_phase`（前の版の task が終わっていない）で止まり、v3 はその task も証拠で参照しないと
  `strategy_failure_unread` になる。D2.4 の 6・D4.2 の文のとおりで欠陥ではないが、実行中に pivot するには走っている本実装を先に cancel する必要がある。
  段 12a・12b はこれに合わせて本実装を終端にしてから v3 を作った。
- strategy の無い `task.create` は止まらない（ADR の「未確認と残る穴」のとおり）。shadow の文書の task も strategy 無しで起票している。

## 提案

- 子 6（途中の親の評価と pivot）: checkpoint が戦略を変える版を作る前に、版の X に残る走っている task（本実装を含む）を cancel する手順を CoS の skill
  （`config/skills/cos-operator/strategy-review.md`）に足すか、子 6 の設計で決める。今の gate はその順を強制する（上の観察）。

## 親の目的への寄与（確認済み・推定・未確認）

- 確認済み（試験）: 改善 A — アーキv4 の E-mid の記録から立てた 3 候補で、初回の子 A〜H の固定計画（compound）は実行の入口で止まり、起票できたのは識別実験 3 件だけだった。
  戦略を選ぶまでの起票は 5 件で、アーキv4 の初回の 8 件より少ない。改善 C の一部 — 実験の結果（固定計画の反証）を見て候補を選び直した版だけが本実装を通し、
  再評価で却下した候補に戻る版と失敗の記録を読まない版は止まった。通常の低抽象度 task は変わらない（回帰）。
- 推定: アーキv4 の当時に CoS がこの手順（strategy を付けた起票）を使っていれば、E-mid の時点で固定計画の候補を反証で却下でき、D〜H の固定計画をそのまま進めることは
  入口で止まった。ただし strategy を付けない起票は止まらないので、CoS が手順を使うかに依存する。
- 未確認: Critic の判定の質、実際の CoS と reviewer の run での再現、費用（実験 3 件の予算の合計 40 USD と E-mid までの 171.06 USD の比較は文書の予算で、実測ではない）。
  第二ゲート（子 6）の「pivot と独立評価が実際に起きた」の判定は子 6 が行う。
