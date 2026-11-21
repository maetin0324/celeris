---
title: 戦略層 v1 子 1 decision-failclosed / fix — 配列でない decisions を受け付けなかった要求 1 件にし、要求を出した仕事を止める
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# decision-failclosed / fix — 配列でない `decisions` を fail-closed にする

完了日: 2026-11-21（`decisions-not-array` の修正。残る経路は同じ段の unit settle-leaf・settle-failure・settle-atomic が直す）

## 結論

- survey の to-fix の経路のうち、owner が許可範囲の中にある `decisions-not-array` を直した。`result.json` の `decisions` が
  object・文字列・数・真偽値のとき、daemon は key `rejected` の決定（open・origin worker・`needed_before` は leaf の key、
  atomic なら `self`）を 1 件記録し、要求を出した仕事を答えが出るまで止める（ADR 2026-11-21-worker-decisions-fail-closed D2）。
- `decision_fail_closed_` で始まる試験 3 件を先に書いた。修正前は 3 件とも assertion で落ち（before.log、exit=100）、修正後は
  3 件とも通る（after.log、exit=0）。
- 残る経路（`continuable-end`・`failed-end`・`question-and-decisions`）は、直す owner が `run_settlement`・`worker_finish.rs` にあり、
  この unit の対象のパスの外にある。replan で同じ段に足された unit settle-leaf・settle-failure・settle-atomic が直す。この unit は
  範囲の外に回避策を置かず、`plan_issue` を返さない。

## 実装前 brief

1. 根本原因: `task_core::decision::worker_decisions_from_json` が `decisions` の欄を `as_array()` で読み、配列でない値を空の配列にした。
   `Dispatcher::worker_decisions` は空なら `None` を返すので、決定の行も進行の行も残らず、leaf（atomic の節点）は done になった。
2. 責務 owner: `task_core::decision`（`worker_decisions_from_json`・`prepare_worker_decisions`）と `Dispatcher::worker_decisions`
   （`crates/task-dispatch/src/dispatcher/tree_units.rs`）。
3. 既存の再利用先: 11-18 の ADR の拒否のまとめ（`RejectedRequest` → `rejected_decision`・`free_key`・D3 の読み直しの抑止）と、
   既存の記録・止めの経路（`WorkerDecisionBatch::recorded` → `DecisionRequested`、`self_hold` → `hold_for_worker_decisions`・
   atomic は `hold_task_for_worker_decisions`、`decision_self_hold`）。試験は `tree_decisions.rs` の `DecisionAdapter`。
4. 選択肢: (a) 欄の型の誤りを要素の拒否と同じ `RejectedRequest` 1 件にする（採用。新しい経路が要らない）。(b) 決定 1 件の形の
   object を 1 件の配列として読み替える（不採用。ADR D2・I4: 承認の無い読み替えを増やさない）。(c) run を失敗にして worker に
   書き直させる（不採用。11-18 の ADR D2 と同じ理由）。
5. 新しい状態・分岐・fallback・port: 無し。`prepare_worker_decisions` の入力の型の場合分け（配列・`null`・それ以外）だけを足した。
   通知の仕組み・Manager・DB 表・run の失敗の種類・決定の kind は足していない。
6. 撤去対象: `worker_decisions_from_json` の「配列でない…ときは空」の説明と挙動、`Dispatcher::worker_decisions` の
   `raw.is_empty()` の分岐、`worker_decisions_are_validated_and_self_is_resolved` の末尾の `.len()` の確かめ（新しい戻り値に書き換えた）。
7. 不変条件: ADR の I1〜I4。I1・I2・I4 は 3 件の `decision_fail_closed_*`、I3（欄が無い・`null`・空の配列・JSON でない本文は
   何も作らない）は task-core の `decision_fail_closed_non_array_decisions_become_one_rejected_decision` の末尾と
   `worker_decisions_are_validated_and_self_is_resolved` の末尾、既存の配列の経路は `rejected_worker_decision*`・`worker_decisions*`・
   `worker_declared_decisions` が確かめる。

## 変えた関数

