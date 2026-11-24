# ADR: planner・side review・CoS の run の供給側の失敗を試行に数えず、account を cooldown にして別の account で再試行する

- Date: 2026-11-24
- Status: Accepted
- 決定者: 人の依頼（2026-11-24 20:2xZ。外部の運用者が起票した task 01M87M20BAB5C21EVARQ7BWCCN）
- 関連: [ADR 2026-10-17 認証失敗のアカウント](2026-10-17-account-auth-failure.md)、ADR-0010 D5 / ADR-0011（供給側失敗の requeue）、
  ADR-0024 D4（アカウントの cooldown）、ADR-0054 D2（reviewer の失敗回数）、ADR-0072 D14 / ADR-0079 D7（planner の試行と `plan_invalid`）、
  [ADR 2026-11-07 health review 統合](2026-11-07-health-review-integration.md) D3.5（設計 review）、
  [ADR 2026-11-06 CoS run の起動の会計](2026-11-06-cos-run-launch-accounting-and-drain.md)、
  [破壊的変更を是とする](2026-10-11-breaking-changes-by-default.md)

## 結論

planner の run が供給側の失敗（認証の失敗・枠切れ・throttle）で終わったら、計画の試行に数えず、worker の run と同じ
`Dispatcher::record_account_failure` で account を cooldown にして、次の planner run を別の account に回す。
設計 review などの side review の run と CoS の chat run にも同じ穴があるので、同じ規則に揃える。
CoS の「no route_defaults candidates are declared for harness claude_code」は daemon の再起動ではなく、
CoS の代替経路が account ではなく候補列の番号で進むことと、CoS が account を cooldown にしないことが原因の不具合で、直す。

## Context（事象）

2026-11-24、本番 release の昇格（18:05:38Z に promote による再起動が始まった）の前後で次が起きた。

1. account `claude_max_lab`（`claude_oauth`、team 契約）のログインが 18:00Z に期限切れになった（credentials の
   `claudeAiOauth.loginEnded = "time_limit"`、`expiresAt = 0`。更新できない種類）。
2. task 01M7XQA74FP70G3T3KSTP02S0W の planner run が 18:05:12Z（01M87BQDFTZDDF7WD6BVS43SYW）と 18:05:23Z
   （01M87BQQRBPNEH9N5R5F74PZP3）の 2 回とも `claude_max_lab` に回り、claude は
   "Failed to authenticate: this login reached its time limit and cannot be renewed …" を返した。adapter は
   `AdapterError::AuthFailed` に分類し、planner の outcome は
   "error(retryable=true): invalid execution plan: planner run did not finish cleanly: infra error: provider auth failed: …" になった。
3. 2 回で計画の試行の上限（`MAX_PLANNER_ATTEMPTS = 2`）に達し、`plan_invalid` の決定 01M87BQTRE2NFXT7MZCG6SGGPZ が人に出た。
   `GET /api/v1/llm/accounts` の `claude_max_lab` は人が 20:28Z に再ログインするまで `cooldown = null`・`excluded_reason = null`・
   `logged_in = true` のままだった。
4. 同じ時刻の CoS の受信箱の一次対応の run（chat_runs 01M87BQVH0SWEA0875FRGZG14T）も `claude_max_lab` で起動して失敗し、
   reason は "no route_defaults candidates are declared for harness claude_code" だった。`claude_max_personal` は使える状態だった。

### コードで確かめた原因

- **planner:** `worker_finish.rs` は current WU が無い planner run を `on_planner_finished` へ早く返す。worker の run の
  `failure_reason` → `record_account_failure`（`worker_finish.rs` の後半）を通らないので、account は cooldown にならない。
  `on_planner_finished` は `design_gate::run_end_of` で `Err(AdapterError)` を "infra error: …" の文にし、
  `plan_file_checked` が偽なので "planner run did not finish cleanly" の不正な試行として扱う。試行は
  `planner_attempts_in_window`（窓の中の `WorkerStarted{role: Planner}` の数。comment の割り込みだけを除く）で数えるので、
  認証の失敗も 1 回に数えられる。cooldown が無いので 2 回目も同じ account が選ばれた。
- **side review（設計 review・親の途中評価）:** `design_gate::on_side_review_finished` も `record_account_failure` を呼ばない。
  reviewer run が供給側の失敗で終わると `review.json` が無く、`record_design_verdict` は `Err` を不合格として数える
  （`consecutive_design_review_failures`）。planner と同じ穴がある。
- **task の final review:** `review_verdict.rs` は `pf.outcome` が分類できた供給側の失敗なら `record_account_failure` を呼び、
  AuthFailed は失敗回数に数えない（ADR 2026-10-17 D3）。穴は無い。
- **worker:** `run_settlement::settle_adapter_failure` が requeue し（AuthFailed は上限に数えない）、`worker_finish.rs` が
  `record_account_failure` を呼ぶ。穴は無い。
