# 健全性v2 R4: policy の path の追加（policy-paths.diff の説明）

operator が決めること: [policy-paths.diff](policy-paths.diff) を policy だけの branch で main に入れるか。入れた後に決定 policy-paths に答えると、
R4 の unit sync-policy が main を取り込み、own-quota・own-supervisor・own-orchestration が始まる。

## patch の中身

根拠は [ADR 2026-11-11-dispatcher-state-ownership](../../adr/2026-11-11-dispatcher-state-ownership.md) の D8。足すだけで、今の path は 1 本も消さない
（patch の削除行は 0、追加行は 43）。

`scripts/dev/repo-health/profiles/celeris.toml` の `[[review.high_risk]]`（8 行の追加）:

| class | 足す正規表現 |
|---|---|
| lease | `^crates/task-dispatch/src/run_supervisor(\.rs$\|/)`、`^crates/task-dispatch/src/run_settlement(\.rs$\|/)` |
| budget | `^crates/task-dispatch/src/routing_runtime/quota(\.rs$\|/)`、`^crates/task-dispatch/src/routing_runtime/quota_estimate(\.rs$\|/)`、`^crates/task-dispatch/src/run_settlement(\.rs$\|/)`、`^crates/task-dispatch/src/dispatch_admission(\.rs$\|/)` |

`(\.rs$|/)` は file 自身とその子 module（`run_supervisor/tests.rs` など）に当たり、`run_supervisorx.rs` のような別名には当たらない。

`agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json` の `capabilities[].owners`（7 件・35 行の追加）:

| capability | 足す owner path |
|---|---|
| run-claim-lease | `crates/task-dispatch/src/run_supervisor.rs`、`crates/task-dispatch/src/run_settlement.rs` |
| orphan-takeover | `crates/task-dispatch/src/run_supervisor.rs` |
| accounts-quota | `crates/task-dispatch/src/routing_runtime/quota.rs`、`crates/task-dispatch/src/routing_runtime/quota_estimate.rs` |
| budget-pricing | `crates/task-dispatch/src/run_settlement.rs`、`crates/task-dispatch/src/dispatch_admission.rs` |

capability-map.json は `json.dumps(…, ensure_ascii=False, indent=1)` と末尾の改行で今の bytes に戻ることを確かめてから、同じ書式で書いた。

## 確かめたこと（この WorkUnit の作業ツリーで実行）

- `git apply --check agent-docs/progress/2026-11-11-health-v2-r4/policy-paths.diff`: exit 0。
- patch の削除行（`-` で始まり `---` でない行）: 0 行。
- 当てた profile を `scripts/dev/repo-health/health/profile.py` の `validate` に通した: 例外なし。lease の class は 7 本、budget の class は 9 本。
- 当てた後の `python3 scripts/dev/check-architecture-map.py --diffs --reference <当てた capability-map.json>`: 食い違いは 141 件から 142 件。
  増えたのは `wrong_owner` の orphan-takeover 1 件（`run_supervisor.rs` を名指しする地図の行がまだ無い）。
  run-claim-lease・accounts-quota・budget-pricing は既に wrong_owner があり、key（capability id）は増えない。
  own-supervisor が地図に `crates/task-dispatch/src/run_supervisor.rs` の行を足すと消える。
- 確かめた後、2 つの policy file は HEAD に戻した。この branch では policy file を変えていない（rule policy.changed に当たらない）。

## 旧い file を消すと arch-map の検査に当たるか

`scripts/dev/repo-health/rules/arch_map.py` は判定を `scripts/dev/check-architecture-map.py` の `find_diffs` に任せる。対照（capability-map.json）は
policy の側から読む。今の地図と対照で、`crates/task-dispatch/src/dispatcher/` の 10 file（leases・ownerless_runs・quota_book・token_budget・
route_select・dispatch_run・housekeeping・worker_finish・review_spawn・review_verdict）が無いとして試算した。

1. 地図を変えずに消す: `wrong_path` が 7 件増える。地図の行が名指しする file（dispatch_run.rs は 55 行目ほか、worker_finish.rs 64 行目、
   review_spawn.rs・review_verdict.rs 65 行目、token_budget.rs 66 行目、housekeeping.rs 74 行目、ownerless_runs.rs 81 行目）。
   消す unit が同じ変更で地図の行の path を直せば当たらない。
2. capability-map.json の owner path は、存在ではなく地図の path の先頭一致で照合される。地図の 59 行目（source/account 選択・quota 見積り）の
   `crates/task-dispatch/src/dispatcher/` が残る間は、file を消しても `wrong_owner` に当たらない。
3. 59 行目から `crates/task-dispatch/src/dispatcher/` を外すと、`wrong_owner` の orphan-takeover（`dispatcher/leases.rs`）と、`missing_invariant` の
   run-claim-lease（`dispatcher/leases.rs`）・routing-lane-route（`dispatcher/route_failure.rs`）・credential-broker-resume
   （`dispatcher/credential_gate.rs`）が増える。

扱い: capability-map.json の owner path と profile の高 risk の正規表現が名指しする `dispatcher/` の file（`leases.rs`・`ownerless_runs.rs`・
`quota_book.rs`・`token_budget.rs`・`dispatch_run.rs`・`route_select.rs` など）は、policy から旧い path を撤去するまで、owner の method を呼ぶだけの
`Dispatcher` の薄い入口として残す。旧い本体は消す。地図の 59 行目の `crates/task-dispatch/src/dispatcher/` は R4 の間は外さない。

旧い path の撤去: record の後に operator が policy だけの branch で、消えた file の正規表現と owner path を外す。責任者は operator。
