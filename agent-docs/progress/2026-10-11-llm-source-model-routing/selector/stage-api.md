---
tasks: [01M4Q53EHAF7BS9V3QMM1WY352]
status: done
completed: 2026-10-13
---
# selector / stage-api: 段階・根拠表の読み取り API と人の段階変更

ADR 2026-10-11-llm-source-model-routing-evaluation D3 の「`enforce` へ進めるのは人、戻すのは daemon」の人の側の口。

## 変更

- `crates/task-ops/src/routing_stage.rs`（新規）: 読み取り view と人の段階変更の本体。
  - `list_stages`: 段階表（`routing_stage`）と根拠表（`routing_evidence`）に現れる genre×harness の組を列挙し、
    `stage`・`since`・`evidence_version`・`latest_evidence_version`・直近の `routing_stage_changed`（`last_change`）を返す。
    明示の DB path を読み取り専用で開く（store の trait は 1 組ずつしか読めないため）。
  - `list_evidence`: 組ごとの最新版の根拠表（区間・`qualifies`）。
  - `set_stage_by_human`: actor `human` で `store.set_routing_stage` を呼ぶ。`enforce` は最新版に D1 の
    「根拠が立つ」行（`n >= 24` かつ Wilson 下限 `pass_lower >= 0.5`）が要り、無ければ `EvidenceInsufficient`。
    理由が空白だけ・task 不明も拒否。
- `crates/task-api/src/routing_stage.rs`（新規）: `GET /api/v1/routing/stages`、`POST /api/v1/routing/stages`、
  `GET /api/v1/routing/evidence`。根拠不足の `enforce` は 409 `routing_evidence_insufficient`、不明 task は 404、
  空の理由は 422、未知の欄・クエリは 400。`ApiState` に `db_path` を足した（読み取り view の別接続用）。
- CoS operation registry: `POST /routing/stages` を `human_only` で EXCLUDED（`cos/ops/admin.rs` と
  `agent-docs/adr/2026-10-09-cos-operations-all-mutations.md` の D2・D3 表と付記）。
- `crates/celerisctl`: `routing stage list [--genre --harness --json]`、`routing stage set --task --genre --harness
  --stage shadow|enforce --reason`、`routing evidence [--genre --harness --json]`。全て明示の `--db` が要る
  （`stage list` は読み取り専用接続、`set`・`evidence` は store を開く）。
- 文書・生成物: `docs/api/v1/api.md`（表 217〜219、§3.126.20〜22）、`UPDATE_SCHEMA=1` で `docs/api/v1/api-v1.schema.json`、
  `corepack pnpm@12.6.0 -C web gen:types` で `web/api/generated/{schema.json,types.ts}`。

## 試験

- `crates/task-api/tests/routing_stage.rs`: 空の一覧、未知クエリ 400、根拠表無し・D1 未達の enforce が 409、
  根拠表だけの組は shadow・since 無し、evidence の絞り込み、422/404/400、承認後の `last_change`（actor human）、
  shadow へ戻した後の `from=enforce`、event 2 件。
- `crates/task-ops/src/routing_stage.rs` の単体試験、`crates/celerisctl/tests/routing_cli.rs` の
  `routing_stage_set_refuses_enforce_without_evidence_then_lists_the_human_change`。

## 検査結果

- `mkdir -p /local/x/sel && TMPDIR=$(mktemp -d /local/x/sel/t.XXXX) sh -c 'cargo test -p task-api && cargo test -p celerisctl'` → exit 0（95 suite、820 passed、0 failed）。
- 長い TMPDIR（run の既定）では `task-api/tests/browser_live_cross.rs` の 3 試験が `path must be shorter than SUN_LEN` で落ちる（本変更と無関係、既知の環境要因）。
- `cargo clippy -p task-ops -p task-api -p celerisctl --all-targets -- -D warnings` → exit 0。
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0。
- `git diff --check $CELERIS_WU_BASE` → 差分の空白問題なし。範囲は brief の path だけ。

## 未解決

- event の帰属: store の `set_routing_stage` は `TaskId` を要求し、全体（task に属さない）の event の口が無い。
  本 unit は API・CLI の本文に `task_id`（承認を記録する task。受信箱の承認の対象など）を必須にした。
  前回 run の「先頭の task に付ける」暫定は消した。
- D1 の判定は根拠表の行の `n`・`pass_lower` だけで見る。「高価 model は 12 問×1 の全問」と
  「paired 差の区間が劣ると確定していない」は行に問題数・paired 情報が無いため判定していない。
- `GET /routing/stages` は guardrail の窓（直近の run 数・違反）を出さない（guardrail-tick unit の成果が入った後、web-routing と合わせて足す）。

## 提案

- task-core に task に属さない system event（または `routing_stage_changed` 専用の帰属）を設け、`task_id` 必須を外す。
- 根拠表に問題数・k・paired 比較の結果を持たせ、D1 の全条件を `row_qualifies` で判定する。
