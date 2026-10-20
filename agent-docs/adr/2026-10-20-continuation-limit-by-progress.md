# ADR 2026-10-20: continuation の上限は進捗で判定する（ADR-0072 D18 の改定）

- 日付: 2026-10-20
- 状態: 採用（実装は同じ task の impl 葉）
- 関連: ADR-0072 D18（上限の元の決定）、ADR-0090 D1（クラスタ job 待ちは数えない）、ADR-0079（auto leaf の guard）、
  2026-10-11-breaking-changes-by-default、2026-10-18-parent-waits-for-children

## 背景

2026-10-20、task 01M5BTS6VK と 01M5C13NRT が、continuation 3 回の時点で
「実行が進みません（continuation 3 回 / 進捗なし 0 回）。予算を増やして続ける／分割し直す（replan）／中止のいずれかを
選んでください。」と聞いて blocked になった。どちらも commit が積み上がっていた（01M5BTS6VK は ADR・migration・判定処理の
4 commit）。運用者は 2 回とも「続ける」と答えた。進捗なし 0 回なのに「進みません」と出て、人の手を止めている。

### 現状（base 669c00ab で確認）

**config**（`crates/celeris/src/config/execution.rs`、`ExecutionTomlConfig`、`#[serde(deny_unknown_fields)]`）:

| 欄 | 既定 | 実体 |
|---|---|---|
| `continuation` | `true` | `false` なら continuation しない（`WorkerError{retryable:true}`） |
| `max_continuations_per_work_unit` | 3（`default_max_continuations_per_work_unit`） | 進捗の有無を見ない回数の上限 |
| `no_progress_limit` | 2（`default_no_progress_limit`） | 進捗なしの連続回数の上限 |

`crates/celeris/src/config/dispatch.rs:206-207` が `task_dispatch::ExecutionConfig`（`crates/task-dispatch/src/dispatcher.rs:571-576`、
`Default` は 3 / 2）へ写す。本番 `~/.config/celeris/config.toml` の `[execution]` は `parallel` と `gate` だけで、
上の 2 欄は書いていない（既定値で動いている。2026-10-20 に読み取りで確認）。

**進捗の定義**（`task_core::checkpoint_shows_progress`、`crates/task-core/src/execution.rs:499`）: 前の checkpoint が無ければ真。
あれば `files_changed` のパス集合が変わる・`repo_state.head` が変わる・`completed` が増える・`remaining` が減る、のどれかで真。
commit が 1 つ積まれれば HEAD が変わるので真になる。

**task の経路**（WU を持たない run。`crates/task-dispatch/src/dispatcher/worker_finish.rs:1077-1112`）:
- `continuations_so_far = task_ops::derive::consecutive_continuations(&events_so_far)`（`crates/task-ops/src/derive.rs:159`。
  `Transitioned{reason:"continue"}` を新しい順に数え、`dispatch`・クラスタ job の wait/resume は読み飛ばし、`answer` を含む他の
  reason で止まる。人の回答で 0 に戻る）。
- `progressed = checkpoint_shows_progress(latest_progress_checkpoint(.., None), &checkpoint)`、
  `no_progress = if progressed { 0 } else { no_progress_streak(.., None) + 1 }`（`no_progress_streak` も最後の `answer` から数える）。
- `continuations_so_far >= max_continuations_per_work_unit || no_progress >= no_progress_limit` で `Trigger::WorkerQuestion`。
  文言はどちらでも「question: 実行が進みません（continuation {n} 回 / 進捗なし {m} 回）。予算を増やして続ける／分割し直す（replan）／
  中止のいずれかを選んでください。」。**前者は進捗を見ない**ので、毎回 commit している task も既定 3 回で止まる（今回の現象）。