- **CoS:** 下の D5。

## Decision

D1. **分類（供給側の失敗とは何か）:** run の結果が `Err(AdapterError)` で、既存の
    `route_failure::provider_failure_outcome` が `Some` を返すもの（`Throttled`・`Exhausted`・`AuthFailed`、起動の失敗 `Spawn` は
    `Exhausted`）を供給側の失敗とする。planner・side review・CoS もこの同じ関数で分類し、文字列から分類し直す経路を足さない。
    run が `Ok(RunOutcome)` で終わったもの（`Terminal::Error`・question・yielded・turn 以外の budget・計画の検証の失敗）は
    従来どおり供給側ではない。

D2. **planner の数え方:**
    - 供給側の失敗で終わった planner run は、`WorkerFinished`（role planner）の `end` を
      `RunEnd::HarnessError { class: Supply }` にして記録する（reviewer の `set_worker_finished_end` と同じ値）。quota の event は従来どおり添える。
    - `planner_attempts_in_window` は、`end` が `HarnessError { class: Supply }` の planner run を数えない（comment の割り込みを
      除くのと同じ形。真実は events で、新しい列・カウンタを置かない）。
    - `AuthFailed` は回数によらず数えない。`Throttled`・`Exhausted` は、続けて供給側の失敗で終わった planner run が
      `max_requeues` 未満の間は数えず、上限に達したら通常の不正な試行として数える（worker の `settle_adapter_failure` の
      `below_requeue_limit` と同じ規則。枠が戻らないまま planner を回し続けない）。
    - 次の planner run は既存の再試行の経路（`planner_retry_pending`）で起こす。route の選択は cooldown 中の account を
      admission で外すので、別の使える account に回る。`plan_invalid` の決定は、数えた試行が `MAX_PLANNER_ATTEMPTS` に
      達したときだけ出る。

D3. **account の cooldown の選択と理由:** 既存の owner `Dispatcher::record_account_failure`（`route_failure.rs`。pool の account）と
    `record_source_failure`（pool を使わない source）をそのまま呼ぶ。worker の run と同じ関数なので、
    - AuthFailed は `AUTH_FAILED_HOLD_SECS`（実質無期限）保持し、再ログイン（`record_account_login`）か check の成功だけが戻す。
    - AuthFailed になった遷移で 1 回だけ `notify_account_auth_failed` が bad_news（受信箱・Discord）を出す。
    - `GET /api/v1/llm/accounts` の `cooldown.reason = auth_failed` と除外理由で、GUI に「再ログインが必要」が出る（ADR 2026-10-17）。
    - Throttled・Exhausted は `account_cooldown_for_failure`（観測値か `fallback_cooldown_secs`、`retry_after`）に従う。

    退けた案:
    - planner だけの再試行の回数を足し、cooldown を付けない: 同じ account が選ばれ続ける（今回の事象そのもの）。
    - planner 用の別の「供給の失敗」状態（表・欄）を置く: account の状態の正本が `AccountBook` と 2 つになる。
    - 失敗のたびに account の check（`POST .../check`）を走らせて判定する: 追加の claude の起動が要り、判定が遅れる。
      run の adapter が既に `AuthFailed` と分類しているので要らない。

D4. **side review（設計 review・親の途中評価）:** `on_side_review_finished` で結果が供給側の失敗なら、D3 と同じく account を
    cooldown にし、`WorkerFinished` の `end` を `HarnessError { class: Supply }` にして、`settle_design_review`・
    `parent_evaluation::settle` に流さない（`DesignReviewed{pass:false}` を追記しない。不合格の回数に数えない）。
    side review の run は既存の起動の経路でもう一度起こす。起こし直しの経路が無い場合は、実装の WorkUnit が
    進捗ファイルに書き、同じ task で足す。
    task の final review と worker は既に正しいので変えない。eval trial・report の run の結果の扱いはこの task では
    確かめていない（未確認。進捗ファイルの未解決に書く）。

