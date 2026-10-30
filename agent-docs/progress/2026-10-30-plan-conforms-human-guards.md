---
title: plan_conforms の人へ回す決定的な条件と危うい段の種類
tasks: [01M655VRMSE9TV5FHWXJFKSP77]
status: done
updated: 2026-10-30
---
# plan_conforms の人へ回す決定的な条件と危うい段の種類（WU guards）

完了日: 2026-10-30

## 変更
- ADR `agent-docs/adr/2026-10-30-plan-conforms-human-guards.md`（2026-10-30-cos-plan-conforms-approve の付記）。
  plan_approval を捨てて plan_conforms に一本化した経緯、G1 kind の 6 種、G2 条件 (1)〜(4)。
- `crates/task-api/src/cos/basis/plan_conforms.rs`: `StageRiskKind`（billing・node_hours・production・external_publish・destructive・secrets_permissions）を
  `GuardedStage.kind`（必須）に足した。`needs_human` が (1) acceptance の human check → 403 `cos_human_required`、
  (2) `review_human:`・`decisions:` 以外の承認理由 → 422 `cos_basis_plan_needs_human`、
  (4) 前の版の承認（最後の `PlanGateDecided{approve}`、無ければ task 作成）以降の objective・acceptance の `Edited` → 422 `cos_basis_plan_needs_human` を拒む。
  (3) 未回答の決定の 422 `cos_basis_invalid` の detail に `key (kind)` を出す。
- `crates/task-api/src/cos/basis.rs`: `within_approved_frame` から `plan_conforms::needs_human` を 1 行で呼ぶ。`BASIS_PLAN_NEEDS_HUMAN` を足した。
- 試験 `crates/task-api/tests/cos_plan_conforms_guards.rs`（7 件）。`cos_plan_conforms.rs`・`cos_chat_triage_scenarios.rs` は basis に kind を足し、task の acceptance を reviewer にした。
- API schema・web の生成型を作り直した。`docs/api/v1/api.md` の (f) に kind と新しい拒否を書いた（policy_version は変えない）。
- `config/skills/cos-inbox-triage/SKILL.md` の (f) の行の変更は打ち消した（WU policy-close で書く）。
- `crates/celeris/tests/credential_wait_notify.rs`: 試験の config に `[selfdeploy] releases_dir = "releases"`・`repo`・`detach = "inline"` を書き直した。
  既定では本番 host の release 置き場を読み、verify 済みで未昇格の release があると停滞の通知（release_unpromoted）が 1 件混ざって落ちる（replan で範囲に入った）。

## commit
- phase 1: plan_conforms に人へ回す決定的な条件と危うい段の種類（kind）を足す
- phase 1: credential_wait_notify の試験が本番 host の release 置き場を読まないようにし、進捗を書く
- phase 1: 範囲外の SKILL.md と credential_wait_notify の変更を打ち消す（範囲 check）
- phase 1: credential_wait_notify の試験の config に selfdeploy の置き場を書き直し、進捗に検査の結果を書く

## 検証（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p task-api --test cos_plan_conforms_guards --test cos_plan_conforms`: 7 + 4 passed
- `cargo test -p celeris --test credential_wait_notify`: 2 passed
- `bash scripts/dev/test-affected.sh`: exit 0（nextest 4788 passed・20 skipped、vitest 101 files・697 passed）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0（差分なし）
- `sh scripts/dev/progress-index.sh --check`: ok

## 未解決事項
- `cargo test --workspace` の `cap_probe_real_daemon`（probe の task が 404）は WU probe-flake で直した（下の節）。

## WU probe-flake: cap_probe_real_daemon の 404 task_not_found

完了日: 2026-10-30

### 原因
- celeris は主 API を `SO_REUSEPORT`（+ `SO_REUSEADDR`）で bind する（`crates/celeris/src/daemon/api.rs` の `bind_reuseport`、ADR-0040 D4）。
  同じ uid の 2 つ目の daemon が同じ port を bind しても失敗せず、kernel が新しい接続ごとにどちらかへ振り分ける。