**WU の経路**（`crates/task-dispatch/src/execution_scheduler.rs:176-225` の `continuation()`。呼び出しは
`worker_finish.rs:726-753` で `WuLimits{max_continuations, no_progress_limit, max_retries}` を組む）:
- `wu.continuations >= limits.max_continuations || no_progress >= limits.no_progress_limit` で WU を
  `blocked(WorkUnitBlockedReason::Limit)`、`Trigger::WorkerQuestion`、文言「question: WorkUnit {key} が進みません（continuation {n} 回 /
  進捗なし {m} 回）。…」。同じく進捗を見ない。
- `wu.continuations` は `work_units.continuations` 列で、**人の回答でも 0 に戻らない**（`resume_after_answer`、
  `execution_scheduler.rs:473` は状態を `needs_continuation` に戻すだけ）。そのため「続ける」と答えても、次の continuable な終わりで
  `continuations >= 3` がまた真になり、1 run しか進まない。ADR-0072 D18 の「回答の時点から数え直す」と食い違っている。
- `blocked(limit)` は失敗として扱われ、replan の余地があれば replan に回る（`dispatcher/work_units.rs:1249` の
  「work unit {key} made no progress」、`:582` の Stuck → `replan_gate`、`execution_scheduler.rs:395` の settle、
  `task-core/src/tree/liveness.rs:282`）。回数で止まっただけの進んでいる WU も「no progress」として replan される。
- auto leaf（ADR-0079、`auto_counts.is_some()`）は `max_continuations`・`no_progress_limit` を `u32::MAX` にし、
  `TreeLimits::auto_leaf_max_continuations`（既定 2）・`auto_leaf_max_compactions` の別の guard
  （`dispatcher/auto_leaf.rs`、決定の要求）で止める。

## 決定

### D1. 進捗がある間は continuation の回数で人に聞かない

task・WU の両経路で、`checkpoint_shows_progress` が真の continuation は、回数が旧既定（3）を超えても人に聞かずに続ける。
回数の判定は D2 の固い上限だけが行う。`max_continuations_per_work_unit` による「回数だけで止める」判定は消す（D4）。

### D2. 人に聞く条件は 2 つだけ

1. **進捗なし**: `no_progress >= no_progress_limit`（欄名・既定 2・数え方〈最後の `answer` から、`Waiting` の checkpoint は除く〉は
   今のまま）。task の経路は従来どおり `Trigger::WorkerQuestion`、WU の経路は従来どおり `blocked(limit)`（replan の余地があれば
   replan、無ければ人に聞く）。
2. **固い上限**（暴走止め）: 新しい欄 `[execution] continuation_hard_limit`（`u32`、既定 **12**、`0` は `validate()` で
   `ConfigError::Invalid`）。最後の人の回答（`answer`）以降の continuation 回数が `continuation_hard_limit` に達したら、進捗が
   あっても人に聞く。12 は 1 run の予算（既定 40 turn / 2400 秒程度）× 13 run ≈ 9 時間弱で、今回の 2 件（4 回目で止まった）の 3 倍の
   余裕を取りつつ、暴走を 1 日以内に止める値。
   - 判定の順: 進捗なし（1）を先に見る。どちらにも当たらなければ continue。
   - 回答で窓を戻す: task は `consecutive_continuations` のまま（既に `answer` で止まる）。WU は `wu.continuations`（生涯の合計。
     stats・`celerisctl execution` が使うので意味を変えない）ではなく、新しい純粋関数
     `task_ops::derive::wu_continuations_since_answer(events, wu_id) -> u32`（`crates/task-ops/src/derive.rs`）で数える:
     その WU の `WorkUnitTransitioned{reason:"continue"}` を、同じ WU の `WorkUnitTransitioned{reason:"answer"}` より後だけ数える。
     `ContinuationInputs` に `continuations_since_answer: u32` を足し、`worker_finish.rs` が渡す。
   - **費用上限は足さない**: 1 run の費用は run の予算（turn・壁時計）で既に上限があり、continuation の回数の上限 × run の予算が
     task あたりの費用上限になる。tokens の上限を別に足すのは、必要になってから別 ADR で決める。
   - auto leaf は今のまま（`u32::MAX` で無効化し、auto leaf の guard が止める）。固い上限も auto leaf では `u32::MAX` にする。