D5. **CoS の代替経路（不具合。同じ task で直す）:**
    - 原因 1: `cos_chat/launch.rs` は供給側の失敗で `RouteRetry { next_route: route_index + 1 }` を積み、
      `retry_routes` → `launch_routes(first = next_route)` → `select_cos_route_from(start)` が `cos.route_candidates` を
      `skip(start)` する。本番の `cos.route_candidates` は `{ source = "claude_oauth", model = "claude-opus-5-5" }` の 1 件だけなので、
      2 回目は空の列になり、`route_select_one` が `RouteFailure::NoDeclaredDefault("claude_code")` を返す。これが
      "no route_defaults candidates are declared for harness claude_code" の正体で、設定が読めなかったのではない。
      代替経路は候補（source × model）の番号で進み、同じ source の別の account（`claude_max_personal`）を試さない。
    - 原因 2: CoS の run の供給側の失敗は `record_account_failure` を呼ばない。番号で進まなくても、同じ account が再び選ばれる。
    - 再起動の影響は無い: chat run は 18:05:27.140Z に起動し 18:05:34.230Z に失敗で閉じた。promote の再起動（18:05:38Z）より前で、
      run の `selection_metadata.candidates` に設定の 1 件が載っている。
    - 直し方: 供給側の失敗の分類（`ProviderOutcome`）と失敗した `(adapter, account)`（または source）を `RouteRetry` に載せ、
      `retry_routes`（`Dispatcher` を持つ）が D3 の owner で cooldown にする。そのうえで `cos.route_candidates` の全体から
      選び直す（admission が cooldown 中の account を外し、同じ source の次の account、次の候補の順に進む）。
      `RouteRetry.next_route` と `select_cos_route_from` の `skip(start)` は消す（番号で進む旧い仕組みを残さない）。
      終わりは、失敗した組が cooldown の間 admission で外れることと、既存の `max_wall_secs` で保証する。
      候補を使い切ったときの理由は `NoEligible`（除外理由つき）になり、誤解を招く `NoDeclaredDefault` は出ない。
    - 付随: `retry_routes` の `retry.next_route == 0` の分岐は `next_route ≥ 1` なので通らず、「次の代替経路で再試行します」が
      一度も出ない（実際は「利用可能な経路がなくなりました」が出た）。選び直す形に変えるときに文面を「再試行します」に揃える。

D6. **再起動の扱い:** `AccountBook` は起動時に `<account root>/.celeris-usage.json` から読み（`RoutingRuntime` の構築で
    `AccountBook::load`）、`record_account_failure` は `set_cooldown` の後に `save`（一時 file に書いて rename）する。
    したがって AuthFailed の cooldown は daemon の再起動で消えない。保存に失敗した（warn だけ）、または file が無い・壊れた
    場合でも、再起動の後の最初の供給側の失敗で D2・D3 が再び cooldown にするので、2 回目の run は別の account に回り、
    試行には数えない。どちらも試験で示す（D7）。

D7. **試験（実装の WorkUnit が足す。fake adapter と注入した時計で決定的に）:**
    - planner（WorkUnit planner-supply）:
      1. account A の planner run が `AdapterError::AuthFailed("Failed to authenticate: this login reached its time limit …")` で終わる →
         `planner_attempts_in_window` が 0、`plan_invalid` の決定が無い、A の cooldown が `AuthFailed`（until ≥ now + `AUTH_FAILED_HOLD_SECS`）、
         bad_news の通知が 1 件、次の planner run が account B に回る。
      2. A・B とも AuthFailed → 次の planner の dispatch は route の選択で断られ、`plan_invalid` は出ず、人には account ごとの
         「再ログインが必要」の bad_news と route の拒否の記録が残る。A の再ログイン（`record_account_login`）の後は A に回る。
         待つ間に task に何が出るかは実装の WorkUnit が既存の挙動を確かめて試験に書く（この ADR の時点では未確認）。
      3. `Throttled { retry_after }`・`Exhausted` → `max_requeues` 未満では数えず、cooldown は `account_cooldown_for_failure` の期限、
         上限で不正な試行に数える。
      4. 供給側でない終わり方（`Terminal::Error` など）は従来どおり数える（既存の試験を残す）。
      5. 設計 review の run が AuthFailed → `DesignReviewed{pass:false}` が無く、account が cooldown になる。
    - CoS（WorkUnit cos-route）:
      6. `cos.route_candidates` が 1 件・同じ source に account A・B → A の run が認証の失敗 → A が AuthFailed の cooldown、
         同じ chat run の再試行が B で起動する。
      7. A・B とも失敗 → run は `NoEligible` の理由で閉じ、`NoDeclaredDefault` の文は出ない。
    - 再起動（WorkUnit restart-close）:
      8. A を AuthFailed にして `save` → 新しい `AccountBook::load` → A は保持されたまま、route の選択は B を選ぶ。
      9. book の file が無い状態で起動 → A の最初の失敗で cooldown になり、2 回目は B に回り、試行に数えない。

## Consequences

- 認証の切れた account は、どの role の run で見つかっても 1 回目で routing から外れ、人に「再ログインが必要」が届く。
  planner の `plan_invalid` と設計 review の不合格は、計画・設計の中身の失敗だけを数える。
- 撤去: CoS の `RouteRetry.next_route` と `select_cos_route_from` の番号での読み飛ばし、`retry_routes` の通らない分岐。
- 高 risk の path class: `design_gate.rs`・`design_verdict.rs`（review-gate）と `worker_finish.rs`（worker-finish）に触れる場合は、
  この ADR を設計 brief とする。完了の判定（`completion-not-self-reported`）と予算（`budget-unknown-cost-stops`。quota の event は
  供給側の失敗でも従来どおり記録する）は変えない。
