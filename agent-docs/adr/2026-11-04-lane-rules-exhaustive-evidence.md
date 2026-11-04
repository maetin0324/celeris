---
tasks: [01M6J81DZ7C8SMY7HNZTW8GT0Z]
---
# ADR 2026-11-04: lane_rules の全入力を本番の daemon で流し、その証拠で昇格の前提を 1000 件から 100 件にする

- 状態: 採用（設計）。実装は同じ task の WorkUnit `evidence`・`store`・`runner`・`api`・`report`。
- 関係: [TS 制御層の境界 ADR](2026-11-02-ts-control-layer-boundary.md) の (f)・付記 S2-6・S2-8・S5-1・S5-2・S5-4・S5-6・S5-7・S5-8、
  [管理 CLI と観測](2026-11-03-ts-control-ops-cli-observe.md)、[方針改善の分離](2026-11-03-ts-intended-behavior-change.md)、
  [CoS の操作の registry](2026-10-09-cos-operations-all-mutations.md)、[破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。

## 結論
- 人が 2026-11-04 に「本番でも網羅を流す＋100 件」と決めた。これは (f) を**評価の開始後に変える**決定である。(f) の元の行は消さず、境界 ADR の付記で改める（D1）。
- 実行中の daemon が、`compare`・`canary` にある `lane_rules` の generation の isolate へ全 19,683 通りを正順と逆順で流し、`match_rule` と比べる（合計 39,366 呼び出し）。
- 結果は表 `script_exhaustive_runs` に残す。live の `script_guard_state`・`script_compare_log`・`events` には入れない。
- 証拠が合格なら、`lane_rules` の `compare→canary` と `canary→ts` の前提は、説明のつかない不一致の無い連続 100 件で、窓の中の timeout・失敗が 0 件になる。証拠が無い・不合格なら 1000 件のまま。`route_priority`・`cos_digest` は変えない。
- 不一致・安全違反が 1 件でも出たら証拠は不合格で、(f) の live の戻しと同じ扱いにする。
- 起こせるのは人と外部 operator の token だけで、CoS の operations では EXCLUDED にする。昇格は今までどおり人の token だけ。

## 背景
- 本番の `lane_rules` の呼び出しは約 48 件/日である。連続 1000 件まで約 21 日かかる。`canary→ts` は TS 側の単位だけを数えるので、`canary_bps` 1000（10%）なら約 208 日かかる。
- 入力は `TaskFeatures` の 9 軸×3 段階で有限（`model_policy::all_features` の 19,683 通り）である。release gate の `script_compare_real_host_matches_rust_on_all_fixtures` は release の bundle で全件を比べている。ただしこの試験は本番の daemon の中の isolate を通らず、deadline も 50 ms ではない（`LONG_DEADLINE`）。
- したがって本番の 1000 件で新しく分かるのは、主に本番の isolate での timeout・失敗の率と、本物の task からの入力の組み立てである。前者は網羅の操作で大量に測れる。後者は入力の網羅では確かめられないので、live の 100 件を残す。

## 決定

### D1 (f) の改め方
- 境界 ADR の (f) の表と「呼び出しが 1000 件に届かない領域は…」の行は消さない。表の直後に、この決定へ向かう 1 行だけを足し、末尾に付記を足す。
- 付記に書くこと: 変えた日（2026-11-04）、評価の開始後に変えたこと、決めたのは人であること、理由（入力が有限で、本番の isolate で全件を確かめるため）、変えた範囲。
- 変えるのは「進む条件」の `lane_rules` の件数と、窓の timeout・失敗の条件だけである。次は変えない。
  - (f) の「即 `rust` へ戻す条件」の列。
  - `route_priority`（1000 件）・`cos_digest`（200 件）の条件。
  - 付記 S5-4 の権限（`→canary`・`→ts` は人の token だけ）。
  - 付記 S6-3 の二重実装を整理する終了条件（純粋判定 1 万件以上、CoS の経路 2,000 件以上）。S6-3 は「進む条件の 10 倍」と説明しているが、1 万件という数は 1000 件のときの値のまま固定する。

### D2 網羅の操作
- **対象**: `script_domain_modes` の `lane_rules` の行が mode `compare` か `canary` で指す generation の、`GenerationRegistry` にある isolate そのもの。`ScriptGenerations::pinned` が返す口（`ScriptPolicy`）を live と共用する。mode `rust`・`ts` では始められない（`ts` には進む先が無い）。
- **入力**: `task_core::model_policy::all_features()`。repo の fixture file は読まない。入力の DTO は live と同じ `LaneRulesInput::redacted`。
- **参照**: 入力ごとに `match_rule` を呼び、`LaneRulesOutput::from` で比べる形にする。分類は live と同じ `classify_lane_rules` を使い、天井は `LaneCeiling::default()`（fixture の試験と同じ。制限なし）にする。task ごとの天井の丸めは live の 100 件の範囲である。
- **順序**: 正順の全件の次に逆順の全件を流す。同じ isolate に同じ順で流すことで、global に値を残す bundle を見つける（付記 S2-7）。
- **deadline**: live と同じ `SCRIPT_CALL_DEADLINE`（50 ms）。`call_lane_rules` で呼ぶ。緩めない。
- **定数**（`task_core::script_exhaustive`）: `LANE_RULES_EXHAUSTIVE_INPUTS = 19_683`、`EXHAUSTIVE_PASSES = 2`、`LANE_RULES_EXHAUSTIVE_CALLS = 39_366`。
- **参照の hash**: 記録に `reference_sha256`（`model_policy::lane_fixture_jsonl()` の byte 列の sha256、`lane_rules_reference_sha256()`）を残す。bundle が同じでも、release で Rust の規則表が変われば証拠は比べた相手を失うので、今の binary の値と違う記録は証拠に数えない（D4）。
- **`explained` は無い**: `lane_rules` の改善仕様は承認できない（ADR 2026-11-03-ts-intended-behavior-change）。分類が `Explained` を返しても不一致に数える。

### D3 tick を止めない方法
- **tick の外で走らせる**: `ScriptGenerations` が tokio の task を 1 本起こす（`crates/celeris/src/daemon/script_exhaustive.rs`）。dispatcher・`tick_loop`・`ScriptTickWait` は変えず、runner はこれらの予算を使わない。V8 は専用 thread の actor で動くので、runner は oneshot を待つだけで tokio の worker を占めない。`ScriptGenerations.ops` の lock は持たない。
- **1 件ずつ**: runner の in-flight は常に 1 件以下である。
- **live を先に通す**: 各呼び出しの前に `registry.get(sha).in_flight` を読む。0 でなければ（live・`cos_digest` の pass が走っている）呼ばずに間隔ぶん待ってから読み直す。
- **間隔を空ける**: 1 件ごとに `EXHAUSTIVE_CALL_GAP`（5 ms）を `tokio::time::sleep` で空ける。全体の時間は 1 呼び出しを (f) の目標の p50 1 ms とすると約 4 分である（見積もり。本番の値は未確認で、証拠の `started_at`・`finished_at` で分かる）。
- **Busy にならない理由**: generation の in-flight の上限は 64（`MAX_IN_FLIGHT_PER_GENERATION`）で、dispatcher の compare は最大 32（`MAX_IN_FLIGHT_COMPARES`）である。runner が足すのは 1 件なので上限に届かない。dispatcher の `unavailable` は isolate が無い・drain 中・dispatcher 自身の in-flight の上限で起きるので、runner は増やさない。
- **live の待ち**: live の要求は actor の queue で runner の 1 件を待つことがある。ふつうは 1 呼び出し分（(f) の目標で p99 5 ms）で、deadline の 50 ms に収まる。
- **timeout の影響を限る**: runner の呼び出しが deadline を超えると isolate は作り直され、同じ generation の未完の要求は `GenerationGone` になる（境界 ADR「terminate 後の isolate」）。この時に重なった live の要求は失敗に数えられ、D6 の「失敗 0 件」を満たさなくなる。これは fail-closed の向きなので計数を消さない。次の 3 つで機会を減らす。
  - timeout の後は `EXHAUSTIVE_TIMEOUT_BACKOFF`（1 秒）待つ。
  - timeout か失敗が連続 3 件で打ち切る（`consecutive_errors`）。
  - timeout か失敗の件数が全呼び出し数の 0.1% を超えると決まった時点で打ち切る（`rate_exceeded`）。
- **`GenerationGone` の分け方**: `registry.get` が無し・drain 中なら generation が終わったので `aborted`（`generation_gone`）にする。isolate がまだ在るなら、live の timeout による作り直しに巻き込まれただけなので失敗に数えて続ける。`Busy` は (f) のとおり失敗に数える。
- **DB の書き込み**: 開始・`EXHAUSTIVE_PROGRESS_EVERY`（1000 呼び出し）ごとの進捗・終了だけを、`spawn_blocking` で短い transaction で書く。
- **上限**: 1 回の実行は `EXHAUSTIVE_WALL_LIMIT`（30 分）までで、超えたら `aborted`（`wall_limit`）にする。同時に走るのは領域ごとに 1 本。
- **再起動・役割**: daemon の停止で runner を止めて `aborted`（`daemon_stopped`）を書く。起動時の復元（`ScriptGenerations::restore`）で、他の instance の `running` の行を `aborted`（`daemon_restarted`）にする。途中から再開しない。standby は今の登録と同じく 503 で受けない。
- 時刻を読むのは `tokio::time` だけにし、試験は `tokio::time::pause` と遅延フックで決定的に確かめる。CPU を焼く負荷をかけない。

### D4 証拠の持ち方
**表 `script_exhaustive_runs`**（migration は次の空き番号。今は 0109）。実行 1 回につき 1 行で、再起動の後も残る。

| 欄 | 中身 |
|---|---|
| `id` | ULID |
| `domain` | 今は `lane_rules` だけ |
| `generation_id`・`bundle_sha256` | 流した generation |
| `mode_at_start` | `compare` か `canary` |
| `daemon_instance_id` | 起動ごとの ULID（`GET /health` の `instance_id`） |
| `requested_by`・`reason` | 起こした主体と理由 |
| `state` | `running`・`passed`・`failed`・`aborted` |
| `stop_code` | 途中で止めた理由。`mismatch`・`safety_violation`・`consecutive_errors`・`rate_exceeded`（`failed`）、`generation_gone`・`wall_limit`・`daemon_stopped`・`daemon_restarted`（`aborted`）。最後まで流して判定したら `null` |
| `reference_sha256` | D2 の参照の hash |
| `started_at`・`finished_at` | RFC 3339（UTC） |
| `calls`・`matches`・`mismatches`・`timeouts`・`failures`・`safety_violations` | 件数。`calls` = `matches` + `mismatches` + `timeouts` + `failures`。`safety_violations` は S2-8 のとおり別に数える |
| `uncovered_inputs` | 2 回の中で一度も `match` しなかった入力の数 |
| `latency_p50_us`・`latency_p95_us`・`latency_max_us` | 全呼び出しの所要時間。p95 は最近傍順位 |
| `first_problem_json` | 最初の不一致・安全違反・失敗の 1 件（pass・入力・Rust の出力・TS の出力の要約・code）。無ければ `null` |

- `state` が `running` でない行の `UPDATE`・`DELETE` は trigger で拒む。領域ごとの `running` は部分 UNIQUE index で 1 行に限る。終了の書き込みは `WHERE state='running'` の CAS で、起動時に `aborted` にされた行を古い daemon が上書きしない。
- **型**（`crates/task-core/src/script_exhaustive.rs`）: `ScriptExhaustiveRun`、`ScriptExhaustiveState`、`ScriptExhaustiveStopCode`、`ScriptExhaustiveCounts`（`verdict()` が純粋関数で合否を返す）、`NewScriptExhaustiveRun`、`ScriptExhaustiveStanding`（`NotApplicable`・`Missing`・`Passed`・`Failed`）、`exhaustive_standing(&[ScriptExhaustiveRun], reference)`。
- **合格**（`verdict`）は次の全部が成り立つとき。
  - `calls` が 39,366 で、`mismatches` と `safety_violations` が 0。
  - `uncovered_inputs` が 0。timeout・失敗の 1 件は比較をしていないので、全件 `match` は「どの入力も 2 回のどちらかで `match`」で確かめる。
  - `timeouts` と `failures` がそれぞれ `calls` の 0.1% 以下（`count * 1000 <= calls`。39 件まで）。
- **証拠の立場**（`exhaustive_standing`）: (domain, generation) の行のうち `reference_sha256` が今の binary の値と同じ行だけを見る。
  1. `mismatches` か `safety_violations` が 1 以上の行があれば `Failed`。その generation の間は消えない（同じ bundle が 2 回目に一致しても、global の状態で答えが変わる bundle を見逃さないため）。
  2. そうでなければ、終わった行（`passed`・`failed`）の最新で決める。`passed` なら `Passed`、`failed` なら `Failed`。timeout・失敗の率で落ちただけなら、もう一度流して合格できる。
  3. 終わった行が無ければ `Missing`。`running`・`aborted` は数えない。
- **無効になる場合**: 表の key が generation なので、generation が変われば行は別で、新しい generation の立場は `Missing` になる。規則表が変わった release でも `Missing` になる。

### D5 不一致・安全違反は不合格にし、live の戻しと同じ扱いにする
- 最初の 1 件で runner を止めて `failed` にする。live の (f) が 1 件で戻すのと同じ基準である。`mismatches` は 1 のまま残る（数を知りたいときは `first_problem_json` の入力から調べる）。
- `script_exhaustive_finish` が `failed` の行の書き込みと次の戻しを 1 つの transaction で行う。どれも付記 S5-7 と同じ関数（`write_mode_tx`・`invalidate_tx`）と actor `system:script-guard` を使うので、受信箱 `script_control` の項目は今の経路で作られる。
  - **不一致**: その時点で mode が同じ generation の `canary`・`ts` なら `rust` へ戻す（`reason` は `guard_exhaustive_mismatch: …`）。`compare` は挙動が Rust のままなので mode を変えない。
  - **安全違反**: mode にかかわらず generation を `invalid` にし、それを指す全領域を `rust` へ戻す（`guard_exhaustive_safety_violation: …`）。合成の入力でも、TS が天井を越える提案や許可されない capability の要求をしたことに変わりはないからである。
- `compare` で不一致が出た generation は、証拠が `Failed` なので 1000 件の条件に戻る。live の不一致が `clean_streak` を 0 に戻すだけで以後の昇格を禁じないのと同じ扱いで、新しい禁止は足さない。
- `timeout`・失敗だけで落ちた実行（`consecutive_errors`・`rate_exceeded`・率の超過）は mode を変えない。live の判定には live の窓がある。
- 戻しの書き込みに失敗したら、runner は 1 秒ごとに書き直す（付記 S5-6 と同じ向き）。daemon が先に止まれば行は `running` のまま残り、次の起動で `aborted` になる。その戻しはそこで失われるので、人が網羅を流し直して確かめる。これは受信箱に出ないが、status と report が `aborted` と示す。

### D6 readiness
- `task_core::script_guard` に `SCRIPT_GUARD_READY_PURE_EXHAUSTIVE: u64 = 100` を足す。`readiness` の引数に `exhaustive: ScriptExhaustiveStanding` を足し、`ScriptGuardReadiness` に欄 `exhaustive` を足す。
- 領域 `lane_rules` で立場が `Passed` のとき: `required` は 100。`ready` は「連続の件数 ≥ 100、`safety_violations` が 0、`timeouts_in_window` が 0、`failures_in_window` が 0」。
- それ以外（`lane_rules` の `Missing`・`Failed`、他の領域の `NotApplicable`）: 今と同じ。`required` は純粋判定 1000・CoS 200、率は 0.1% 以下。
- **両方の遷移に当てる**: `compare→canary` と `canary→ts` のどちらも同じ規則にする。連続の件数の数え方は今のまま（`compare` は `clean_streak`、`canary` は `ts_side_clean_streak`）で、窓は mode ごとに数え直す今の計数を使う。同じ generation の証拠は `canary` でも有効なので流し直さない。
  - 理由: `canary→ts` だけ 1000 件に残すと、上の約 208 日が残って今回の目的を果たせない。100 件で見るものは両方で同じである。
  - 目安（48 件/日の場合）: `compare→canary` は約 2.1 日。`canary→ts` は `canary_bps` 1000 で約 21 日。
- **gate は 1 か所**: `script_domain_set_mode` の昇格の検査が同じ transaction で立場を読み、`readiness` に渡す。不足は今の 422 `script_canary_not_ready` で、本文の `detail` に `required`・`exhaustive` が入る。`crates/celeris/src/daemon/script_guard.rs` の readiness の log も同じ値を使う。
- 自動の昇格は足さない。

### D7 起動の権限と CoS operations
- `POST /api/v1/script/domains/{domain}/exhaustive-runs`
  - 本文 `ScriptExhaustiveRunBody`（`deny_unknown_fields`）: `generation`（`bundle_sha256`、必須）、`reason`（必須）。
  - 応答: 202 `ScriptExhaustiveRunView`（実行は裏で続く）。
  - 通る主体: `human:*`・`operator:*`（`script_compare::actor_may_run_exhaustive`）。他は 403 `script_exhaustive_requires_operator`。
  - 他の拒否: 未知の領域は 404 `script_domain_not_found`、`lane_rules` 以外は 422 `script_exhaustive_domain_unsupported`、mode が `compare`・`canary` でない・`generation` が今の行と違うは 409 `script_exhaustive_mode_conflict`、走っている間は 409 `script_exhaustive_running`、全体の停止の間は 409 `script_guard_halted`、runtime が無い・standby は 503 `script_runtime_unavailable`。どの拒否でも表を変えない。
- `GET /api/v1/script/domains/{domain}/exhaustive-runs` → 200 `ScriptExhaustiveRunList`（`standing`、`reference_sha256`、新しい順の `runs`、最大 50 行）。認証された誰でも読める。
- `ScriptGenerationControl` に `fn exhaustive_start(&self, domain: ScriptDomain, bundle_sha256: &str, actor: &str, reason: &str) -> Result<ScriptExhaustiveRun, ScriptGenerationError>` を足す。既定の実装は `ScriptGenerationError::Unavailable` を返し、`ScriptGenerations` が上書きする。409・422 の拒否は `ScriptDomainError` の variant で表し、`domain_problem` が code に写す。
- `celerisctl script exhaustive run <domain> --generation <sha256> --run-reason <text>`、`celerisctl script exhaustive status <domain> [--json]`。全体の `--reason` と重ならないよう `--run-reason` にする。`status` は `cos_read_only` に入れ、`run` は `request_of` に入れる。
- **CoS operations は EXCLUDED** にする。`("POST", "/api/v1/script/domains/{domain}/exhaustive-runs", "script_api_only: …")`。CoS の run の `celerisctl script exhaustive run` は 422 `cos_operation_not_allowed`、CoS の credential で API を直接呼べば 403 になる。
  - 理由 1: この操作は mode を直接変えないが、結果が人の昇格の条件を下げ、結果によっては `rust` への戻しと `invalid` を起こす。読み取りでも計数だけの書き込みでもない。
  - 理由 2: 人の決定は「人と operator の token で起こす」であり、CoS が自分で流す必要が無い。
  - 必要になれば、その変更で ALLOWED に移す。

### D8 synthetic を live に数えない
- runner は `script_compare_append_observed`・`script_compare_log`・`script_compare_recorded`・`script_guard_observe` を呼ばない。`script_guard_state` の `calls`・`window`・`clean_streak`・`ts_side_clean_streak`・`timeouts_10min` を変えない。
- `script-compare.py` の live の件数は今の定義のまま `events` と `script_compare_log` から数え、網羅の行を足さない。report には別の節で証拠を出す。D5 の戻しだけが live の表に書く行で、これは計数ではなく mode の変更の記録である。
- ready の判定は、script-compare.py でも `readiness` と同じ規則にする。今の binary の `reference_sha256` は DB に無いので、`--lane-reference-sha256` で渡す。省略時は `control-ts/fixtures/lane_rules/all-features.jsonl` の sha256 を使う（`tests/script_fixtures.rs` が `lane_fixture_jsonl()` と同じ内容であることを固定している）。どちらも得られなければ、`exhaustive_reference_unverified` の blocker で ready にしない。

## 不採用
- **別の isolate を作って流す**: 本番の isolate での timeout・失敗を測るのが目的の一部なので、別の isolate では測りたいものが測れない。人の指定も generation の isolate そのものである。生きた generation の RSS の上限（(f)）にも当たる。
- **tick の中で少しずつ流す**: `ScriptTickWait` の 200 ms を live と取り合い、`tick_budget` の `unavailable` を増やしうる。
- **fixture file を読む**: 本番の host に repo の file がある前提になる。Rust の関数なら binary の規則と必ず揃う。
- **網羅の結果を `clean_streak` に足す**: 本物の task からの入力の組み立てを確かめないまま 1000 件を満たしてしまう。(f) の意味が変わる。
- **live の計数の窓に `timeout` の 0.1% を残す（100 件で 0.1%）**: 100 件の窓では 1 件で 1% になり、0.1% の条件は 0 件と同じである。0 件と書く。
- **`route_priority` にも広げる**: 入力に route の表を含み有限ではない。

## 後続の unit の作業範囲と試験名
| unit | 範囲 | 試験（名前を変えるなら付記に書く） |
|---|---|---|
| `evidence` | `crates/task-core/src/script_exhaustive.rs`（新規）、`crates/task-core/src/script_guard.rs`、`script_compare.rs`（`actor_may_run_exhaustive`）、readiness の呼び出し側の引数（`store` が本当の立場に替えるまで `Missing` を渡す） | `exhaustive_verdict_each_condition`、`exhaustive_standing_rules`（`Failed` が消えない・`aborted` を数えない・参照の違う行を数えない）、`readiness_lane_rules_exhaustive_required_100`、`readiness_without_evidence_stays_1000`、`readiness_failed_evidence_stays_1000`、`readiness_other_domains_unchanged`、`readiness_exhaustive_zero_timeouts_and_failures` |
| `store` | migration（`0109_script_exhaustive_runs.sql`）、`crates/task-core/src/store/script_exhaustive.rs`（`script_exhaustive_begin`・`script_exhaustive_progress`・`script_exhaustive_finish`・`script_exhaustive_abort_stale`・`script_exhaustive_runs`・`script_exhaustive_standing`）、`store/script_domains.rs` の gate、`ScriptDomainError` の variant と `crates/task-api/src/script_domains.rs` の `domain_problem`、`crates/task-api/src/script_generations.rs` の `exhaustive_start`（既定の実装まで） | `script_exhaustive_begin_checks_mode_and_generation`、`script_exhaustive_one_running_per_domain`、`script_exhaustive_terminal_rows_immutable`、`script_exhaustive_survives_reopen`、`script_exhaustive_generation_change_is_missing`、`script_exhaustive_finish_rolls_back_canary_on_mismatch`、`script_exhaustive_finish_keeps_compare_on_mismatch`、`script_exhaustive_finish_invalidates_on_safety_violation`、`script_exhaustive_does_not_touch_guard_counters`、`script_domain_promote_uses_exhaustive_standing` |
| `runner` | `crates/celeris/src/daemon/script_exhaustive.rs`（新規）、`script_layer.rs`（`exhaustive_start` の上書き）、`docs/architecture-map.md`、`crates/celeris/tests/script_exhaustive.rs`（新規） | 一時 DB の daemon と本物の bundle で `exhaustive_real_bundle_all_inputs_match_and_persist`（全 19,683×2 件が `match`、再起動の後も残る）・`exhaustive_new_generation_invalidates_evidence`・`exhaustive_always_different_bundle_fails`・`exhaustive_does_not_move_live_counters`。遅延フックと `tokio::time::pause` で `exhaustive_tick_continues_while_running`・`exhaustive_live_call_not_busy_or_unavailable`・`exhaustive_waits_for_live_in_flight`・`exhaustive_timeout_backs_off_and_stops_at_three`・`exhaustive_restart_marks_running_aborted` |
| `api` | `crates/task-api/src/script_exhaustive.rs`（新規）、`cos/ops/admin.rs` の EXCLUDED、`crates/celerisctl/src/commands/script.rs`、`docs/api` と web の生成型、`crates/task-api/tests/script_domains.rs` の昇格の試験 | `exhaustive_start_requires_human_or_operator`、`exhaustive_start_rejects_each_conflict`、`exhaustive_list_shows_standing`、`canary_promote_with_exhaustive_100`、`canary_promote_without_exhaustive_422`、`script_exhaustive_cos_read_only_and_wrapped` |
| `report` | `scripts/dev/script-compare.py`、`scripts/dev/tests/script-compare.sh`、task の進捗ファイルの「本番への移行」 | report の表示と ready（合格 100 件で ready、無し・不合格は 1000 件、参照の照合ができなければ blocker） |

`store` は `evidence` の型を使い、`runner`・`api` は `store` までの統合の後に作る。`runner` と `api` は `exhaustive_start` だけでつながるので、互いの file を変えない。

本番の daemon・DB・config・release には worker が触れない。本番で流す・昇格するのは人の手順である（ADR-0095 付記 D-d、付記 S5-4）。