### D3. 固い上限の質問文は事実と進捗を示す

「進みません」と書かない。上限に達した事実と、この窓の進捗を数で示す。

- task: `question: continuation の上限（continuation_hard_limit = {limit} 回）に達しました。進捗は続いています: 前回の回答以降
  continuation {n} 回、うち進捗あり {p} 回、commit {c} 件（{base7}..{head7}）、変更 file {f} 件。続ける（上限まで再び数える）／
  分割し直す（replan）／中止のいずれかを選んでください。`
- WU: 先頭を `question: WorkUnit {key} の continuation の上限（…）に達しました。` にし、後は同じ。
- 数の出どころ:
  - `n`: D2 の窓の continuation 回数。
  - `p`: 窓の中で進捗ありだった checkpoint の数。新しい純粋関数
    `task_ops::derive::progress_checkpoints_since_answer(events, work_unit_id: Option<&str>) -> u32`（`no_progress_streak` と同じ窓・
    同じ比較で、`progressed` を数える）。
  - `c`: `task_core::RepoState` に `#[serde(default)] pub ahead: u64` を足し、`crates/task-dispatch/src/checkpoint.rs` の
    `gather_repo_facts_from` が `task_ops::changes::changes(..).ahead`（`base..HEAD` の commit 数。既に計算している）を入れる。
    `repo_state` が無い（remote・git でない）ときは「commit 不明」と書く。
  - `f`: 今回の checkpoint の `files_changed.len()`。`base7`/`head7` は `repo_state` の先頭 7 文字。
- 進捗なし（D2-1）の文言は、事実に合うよう `question: 実行が進みません（進捗なしが {m} 回続きました。continuation {n} 回）。…`
  （WU は `WorkUnit {key} が進みません（…）`）にする。進捗なし 0 回で「進みません」と出る経路は無くなる。
- 文言の組み立ては 1 か所にする: `crates/task-dispatch/src/execution_scheduler.rs` に
  `pub fn continuation_cap_question(key: Option<&str>, limit: u32, facts: &ContinuationFacts) -> String` と
  `pub fn no_progress_question(key: Option<&str>, no_progress: u32, continuations: u32) -> String` を置き、task 経路
  （`worker_finish.rs`）と WU 経路（`continuation()`）の両方が使う。

### D4. 旧欄 `max_continuations_per_work_unit` は消す（読み込みエラー）

意味が「人に聞く回数」から「暴走止め」に変わるので、改名して旧欄を消す（ADR 2026-10-11）。

- `crates/celeris/src/config/execution.rs`: `max_continuations_per_work_unit` と `default_max_continuations_per_work_unit` を消し、
  `continuation_hard_limit`（`default_continuation_hard_limit() -> 12`）を足す。`deny_unknown_fields` により旧欄は
  `unknown field \`max_continuations_per_work_unit\`` の読み込みエラーになる。`validate()` で `continuation_hard_limit == 0` を拒否。
- `crates/celeris/src/config/dispatch.rs:206`: `continuation_hard_limit` を写す。
- `crates/task-dispatch/src/dispatcher.rs` の `ExecutionConfig`: `max_continuations_per_work_unit` を `continuation_hard_limit`
  （`Default` 12）に置き換え、doc comment を本 ADR に合わせる。
- `crates/task-dispatch/src/execution_scheduler.rs` の `WuLimits.max_continuations` を `continuation_hard_limit` に改名。
- 試験: `crates/celeris/src/config/tests.rs` に「旧欄は読み込みエラー（エラー文に欄名）」「既定 12」「0 は Invalid」。旧欄を
  使う既存試験（`dispatcher/tests/phase_effect_ab/continuation.rs`・`session_resume.rs`・`work_units.rs`）は新欄へ書き換え、
  「3 回で止まる」ことを前提にした試験は「進捗なしで止まる」か「固い上限で止まる」のどちらを見ているかで振り分け直す。
