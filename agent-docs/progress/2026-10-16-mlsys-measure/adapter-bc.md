---
title: "B/C 用 CelerisExecutor（監査付き API で投入・待機・再開・子 task 追跡・cancel）"
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# adapter-bc（WU adapter-bc）

## 実装済み
- `crates/campaign/src/celeris_exec.rs`: `Transport` trait（`request(method, path, body) -> Result<HttpResponse, String>`。`Err` = 応答が得られず要求が届いたか不明）と `CelerisExecutor<T: Transport>`。実 network の実装は置かない（この unit は mock 検証のみ）。
  - 投入: 通常経路 `POST /api/v1/tasks`（新スケジューラなし・SQLite 直書きなし）。本文は雛形（acceptance・project・repos 等）+ title / objective（冪等キーを末尾に記載）/ `labels`（冪等キーの先頭 64 文字 = task-core の label 上限）/ `workspace`（台帳の試行 workspace）/ `max_wall_secs` / `routing_override`。
  - 冪等: `lookup` は `GET /api/v1/tasks?label=<key64>&archived=1` で id 昇順の最古を返す。`submit` は投入直前にも照会し（二重起動で既存を返す）、投入後に再照会して自分より古い同キー task があれば自分の作った方を cancel して古い方へ寄せる。
  - routing: `routing_override(cfg)` — B=`fixed`、C=`decision_assisted`。`lower_policy` は省略時 `mlsys-min-1` を明示、`online_update` は省略時 false、C は `evidence_table_id` 必須（最新版への暗黙追従を拒否）、A は Err（Celeris に投入しない）。`routing()` は台帳へ投入前に記録する値で、投入値と同一。
  - 追跡: `track`/`poll` は `GET /api/v1/tasks/{id}` の `children` を幅優先で再帰的にたどる。全 task が終端（done/failed/cancelled）かつ `cluster_job_wait.jobs` が全て `finished|gone` になるまで `ExecStatus::Wait`。子 task id と cluster job（`scheduler:cluster:job_id`）は TaskRef に積み増し、runner が台帳に追記する。結果は根の status（done→passed、cancelled→cancelled、他→failed）、token/cost は木の全 run の usage 合計（usage の無い run があれば欠測 None）、壁時計は task 作成から最後の run 終了まで。
  - 打ち切り: `poll` で server 時計の経過（`timers.now - task.created_at`）が試行 timeout を超えたら木を cancel して `Timeout`。`cancel` は終端でない子孫を深い順 → 根の順に `POST /api/v1/tasks/{id}/cancel`（409 = 既に終端は成功扱い、全対象を試してから Err を集約）。cluster job は自動で消さず、`scancel <id>`（slurm）/ `qdel <id>`（pbs）の人手手順を `out/cluster-cancel.jsonl`（`celeris.campaign-cluster-cancel/1`、`executed:false`、同じ job・理由は 1 行）に追記。
- `crates/campaign/src/manifest.rs`: `Config.routing: Option<ArmRouting>`（`lower_policy`・`evidence_table_id`・`online_update`、`deny_unknown_fields`）。省略時は直列化しないので既存 manifest の hash は不変。A に書くと validate が拒否。
- `crates/campaign/Cargo.toml`: `time`（RFC 3339 の解釈のみ）。

## mock 検証済み（`crates/campaign/tests/celeris_exec.rs`、7 件、偽 task-api = in-process `Transport`）
- 投入・poll・応答喪失後の再照会（新しい実行器 = アダプタ再起動で同じ task、POST 1 回）、直接 submit の再呼び出しも POST 増えず
- 照会と投入の競合（二重起動）で最古へ寄せ、新しい方を cancel
- 根が done でも子孫・cluster job が終わるまで Wait、孫まで追跡、usage は木の合計
- cancel: 子孫 → 根、終端は対象外、`qdel 777` の人手手順、再 cancel は冪等
- timeout 超過で木を cancel し `Timeout`、`scancel 9` を記録
- B/C で `routing_override` が異なる（投入本文と台帳記録値が一致）、A は投入しない、C の根拠表未固定は Err
- runner 経由（A=MockAsync、B/C=Celeris の試験用束ね）: Wait で workspace 保持・task の workspace が試行 workspace、再起動しても同じ task、完了後も POST は試行あたり 1

## 実経路検証済み
なし（この unit は実 API を呼ばない）。

## 未実施（後続 unit）
- 実 HTTP の `Transport`（token・base URL）と CLI `--executor` での選択、A/B/C の束ね（cli-adr / integrate）。
- ADR 2026-10-15-mlsys-min-campaign 追補の D 番号（cli-adr）。

## 提案
- `POST /tasks` に冪等キー欄が無いため label で代用した（label は 64 文字・最大 8 個）。投入と照会の間の競合は事後に最古へ寄せる（重複 task が一瞬作られ cancel される）。厳密に「重複を作らない」なら task-api に `idempotency_key`（browser control と同じ方式）を足すのが筋。

## 証跡
- `cargo test -p celeris-campaign --test celeris_exec` → 7 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、5405 passed / 0 failed / 13 ignored
- `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` では範囲外の `crates/celeris/src/daemon/jev_verifier.rs:54` items-after-test-module が既存で出る。campaign は `--all-targets` でも clean）
- `cargo fmt --all -- --check` → exit 0

## 再実行（attempt 2 相当, 2026-10-16）
- 前回の範囲 check が `Cargo.lock`（campaign に `time` 依存を足した差分）で不合格。`time` 依存を外し、RFC 3339 の最小読み取り（`parse_ts`、unit 試験 `ts_tests`）を celeris_exec.rs 内に置いた。Cargo.lock は base と差分ゼロ。
- 証拠: `cargo test -p celeris-campaign` 全合格 / `bash scripts/dev/test-parallel.sh` passed 5406 failed 0 exit 0 / `cargo clippy --workspace -- -D warnings` exit 0 / `cargo fmt --all -- --check` exit 0 / 範囲 check exit 0。
