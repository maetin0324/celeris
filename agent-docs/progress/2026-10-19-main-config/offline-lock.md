---
tasks: [01M5A3BZC2GE7PHTE2MV5AQX4B]
status: done
completed: 2026-10-19
---
# offline-lock: 通常関数を使う最小の無試行 lock 入口

## 実装
- 既存 CLI（`env-lock`）は catalog を実 daemon から取るため、daemon 無しで campaign lock（`lock.json`）まで作る入口が無かった。
  そこで `celeris-campaign offline-lock` を 1 つ追加した（新基盤・新 schema は無し）。
- 本体は `crates/campaign/src/offline_lock.rs`。呼ぶのは通常経路と同じ関数だけ:
  `preflight::expected_lock` → `env_lock::expected`（事前判定）→ `lock_exec::begin` → `env_lock::establish` → `lock_exec::save` → `lock_exec::load_saved`（再照合）。
  watch・runner・executor・台帳・meta は呼ばない。out に残るのは `env-lock.json`（celeris-env-lock/3）と `lock.json` だけ。
- catalog は承認済み実証跡（`GET /api/v1/llm/models` の本文を保存した JSON）から `env_lock::catalog_from_models` で取る。
  捏造防止のため `--models-evidence-sha256` を必須にし、file の sha256 が違えば何も書かず拒否する。

## 引数
`celeris-campaign offline-lock --manifest <m> --out <dir> --a-program <path> [--worker-program <name>=<path>]...
--models-evidence <json> --models-evidence-sha256 <hex> [--cap-env <json>] --cases-root <dir> --grader-root <dir>
--grade-case-root <dir> [--measurement-id <id>] --repo-src/--materials-src/--kb-src/--inputs-src <dir> [--verify]`
- 新規作成時は取り込み元（source）を全て要求する（`lock_exec::begin` の Fresh と同じ）。`--verify` は何も書かず、保存済みの両 lock を現物から作り直した期待値と照合する。
- 出力 JSON: `paid_calls_made: 0`、`env_lock` / `campaign_lock`（created|verified）、`campaign_sha256`、`lock_sha256`。

## 証拠
- `cargo test -p celeris-campaign --test env_lock --test campaign_lock --test lock_cli --test inner_symlink --test cap_probe` → 全 ok（lock_cli に offline 試験 4 件追加、既存 inner_symlink 10・cap_probe 7 維持）。
- 試験（実 binary）: 成功・再実行 verified・`--verify`、実証跡の本文差替え（hash 不一致）・欠落・空 catalog、偽 catalog・A 実体差替え・repo-src / cases-root 不一致・保存 lock / env-lock 変造の拒否、試行 data のある out の拒否。拒否時は out を変えない。

## 未解決・提案
- 本番での実行（実際の承認済み実証跡・release での lock 生成）は人が行う。本タスクは通常 config・release・本番 DB に触れていない。