- `start_daemon_with` は `127.0.0.1:0` に bind して port を取り、すぐ閉じて config に書いていた。閉じた後は、並行する
  別の試験（同じ binary の 5 試験、nextest では別 process）が同じ port を取れ、その daemon も bind に成功する。
- そのとき probe の `POST /tasks` は片方の daemon に、`GET /tasks/<id>` はもう片方の daemon に届き、404 `task_not_found` になる。
  子 daemon は bind に失敗しないので終了せず、daemon.log にも bind の失敗は出ない。応答した daemon の identity.pid は
  spawn した子の pid と違うものが混ざる。
- 失敗した run の daemon.log は残っていない（一時 dir）。仕組みは新しい試験
  `a_second_daemon_on_the_same_port_is_detected` で決定的に再現した: 同じ config の 2 つ目の celeris を起こすと
  `/proc/net/tcp` の LISTEN が 2 process になり、`GET /config` の identity.pid が 2 つ目の daemon の pid になる接続が出る。
- `credential_wait_notify.rs` は API を開かない（dispatcher と notifier を直接回す）ので、この穴は無い。
  同じ「port を取って閉じてから daemon に渡す」型は `routing_eval_run.rs`（同じ process で `celeris::run`）にもあった。
  他の試験は listener を持ったまま `serve_with_listener` に渡すか `listen = "127.0.0.1:0"` で、穴は無い。

### 直し方
- `crates/celeris/tests/daemon_port/mod.rs`（新規）:
  - `PortReservation` は `SO_REUSEPORT` だけを立てた socket を bind したまま（listen しない）daemon の寿命の間持つ。
    daemon の bind（同じ uid の `SO_REUSEPORT`）は通る。port 0 への bind は使用中の port を選ばない。`SO_REUSEPORT` の無い明示 bind は `EADDRINUSE` で落ちる（この host の kernel 6.8 で確認）。
  - `listening_pids(port)` は `/proc/net/tcp` の LISTEN の inode と `/proc/<pid>/fd` から持ち主の pid を返す。
  - `check_sole_owner(port, expected, identity_pid)` は identity.pid が spawn した子と一致し、port の listener が子だけであることを確かめる。
- `cap_probe_real_daemon.rs`: 起動待ちで `child.try_wait()` が `Some` なら daemon.log 付きで失敗する。`GET /config` が 200 になったら
  `check_sole_owner` を通す。通らなければ子を止め、新しい予約の port で起こし直す（3 回まで。尽きたら理由の一覧で失敗）。sleep は延ばしていない。
- `routing_eval_run.rs`: 予約を持ったまま `listen` に渡し、API が答えた後に listener が自 process だけであることを確かめる。

### commit
- phase 1: 試験 daemon の API port を予約し、spawn した子本人の応答か確かめる（cap_probe_real_daemon・routing_eval_run）
- phase 1: probe-flake の検査の結果を進捗に書く

### 検証（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo test -p celeris --test cap_probe_real_daemon --test credential_wait_notify` を 3 回続けて実行: 3 回とも exit 0（6 passed + 2 passed、cap_probe は約 2.5 秒）
- `cargo test -p celeris --test routing_eval_run`: 1 passed
- `bash scripts/dev/test-affected.sh`: exit 0（対象 celeris・celerisctl、nextest 820 passed・0 skipped。新しい試験も一覧に入る）
- `cargo clippy --workspace -- -D warnings`・`--all-targets`: exit 0
- `cargo fmt --all -- --check`: exit 0（差分なし）
- `sh scripts/dev/progress-index.sh --check`: ok
- `cargo test --workspace` はこの WU では走らせていない（release gate と最終レビューの検査で走る）

## 提案
- 計画の段に構造化した危うさの欄が無いので、kind は CoS の申告のまま。planner の出力に kind を持たせると API が申告漏れを検査できる（別 ADR）。