- `task_core::decision::worker_decisions_from_json`（`crates/task-core/src/decision.rs`）: 戻り値を `Vec<Value>` から
  `Option<Value>` にした。要求が無い（欄が無い・`null`・空の配列・本文が JSON でない）ときは `None`、それ以外は欄の値をそのまま返す。
  ADR D2 は空の配列の判定を `Dispatcher::worker_decisions` に置く書き方だったが、「要求が無い」の定義を 1 か所にするため
  この関数に入れた（`Dispatcher::worker_decisions` が `None` を返す条件は ADR のとおり）。
- `task_core::decision::prepare_worker_decisions`: 入力を `&[Value]` から欄の値 `&Value` にした。配列なら今までどおり要素を読む。
  `null` なら何もしない。それ以外は `RejectedRequest`（要約 `#field`〈object なら文字列の `key`・`question` を続ける〉、理由
  `decisions must be an array of decision objects (got <型>)`、後戻りは分からないので `high`）を 1 件足し、既存の
  `rejected_decision` でまとめる。
- `raw_summary`: 位置を引数（`#n` か `#field`）にした。`json_type`（型の名前）を足した。
- `Dispatcher::worker_decisions`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）: `let Some(field) = worker_decisions_from_json(..)
  else { return Ok(None) }` にし、欄の値を `prepare_worker_decisions` に渡す。記録と止めの経路は変えていない。

## 足した試験（守る不変条件と、既存の試験で検出できない理由は各試験のコメントに書いた）

- `task-core decision::tests::decision_fail_closed_non_array_decisions_become_one_rejected_decision`: object（決定 1 件の形）・
  文字列・数・真偽値の欄で、leaf（`self_key = a`）と atomic（`None`）の両方について、`rejected` 1 件・`needed_before = [a]`／`[self]`・
  後戻り `high`・問いに `#field` と型の理由、受け付けた決定 0 件。欄が無い・`null`・空の配列・JSON でない本文は何も作らない。
- `task-dispatch dispatcher::tests::tree_decisions::decision_fail_closed_non_array_decisions_hold_the_leaf`: leaf a が `decisions` に
  決定 1 件の形の object を書く。`rejected` の行（open・origin worker・`needed_before = [a]`・run id は a の run）が受信箱
  （`task_ops::decision::open_rows`）に出て、a は `blocked(decision)`、integrate-s1 と root は done にならない。答えると a が
  走り直し（runs 2）、木が done になり、replay が一致する。
- `task-dispatch dispatcher::tests::tree_decisions::decision_fail_closed_non_array_decisions_hold_the_atomic_node`: 木の子の atomic の
  節点 k が `decisions` に文字列を書く。節点の task に `rejected`（`needed_before = [self]`）ができて受信箱に出て、節点は `ready` の
  まま run 1 本で止まり、root は done にならない。答えると節点が走り直して（run 2 本）done、root も done。

既存の試験の変更（期待値は弱めていない）:

- task-core の `worker_decisions_are_validated_and_self_is_resolved`・`worker_decisions_beyond_the_cap_are_bundled`・
  `rejected_worker_decisions_become_one_self_decision`: `prepare_worker_decisions` の入力の型が変わったので、
  呼び出しの引数を試験の helper `field(&items)`（要素の配列の欄）で包んだ。確かめる値は変えていない。
- `worker_decisions_are_validated_and_self_is_resolved` の末尾: `worker_decisions_from_json(..).len()` の 0 件・1 件の確かめを、
  無い・`null`・空の配列・JSON でない本文が `None`、配列は `Some(配列)` の確かめに書き換えた（ADR の「試験の変更」の予定どおり）。
- task-core の新しい試験の helper `batch_from_result` は、試験を書いた commit では修正前の API（`&worker_decisions_from_json(text)` を
  そのまま渡す）を使い、修正の commit で新しい戻り値（`None` なら空の batch）に合わせた。試験の本文の確かめは同じ。

## before / after の要点

- 修正前（commit「decision-failclosed fix: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」の tree）:
  `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision_fail_closed/)'` → `fix/before.log`、exit=100、
  3 tests run: 0 passed, 3 failed。
  - task-core: `tests.rs:465` の assertion「a non-array decisions field (object) must reach a human: WorkerDecisionBatch { accepted: [],
    rejected: [], bundled: [], escalated: None }」。
  - leaf: `tree_decisions.rs:924` の `assert!(blocked_by_decision(&units, "a"))` が落ちた（a は `status: Done`、integrate-s1 も Done）。
  - atomic: `tree_decisions.rs:1024` の `assert_eq!` が落ちた（節点の status が left: Done、right: Ready）。
  - 行番号はその commit の file の行（修正の commit で `cargo fmt` が試験の数行を折り返したので、HEAD では少しずれる）。
