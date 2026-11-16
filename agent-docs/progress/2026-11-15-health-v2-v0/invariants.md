---
title: 健全性v2 V0 invariants — capability-map の不変条件と保護 13 件を HEAD の試験に対応づけて流す
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# 健全性v2 V0 invariants

完了日: 2026-11-16

## 結論
- 対応表 `invariants.json` は 139 行（capability-map の不変条件 125 件、repo profile の保護 13 件、親 task の「候補が policy を弱めて合格しない」1 件）。
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/invariants.py --run agent-docs/progress/2026-11-15-health-v2-v0/invariants.json` は exit 0。142 個の command（同じ command は 1 回）がすべて exit 0 で、どれも 1 件以上の試験に当たった。走った試験は計 951 件、所要 901 秒。
- 行の結果: 通った 133 行、release gate で確かめる 4 行、未検証 2 行。通った行のうち 12 行は一部だけを確かめている（下の「一部だけ確かめたもの」）。
- 落ちた不変条件の試験は無い。delivery-repair・replan の申告は要らない。
- この WU は試験を足していない（V0 の write set は progress・ADR・SPEC・architecture-map だけ）。足すべき試験は「提案」に書いた。

## 作り方
- 入力: `agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json` の `capabilities[*].invariants`（evidence_path と tests）と、`scripts/dev/repo-health/profiles/celeris.toml` の `[review.invariants]`（13 件）。
- HEAD の試験名は `cargo nextest list --message-format json` で 13 crate（celeris-campaign と e2e を除く）の 7,575 件を出し、不変条件ごとに試験名の正規表現で選んだ。celeris-campaign は `crates/campaign/tests/*.rs` の関数名で選んだ。
- capability-map の試験 file のうち 18 件は HEAD に無い。R3〜R6 の「bundle … integration test targets」で束ねた suite の module に移ったもの 16 件、改名 1 件（project_auto_integrate → repo_auto_integrate）、R6 legacy で削除 1 件（task-ops の plan/tests.rs）。`git log --diff-filter=DR` と nextest の一覧で今の所在を追い、`invariants.json` の `moved_tests` に書いた。dispatcher の試験 3 件（cos_chat_triage_fallback・project_controller_replay・project_achievement）は module の入れ子に移っていた。
- 各行の command は `cargo nextest run -p <crate> --lib|--test <target> -E 'test(/<正規表現>/)'` か `python3 -I -m unittest discover -s scripts/tests -p <file> -k <名前>`。`--lib`・`--test` で対象の binary を絞り、build と一覧の時間を減らした。
- `invariants.py` は表の形（不変条件・根拠 file の存在・command・結果・coverage が full でない行の理由）を検査し、mode=run の command を順に流す。exit が 0 でないか、0 件の試験にしか当たらない command があれば exit 1、表の形が壊れていれば exit 2。`--record` で各行の `result` を書き戻す。

## 通った不変条件
不変条件の種類ごとの行数（通った / release gate / 未検証）:

| 種類 | 通った | release gate | 未検証 |
|---|---|---|---|
| 人の権限 | 13 | 0 | 0 |
| credential/sandbox | 16 | 2 | 0 |
| fail-closed | 24 | 1 | 1 |
| 原子的な state+event | 18 | 0 | 0 |
| lease/排他 | 11 | 0 | 0 |
| restart/replay | 13 | 0 | 1 |
| budget | 8 | 0 | 0 |
| reviewed candidate の同一性 | 6 | 0 | 0 |
| その他 | 24 | 1 | 0 |

保護 13 件の結果:

- `budget-unknown-cost-stops`: 通った: 2 command・9 件
- `completion-not-self-reported`: 通った: 2 command・11 件
- `credential-stays-in-broker`: 通った: 3 command・6 件
- `events-append-only`: 通った（一部）: 1 command・9 件
- `lease-single-holder`: 通った: 3 command・22 件
- `no-data-loss`: 通った（一部）: 1 command・11 件
- `no-llm-in-dispatcher`: 通った（一部）: 2 command・26 件
- `no-unwrap-outside-tests`: 未検証
- `remove-superseded-mechanism`: 通った（一部）: 1 command・2 件
- `task-state-in-sqlite`: 通った（一部）: 1 command・11 件
- `unpromoted-release-no-takeover`: 通った: 2 command・18 件
- `worker-db-read-only`: 通った: 2 command・17 件
- `worker-no-subagents`: 通った: 2 command・22 件
- 親 task の「候補 branch が検査器・policy・baseline を弱めて自分を合格させない」: 通った（`test_policy_weakening_is_evaluated_on_base_and_fails`）

capability ごとの行と試験名は `invariants.json` の `rows[*].checks[*].tests` にある。

## release gate で確かめるもの
WU の `--run` から外し、release gate（`scripts/selfdeploy/release.sh`）で確かめる。release.sh は userns の要る試験を `CELERIS_USERNS_TESTS=1` で走らせ、web の gate も持つ（release.sh の冒頭の説明）。

- `cap:browser-launcher#0`: `cargo nextest run -p task-worker --test browser_launcher_ptrace` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `cap:browser-egress-runtime#0`: `cargo nextest run -p task-worker --test browser_runtime_isolated` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `cap:browser-live-view-control#1`: `release gate の web 段（web の unit 試験）: web/server/browser-live.test.mjs` — web の unit 試験。release gate の web 段で確かめる
- `cap:web-gateway#0`: `release gate の web 段（web の unit 試験）: web/server/relay.test.mjs` — web の unit 試験。release gate の web 段で確かめる
- `cap:web-gateway#1`: `release gate の web 段（web の unit 試験）: web/server/origin.test.mjs web/server/auth.test.mjs` — web の unit 試験。release gate の web 段で確かめる
- `cap:web-spa#0`: `release gate の web 段（web の unit 試験）: web/api/client.test.ts web/test/routes.test.ts web/features/browser/browser-query.test.ts` — web の unit 試験。release gate の web 段で確かめる
- `cap:sandbox-db-guard#0`: `cargo nextest run -p task-worker --lib -E 'test(/^db_guard.*(nested_user_namespace|inside_namespace|not_writable_inside|read_only_but_siblings|no_capabilities|systemd_bus_is_hidden)/)'` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `cap:sandbox-db-guard#0`: `cargo nextest run -p e2e --test worker_db_read_only` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `cap:sandbox-db-guard#2`: `cargo nextest run -p task-worker --lib -E 'test(/^db_guard.*host_config_read_only_inside_namespace/)'` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `profile:worker-db-read-only`: `cargo nextest run -p task-worker --lib -E 'test(/^db_guard.*(nested_user_namespace|inside_namespace|not_writable_inside|read_only_but_siblings|no_capabilities|systemd_bus_is_hidden)/)'` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる
- `profile:worker-db-read-only`: `cargo nextest run -p e2e --test worker_db_read_only` — userns・本物の namespace が要る。WU の --run から外し release gate（test-parallel の userns 段）で確かめる

## 未検証のものと理由
- `cap:delivery-merge-candidate#1`（Merging 中の daemon 再起動では外部操作を自動で再実行せず Blocked にして git と承認 SHA の照合を求める）: Merging 中の daemon 再起動で Blocked にする挙動を名指しする試験は試験名と grep で見つからない
- `profile:no-unwrap-outside-tests`（no-unwrap-outside-tests）: clippy の unwrap_used は workspace で有効でなく、repo-health の rule にも unwrap の検査は無い。review だけで守る

### 一部だけ確かめたもの
- `cap:browser-site-policy-trusted-devices#1`: store と API の試験は通るが、表と event を同じ store 関数で書くことを名指しで確かめる試験名は見つからない
- `cap:pre-review-sync-auto-resolve-integration-repair#3`: auto_resolve の試験は scratch で merge するが、本番の checkout に触らないことを直接は確かめていない
- `cap:routing-lane-route#0`: dispatcher が LLM を呼ばないことを検出する試験は無い。選択が決定的なことは Rust の試験で確かめた。TS 側の試験（control-ts/src/domains/lane_rules.test.ts・route_priority.test.ts）は WU の --run に入れていない。release gate の script-bundle 段は bundle と Rust 側の script 試験を回すが、vitest の単体試験を回すかは未確認
- `cap:project-controller#0`: Rust 側の replay 試験は通った。判定（TS）側の試験（control-ts/src/domains/project_controller.test.ts）は WU の --run に入れていない。release gate が vitest の単体試験を回すかは未確認
- `cap:continuation-session-resume#2`: CheckpointSaved event を名指しする試験は無い。checkpoint の読み取りと再起動後の継続は試験がある
- `cap:workunit-scheduling-stage-integration#3`: ExecutionGated の追記は試験があるが、routing と同じ transaction であることは確かめていない
- `cap:recursive-task-tree#1`: 子の insert と unit と親の event が 1 transaction であることは直接は確かめていない。event の書き込みは試験がある
- `profile:events-append-only`: events 表の UPDATE・DELETE を拒む trigger も、それを確かめる試験も無い。store に events の更新・削除の関数が無いことと、review の高 risk class events で守る。試験があるのは script・project 系の追記専用表と、遷移と event の追記順
- `profile:no-data-loss`: migration が行を保つ試験はある。credential を消さないこと一般を確かめる試験は無い
- `profile:no-llm-in-dispatcher`: dispatcher に LLM 呼び出しが入ったことを検出する試験・検査は無い。純粋関数の試験（orphan・execution_scheduler）と review で守る
- `profile:remove-superseded-mechanism`: repo-health の migration_remnant rule の試験は通る。旧い仕組みが残ったかの最終判断は review
- `profile:task-state-in-sqlite`: 継続 session を DB に置く試験はある。worker が memory に状態を持たないこと一般を確かめる試験は無い

## 証拠コマンドと結果（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/invariants.py --run agent-docs/progress/2026-11-15-health-v2-v0/invariants.json --record`: exit 0、`rows: pass=133 fail=0 release_gate=4 unverified=2`、`commands: ok=142 fail=0`、901 秒
- 同じ command を `--record` 無しで再実行（control-ts の 2 行の書き直しの後、build の cache が温まった状態）: exit 0、`commands: ok=142 fail=0`、951 件、214 秒
- `bash scripts/dev/test-affected.sh`: exit 0（試験対象の変更なし。docs のみ）。`cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。`sh scripts/dev/progress-index.sh --check`: ok
- `cargo nextest list`（13 crate）: 7,575 件、190 binary、build 130 秒

## 未解決事項
- userns の試験は WU で `unshare -Ur` が通る host でも、目的どおり `--run` から外した。release gate の結果は integrate-verify の後に人が release.sh を実行して確かめる。
- 正規表現で選んだ試験が不変条件のどの面を確かめるかは試験名で判断した。試験の本文までは読んでいない行がある。

## 提案
- `no-unwrap-outside-tests`: workspace の `[workspace.lints.clippy]` に `unwrap_used = "deny"`（試験は `#[cfg(test)]` で allow）を置けば、決定的な検査になる。
- `events-append-only`: events 表に UPDATE・DELETE を拒む trigger を置く migration と、その試験を足す。
- `cap:delivery-merge-candidate#1`: Merging 中の daemon 再起動で外部操作を再実行せず Blocked にする試験を celeris の delivery の試験に足す。
- `cap:workunit-scheduling-stage-integration#3`・`cap:recursive-task-tree#1`: 同じ transaction であることを、途中で失敗させて何も残らないことで確かめる試験を足す。
- `no-llm-in-dispatcher`: task-dispatch の依存に LLM の client crate・HTTP client が入らないことを repo-health の deps rule の profile に書けば、決定的な検査になる。