- ADR-0072 D18 の表の `max_continuations_per_work_unit` 行と、ADR-0090 の言及に「2026-10-20 に本 ADR で置換」の付記を足す
  （本文は書き換えない）。
- **本番への移行**: 本番 config は旧欄を書いていないので書き換えは不要。運用者は release 昇格前に
  `grep -n max_continuations_per_work_unit ~/.config/celeris/config.toml` が何も出さないことを確かめる（出たら行を消し、必要なら
  `continuation_hard_limit = <n>` を書く）。進捗ファイルの「本番への移行」節に書く。

### D5. WU の continuation にも同じ規則

- `execution_scheduler::continuation()` の判定を D2 に置き換える（進捗なし → `blocked(Limit)`。固い上限 → 下の新しい理由）。
- 固い上限で止めた WU は失敗ではないので replan に回さない。`WorkUnitBlockedReason` に `ContinuationCap`
  （`as_str`/`parse` は `"continuation_cap"`。`work_units.blocked_reason` は TEXT 列なので migration は要らない。
  `crates/task-core/migrations/0026_execution.sql` の列の注記は既存 migration なので書き換えない）を足し、次のとおり扱う:
  - 人の回答待ちとして `Question` と同じ扱い: `execution_scheduler.rs:372`（settle の `PhaseSettle::Question`）・`:445`、
    `dispatcher/work_units.rs:1232`（`deferred_work_unit_trigger` が outcome の質問文をそのまま出す）、`task-ops/src/inbox.rs:581`、
    `task-ops/src/gate.rs:324`、`task-api/src/inbox_notifications.rs:436`、`task-core/src/tree/liveness.rs`（replan の対象にしない。
    人の回答待ちとして説明する）。
  - 回答での再開は `Limit` と同じ: `dispatcher/work_units.rs:518` の resumable に入れ、`resume_after_answer` で
    `needs_continuation` に戻す（checkpoint の続きから）。窓は D2 の `wu_continuations_since_answer` が `answer` から数え直す。
  - replan の対象（`work_units.rs:582`・`:1249`、`execution_scheduler.rs:395`）には入れない。
  - `task-ops/src/replay.rs:351` の reason → blocked_reason の写像に `"continuation_cap"` を足す。
- WU の `decision.reason`（`WorkUnitTransitioned.reason`）は、進捗なしは従来の `"limit"`、固い上限は `"continuation_cap"`。

## 試験（impl 葉の受け入れ）

LLM・外部ネットワーク・sleep を使わない（既存の dispatcher 試験の stub worker と注入した checkpoint で決定的に作る）。

1. task 経路: 毎回 checkpoint が進捗を示す（HEAD か completed が変わる）run を旧既定 3 回を超えて（例 5 回）continuation させても
   blocked にならず `Continue` が続く。
2. task 経路: 進捗なしが `no_progress_limit` 回続くと `WorkerQuestion` で、文言が「進捗なしが {m} 回続きました」を含む。
3. task 経路: `continuation_hard_limit` を小さく（例 4）した config で、進捗ありのまま上限に達すると `WorkerQuestion`、文言が
   「上限」「continuation 4 回」「commit」を含み、「進みません」を含まない。
4. WU 経路で 1〜3 と同じ 3 件（`execution_scheduler` の単体試験と `dispatcher/tests/work_units.rs` の結合試験）。固い上限の WU は
   `blocked(continuation_cap)` で replan されず、回答後に `needs_continuation` から再開し、窓が 0 から数え直される。
5. config: 旧欄が読み込みエラー、既定 12、0 は Invalid。

## 採らなかった案

- `max_continuations_per_work_unit` の名前のまま意味だけ変える: 本番や手元の config が古い意味で書いた値を黙って別の意味に読む。
  ADR 2026-10-11 に反する。
- 固い上限でも `Limit` を使い replan に回す: 進んでいる WU を「made no progress」として計画し直すことになり、今回の誤報と同じ。
- tokens の費用上限を同時に足す: run の予算 × 回数の上限で費用は抑えられており、値の根拠が今は無い。
