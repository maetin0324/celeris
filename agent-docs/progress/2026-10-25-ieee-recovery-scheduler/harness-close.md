---
title: IEEE recovery scheduler — ADR 付記 I14 と実装の突き合わせ・harness 段の総括（harness-close）
tasks: [01M5T2P8MAW1564BFKSXXV33YG]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# ADR 付記 I14 と実装の突き合わせ・harness 段の総括（harness-close）

完了日: 2026-10-25。基準は `git merge-base HEAD main`（WU は `$CELERIS_WU_BASE`）。D1–D5・I1–I13 は変えていない。

## やったこと

- `agent-docs/adr/2026-10-25-recovery-scheduler-backend.md` の付記 I14 に (7)「実装突き合わせ」を足し、実装との食い違いを
  付記側で直した（実装が正）。主な点: 入口は `run_pbs`（`run_from_env` が振り分け）、`run.json` は全 trial 後に呼び出し側が
  `write_run` で 1 回（`real_jobs` は `RunMeta` に残る）、e2e の in-memory `Cluster`・`entry::run_trial`・`barriers.jsonl` は
  I12 経路として残る、型の置き場所は新規 `pbs/recovery.rs`、`wall_spawn_to_wait_saved_ms`、拒否は `reject.json`
  （`recovery-trials.jsonl` に行を足さない）、`collector_failed:` / `unobserved:` は Err の理由名、子の故障 3 値、
  再集計入口 `regen_from_env` / `DAGBENCH_REGEN_RAW`・`DAGBENCH_REGEN_OUT` と出力 TSV 4 本。
- `docs/architecture-map.md` の campaign::dagbench::pbs 行（6 列のまま）に正式入口（`dagbench_harness.rs` の
  `run_from_env` → `run_pbs`・`pbs_control_child`・`regen_from_env`）、module（file_pbs・proc・recovery・metrics）、
  新試験（`harness_pbs_*`・`harness_pbs_raw_*`・`harness_pbs_reject_*`、`tests/common/dagbench_pbs_controller.rs`）、ADR 範囲 I1–I14 を反映。

## 受け入れ条件と証拠（提出時の HEAD で実行）

- 条件 0「付記と実装の差異なし」: 付記 (7) の各項を `crates/campaign/src/dagbench/pbs/{proc,recovery,metrics,file_pbs,raw,entry}.rs`・
  `tests/dagbench_harness.rs`・`tests/dagbench_pbs_e2e.rs`・`tests/common/dagbench_pbs_controller.rs` の定義と grep で照合。
  `cargo test -p celeris-campaign --test dagbench_harness harness_pbs_` → exit 0、15 passed（proc 6・raw 4・reject 5）/ 17 filtered out。
- 条件 1「architecture-map 検査 OK」: `python3 scripts/dev/check-architecture-map.py` → exit 0（743 件のパスを確認）。
- 条件 2「文書検査 3 本と test-affected・clippy・fmt」:

| 検査 | 結果 |
|---|---|
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` | exit 0（ok） |
| `sh scripts/dev/check-adr-numbers.sh` | exit 0（293 files） |
| `bash scripts/dev/test-affected.sh`（WU 基点） | exit 0、docs のみの変更で Rust 試験対象なし |
| `CELERIS_WU_BASE=$(git merge-base HEAD main) bash scripts/dev/test-affected.sh` | exit 0、crates celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch、2700 passed / 8 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |

## harness 段の commit（件名）

- 「docs: add ADR addendum I14 for formal entry, process stop, raw metrics and rejections」
- 「harness-proc: connect PBS backend and normal/A2 to the formal dagbench entry with a separate controller process」
- 「harness-raw: record recovery-trials raw with separate wall/logical clocks and regenerate metrics from the new raw only」
- 「harness-reject: refuse stop-before-save, missing saved evidence, job mismatch, duplicate submit/consume and unknown scheduler on the formal pbs entry」
- 「harness-process: close the child stdin only after SIGKILL so an immediate kill is never pre-empted by an EOF exit」
- 「harness-close: reconcile ADR appendix I14 with the implementation and map the formal pbs entry」
- ほか各 unit の進捗・検査記録の commit（「docs: record harness-design checks」「harness-reject: progress」「harness-reject: record checks at the submitted HEAD」）

## 未解決事項

- 実 `SshPbs` 接続（`DAGBENCH_PBS_TRANSPORT=ssh` は入口で拒否）・Sirius の認証・承認残枠の消費は人の決定の後。本段の試験は
  無 LLM・無実 job の偽 transport（`FilePbs`）での決定的試験で、LLM の未知修復能力・HPC 実測とは称さない。
- `write_run`（測定を閉じる）を呼ぶ env 入口が無い。`run_from_env` だけで trial を足した測定は `regen_from_env` で Err になる。
  実測の前に閉じる入口（例: ignored の close 入口）が要る。
- 旧い仕組みの残り: e2e の in-memory `Cluster`・`CelerisController`、`entry::run_trial` の in-process A2（`Gate` の `on_stop`）・
  `barriers.jsonl`・`entry::A2Evidence` は正式入口では使われないが残る（I14 (2)(3) の「消す」は未実施。付記 (7) に記載）。
- 拒否された trial は `reject.json` だけで、raw（`recovery-trials.jsonl`）からは拒否件数を再集計できない。
- `tests/dagbench_pbs_e2e.rs` に未使用 import `ExtBackend` の warning（test target。`cargo clippy --workspace` は対象外で exit 0）。

## 本番への移行

- なし。本番 host の操作（daemon・config・DB・release）は行っていない。

## 提案

- 正式入口に測定を閉じる入口を足し、e2e を `run_pbs` 経由へ移してから `entry::run_trial`・`barriers.jsonl`・in-memory `Cluster` を消す
  （ADR-2026-10-11 breaking changes by default）。拒否 trial も raw に `rejected` 行で残す形を検討する。