- 修正後（提出時の HEAD）: 同じ command を `NEXTEST_STATUS_LEVEL=pass` で走らせて `fix/after.log`、exit=0、3 tests run: 3 passed
  （3 件の名前の PASS の行がある）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

2 回目の run（attempt 1）は実装をし直さず、3 件の commit（「decision-failclosed fix: decision_fail_closed_* の試験を先に書き、
修正前に落ちる before.log を取る」「decision-failclosed fix: 配列でない decisions を受け付けなかった要求 1 件にし、要求を出した
仕事を止める」「decision-failclosed fix: fix.md に変えた関数・試験・before/after と残る 2 経路の replan の要求を書く」）が
`git log` で HEAD にあることを確かめてから、次の検査を走らせた。

- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/) | test(/decision_fail_closed/) |
  test(/worker_decisions/) | test(/worker_declared/)'` → exit 0、11 tests run: 11 passed
  （`rejected_worker_decision_*`・`worker_decisions_*`・`worker_declared_decisions`・`decision_fail_closed_*` の 3 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0。repo_health_gate: all checks passed、web の tsc -b と unit（vitest 101 files・
  705 passed）、test-ts-verify 20 passed、nextest 14 crate で 8523 passed・22 skipped、doc test 通過、script bundle の SKIP 0 件。
  末尾に scratch の古い trash dir を消せない `rm: Permission denied` が 3 行出たが、exit code は 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（外部の crate `deno_core` の deprecated の warning 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は decision.rs・decision/tests.rs・tree_units.rs・tree_decisions.rs と
  この dir の fix.md・fix/before.log・fix/after.log だけ
- 既存の試験の期待値: `git diff` で、`rejected_worker_decision*`・`worker_decisions*` の確かめは引数を `field(..)` で包んだだけで、
  `worker_decisions_from_json` の `.len()` の確かめは `None`（4 通り）と `Some(配列)` の一致に変わった（弱めていない）。
  `tree_decisions.rs` の削除行は 0 行。

## LoC の差分（`git diff --numstat "$CELERIS_WU_BASE" HEAD`、追加/削除）

- production: `crates/task-core/src/decision.rs` +50/−14、`crates/task-dispatch/src/dispatcher/tree_units.rs` +6/−5（計 +56/−19、純増 37 行。
  説明の comment を含む）
- test: `crates/task-core/src/decision/tests.rs` +105/−12、`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs` +176/−0
  （計 +281/−12）
- この task は行数の純減を目的にしていない。

## 未解決

survey の to-fix・提案の残り 3 経路は、replan で同じ段に足された unit が直す（この unit は `plan_issue` を返さない）。

- `continuable-end`（yield・予算切れ・クラスタ job の wait で終わった run の `decisions`）と `failed-end`（checks の不合格・worker の
  失敗で終わった run の `decisions`）: settle-leaf が leaf の WU の run の `decisions` を ADR D1 の規則で読み、`self` の要求なら失敗の
  処理より先に決定で止める。settle-atomic が atomic の節点の run を同じ規則で読む。
- `question-and-decisions`（質問で止まる run の `decisions` の中身が人に届かない）: settle-failure が、失敗・`plan_issue`・質問・
  replan の使い切りで終わった leaf の run でも決定が先に止めることを試験で確かめる。
- owner は `run_settlement`（`crates/task-dispatch/src/run_settlement/work_unit.rs`・`crates/task-dispatch/src/run_settlement/task_run.rs`）と
  `crates/task-dispatch/src/dispatcher/worker_finish.rs`（高 risk の path class `worker-finish`）。`decision.rs`・`tree_units.rs` 側は
  変えなくてよい（`Dispatcher::worker_decisions` をそのまま呼べる）。

## 提案

- 無し。前の run が書いた replan の案（`run_settlement`・`worker_finish.rs` を範囲に入れた unit を足す）は、計画の unit
  settle-leaf・settle-failure・settle-atomic として入った。
