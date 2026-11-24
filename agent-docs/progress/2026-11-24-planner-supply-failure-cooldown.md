---
title: planner の run の認証の失敗を計画の不正と数えず、account を cooldown にして別の account で再試行する
tasks: [01M87M20BAB5C21EVARQ7BWCCN]
status: running
updated: 2026-11-24
---
# planner の run の認証の失敗を計画の不正と数えず、account を cooldown にして別の account で再試行する

状態: running。WorkUnit adr-cos（ADR と CoS の失敗の原因の調査）が済んだ。コードは変えていない。
実装は WorkUnit planner-supply・cos-route・restart-close が行う。基準は `$CELERIS_WU_BASE`（WorkUnit の base）。

設計の判断は [ADR 2026-11-24-planner-supply-failure-cooldown](../adr/2026-11-24-planner-supply-failure-cooldown.md) に書いた。

## commit（件名）

- adr: planner・side review・CoS の run の供給側の失敗を試行に数えず account を cooldown にする（CoS の失敗の原因を含む）
- progress: ADR と CoS の失敗の原因の WorkUnit の検査の結果

## CoS の仕分けの run の失敗の原因（chat_runs 01M87BQVH0SWEA0875FRGZG14T）

結論: daemon の再起動の影響ではなく、CoS の代替経路の不具合。同じ task の WorkUnit cos-route で直す。

確かめたこと（本番 DB は `sqlite3 "file:…?mode=ro"` で読んだだけ。config と account の台帳も読んだだけ）:

1. chat run の行: `launched_at = 2026-11-24T18:05:27.140Z`、`finished_at = 18:05:34.230Z`、`state = failed`、
   `reason = "no route_defaults candidates are declared for harness claude_code"`。`resolved_config_json` の
   `selection_metadata.candidates` は `[{source: claude_oauth, model: claude-opus-5-5}]`、`decision.account = claude_max_lab`。
   promote の再起動（18:05:38Z）より前に起動から失敗まで終わっており、設定の候補列も読めている。
2. chat_events の順: 18:05:26.944Z に経路 `claude_code / claude_oauth / claude_max_lab / claude-opus-5-5` で起動 →
   18:05:34.226Z に「CoS の利用可能な経路がなくなりました。理由: [redacted]」（`retry_routes` の通知。認証の失敗の文は redact された）→
   同時刻に「CoS route unavailable: no route_defaults candidates are declared for harness claude_code」→ run が failed。
3. 本番の `[cos] route_candidates` は `{ source = "claude_oauth", model = "claude-opus-5-5" }` の 1 件だけ
   （`claude_max_lab` と `claude_max_personal` は同じ source `claude_oauth` の account）。

コードの経路（`crates/task-dispatch/src/dispatcher/cos_chat/`）:

- `launch.rs`: run が失敗し `routes::supply_failure(reason)` が真なら `RouteRetry { next_route: route_index + 1 }` を積む。
- `routes.rs` の `retry_routes` → `launch_routes(first = next_route)` → `route_select.rs` の
  `select_cos_route_from(input, start)` が `cos.route_candidates` を `skip(start)` する。候補が 1 件なので 2 回目は空の列になり、
  `route_select_one` が `RouteFailure::NoDeclaredDefault("claude_code")` を返す。その `reason()` が上の文。
- 代替経路は候補（source × model）の番号で進み、同じ source の別の account を試さない。
- CoS の run の供給側の失敗は `record_account_failure` を呼ばないので、`claude_max_lab` は cooldown にならない
  （番号で進まなくても同じ account が選ばれる）。
- 付随: `retry_routes` の `retry.next_route == 0` の分岐は、`next_route` が常に 1 以上なので通らない。
  「次の代替経路で再試行します」は一度も出ず、代替経路が残っていても「利用可能な経路がなくなりました」が出る。

不具合かどうか: 不具合。CoS の代替経路（ADR 2026-11-06 の「起動済みの run の代替経路」）が、account の pool を持つ source で働かない。

修正方針（ADR D5。WorkUnit cos-route が実装する）:

- `RouteRetry` に供給側の失敗の分類（`ProviderOutcome`）と失敗した `(adapter, account)` を載せ、`retry_routes` が
  `Dispatcher::record_account_failure`（pool を使わない source は `record_source_failure`）で cooldown にする。
- 再試行は `cos.route_candidates` の全体から選び直す。admission が cooldown 中の account を外す。
- `RouteRetry.next_route` と `select_cos_route_from` の `skip(start)` を消す。通らない分岐の文面を揃える。
- 試験: 候補 1 件・account 2 つで、1 つ目の認証の失敗の後に同じ chat run が 2 つ目で起動する。両方失敗なら `NoEligible` の理由で閉じる。

## planner と他の role の調査の結果（ADR の Context に詳細）

- planner: `worker_finish.rs` が planner run を `on_planner_finished` へ早く返すので、worker の run の
  `record_account_failure` を通らない。`planner_attempts_in_window` は `WorkerStarted{role: Planner}` を数えるので、
  認証の失敗も試行に数える。今回の 2 回の失敗と `plan_invalid` はこれで説明できる。
- 設計 review・親の途中評価（side review）: `on_side_review_finished` も cooldown を付けず、`review.json` が無いことを
  不合格として数える。同じ穴。
- task の final review（`review_verdict.rs`）と worker（`run_settlement.rs`・`worker_finish.rs`）: 既に cooldown にし、
  AuthFailed は回数に数えない。穴は無い。
- 再起動: `AccountBook` は `.celeris-usage.json` から読み、cooldown の設定ごとに保存する。AuthFailed の cooldown は
  再起動で消えない。今回は cooldown が一度も付かなかったので、再起動とは関係しない。
  本番の `claude-accounts/.celeris-usage.json` は 20:34 の時点で両 account とも `cooldown = None`・`last_check.result = ok`
  （人の再ログインの後）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

この WorkUnit は文書（ADR と進捗ファイル）だけを足した。Rust のコードは変えていない。

- `git diff --name-only $CELERIS_WU_BASE HEAD` → ADR と この進捗ファイルの 2 file だけ（`sh "$CELERIS_WU_SCOPE_PATHS"` も同じ 2 file）。
- `bash scripts/dev/test-affected.sh` → exit 0（「試験対象の変更なし（docs のみ等）」。repo-health は verdict=pass、new=0・worsened=0、source-size は doc_only で対象外）。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）。
- `cargo fmt --all -- --check` → exit 0（差分なし）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（ok）。

## 未解決

- 使える account が無いときに、planner の task に何が出るか（route の拒否が worker progress に残るだけか）は未確認。
  WorkUnit planner-supply が既存の挙動を確かめて試験に書く（ADR D7 の 2）。
- side review の run を供給側の失敗の後に起こし直す経路があるかは未確認（ADR D4）。
- eval trial（`dispatcher/eval_trials.rs`）と report（`reports.rs`）の run の供給側の失敗の扱いは確かめていない。

## 提案

- eval trial・report の run の供給側の失敗の扱いを、この ADR の D1〜D3 と同じ規則で確かめる task を別に起こす（穴があれば）。
