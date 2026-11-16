---
title: 健全性v2 R6 scratch — scratch の旧欄・重複定数・未参照の関数と script の片付け
tasks: [01M6TYVJT7EJ6WWHEF0EBH3JPS]
status: done
updated: 2026-11-15
completed: 2026-11-15
---
# 健全性v2 R6 scratch — scratch の旧欄・重複定数・未参照の関数と script の片付け

完了日: 2026-11-15

WorkUnit `scratch`。B0 の [`dup-candidates.json`](../2026-11-07-health-v2-b0/dup-candidates.json) で R6 に割り当てた
scratch の 5 件を片付けた。4 件を消し、1 件（schema 名の定数）を 1 か所に寄せた。残した項目は無い。
基準は `$CELERIS_WU_BASE`（この WU の開始時の snapshot）。Cargo.toml・生成 API schema の JSON・API の欄
`ScratchCacheView.stats`（段 api の console-api の担当）は変えていない。

## 結論

| id | 判定 | 変更 |
|---|---|---|
| `config-scratch-l1-max-gb` | 消した | `[scratch] l1_max_gb` と `default_scratch_l1_max_gb` を `crates/celeris/src/config/scratch.rs` から削除。書いた config は `deny_unknown_fields` で読み込みエラー。試験 `removed_scratch_l1_max_gb_is_a_load_error` で固定 |
| `scratch-status-schema-const` | 寄せた | 唯一の定義を `task_ops::daemon::SCRATCH_STATUS_SCHEMA` に残し、未使用の `task_worker::scratch::STATUS_SCHEMA` を削除（理由は下の「schema 名の定数の置き場所」） |
| `unref-replace-owner` | 消した | `SharedExtentIndex::replace_owner`（`crates/task-worker/src/scratch/gc.rs`）を削除。使われている経路は `replace_measurement` |
| `unref-unique-bytes` | 消した | `SharedExtentIndex::unique_bytes`（同）を削除。使われている経路は `owner_bytes` |
| `script-scratch-u5-stats` | 消した | `scripts/scratch/u5/stats.py` を削除 |

## 根拠の検索（消す前、作業ツリーの開始時の HEAD で実行）

検索の範囲は crates・web・control-ts・scripts・docs・config（`config/celeris.example.toml` を含む）。cfg・feature・試験・API の wire・文書の参照を見た。

- `git grep -n -e l1_max_gb -e default_scratch_l1 -- crates web control-ts scripts docs config`
  → `crates/celeris/src/config/scratch.rs` の定義の 4 行（serde 属性・欄・`Default` の行・既定の関数）だけ。
  `.l1_max_gb` の読み出し・cfg・feature・試験・`config/*.toml`・docs は 0 件。
  repo 全体では `agent-docs/adr/0075-tiered-build-cache.md` と `agent-docs/adr/2026-10-10-local-disk-growth-paths.md` が
  当時の設計として名前を挙げる（履歴の記録。R6 の write set の外なので変えていない）。
- `git grep -n -w STATUS_SCHEMA -- crates web control-ts scripts docs config`
  → `crates/task-worker/src/scratch.rs:22` の定義の 1 件だけ（task-worker の試験も参照しない）。
  `git grep -n 'scratch-status/1'` の残りは、`task-ops/src/daemon.rs:149` の定数（残す）、doc comment（task-ops の `ScratchStatus`・
  task-api の `handlers/system.rs`・`schema.rs`・celerisctl の `commands/scratch.rs`）、それらから生成された
  `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json` の description（変えていない）、
  `crates/task-api/src/handlers/tests.rs:272` の wire 値の assert（定数を使わず文字列で wire を固定する試験。残す）。
- `git grep -n -w replace_owner -- crates web control-ts scripts docs config` → `gc.rs:643` の定義の 1 件だけ。
- `git grep -n -w unique_bytes -- crates web control-ts scripts docs config` → `gc.rs:700` の定義の 1 件だけ。
- `git grep -n -e 'u5/stats' -e 'stats\.py'`（repo 全体）→ B0 の記録（`dup-candidates.*`・`slices.json`）だけ。
  同じ dir の `measure.sh` は sccache の `--show-stats` を `stats1.json` に書くが `stats.py` は呼ばない。試験は無い。

## schema 名の定数の置き場所

Objective の第一案は「`task_worker::scratch` の 1 つに寄せ、task-ops はそれを使う」だが、依存の向きで寄せられない。
task-ops の通常の依存は task-core だけで、task-worker を使うには Cargo.toml を変える必要がある（この WU では変えない）。
第二案の「両方が依存する crate（task-core）に置く」も採らなかった。task-worker はこの定数を 1 度も使わない（上の検索）ので、
2 つの crate から共有する必要が無い。task-core に移すと、wire の型 `ScratchStatus`（`task_ops::daemon`）と schema 名が別の
crate に分かれ、task-core に使い手が 1 crate だけの item が増える。そこで型と同じ場所の `task_ops::daemon::SCRATCH_STATUS_SCHEMA`
を唯一の定義にし（B0 の canonical と同じ）、doc comment に置き場所の理由を書いた。使い手は task-dispatch の `scratch_gc.rs`
（2 か所）と task-api の試験で、変えていない。

## 試験

- 足した試験 `crates/celeris/src/config/tests.rs::removed_scratch_l1_max_gb_is_a_load_error`。守る不変条件は「消した
  `[scratch] l1_max_gb` を書いた config は読み込みエラーになる（警告して無視しない）」。既存の
  `scratch_defaults_follow_the_build_cache_parent` は `bogus = 1` だけを見るので、欄を消し忘れて `l1_max_gb` が黙って通る戻りを
  検出できない。
- guard を外した確認: `l1_max_gb` の欄を一時的に戻して `cargo nextest run -p celeris --lib -E 'test(removed_scratch_l1)'` を
  走らせ、`FAIL ... removed_scratch_l1_max_gb_is_a_load_error`（1 failed）を確かめてから元に戻した。
- 消した試験は無い。消した 2 関数と script には試験が無かった。

## 変更（commit の件名）
- `r6 scratch: [scratch] l1_max_gb・重複の schema 名の定数・未参照の SharedExtentIndex の 2 関数・u5/stats.py を消す`
- `r6 scratch: 進捗に判定・根拠の検索・検査の結果を書く`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

test-affected・clippy・check-health は 1 つ目の commit の作業ツリーで実行した。2 つ目の commit は本 file だけを足すので、
code は提出時の HEAD と同じ。fmt・doc-links・write set・progress-index は提出時の HEAD で再実行した。

- `bash scripts/dev/test-affected.sh` → exit 0。nextest 6905 passed・23 skipped（対象は celeris・celeris-campaign・celeris-mcp・
  celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker）。web は task-ops の変更で typecheck と unit を実行し、
  vitest 103 files・714 tests passed。
- `cargo nextest run -p celeris --lib -E 'test(removed_scratch)'` → 2 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh --base <git merge-base HEAD $CELERIS_WU_TARGET>` → exit 0（21 rule が pass、検査器の試験も通過）。
- `bash scripts/dev/check-doc-links.sh` → exit 0。
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`。
- `git diff --name-only <基準> HEAD | python3 agent-docs/progress/2026-11-07-health-v2-b0/slices-check.py --write-set R6 …/slices.json`
  → `write-set R6: ok`。
- 計画の保護の範囲（campaign・routing-eval・jev-classifier・script-runtime・control-ts・migration・`events.rs`・`script_guard`・
  credentiald）と K2/K4/IEEE Access の凍結 fixture の差分は 0。変えた path は `crates/celeris/src/config/{scratch,tests}.rs`・`crates/task-ops/src/daemon.rs`・
  `crates/task-worker/src/scratch.rs`・`crates/task-worker/src/scratch/gc.rs`・`scripts/scratch/u5/stats.py`（削除）と本 file だけ。

## 本番への移行

本番の config に `[scratch] l1_max_gb` が残っていると、この変更を含む release の daemon は config の読み込みで落ちる。
運用者は 2026-11-15 に「本番の config に無い」と確かめた（計画の承認の回答）。release を promote する前に、人が次を確かめる。

1. `grep -rn 'l1_max_gb' ~/.config/celeris/` を実行し、0 件であることを確かめる。
2. 見つかったら、その行（`[scratch]` の中）を消す。この欄は読まれていなかったので、消しても挙動は変わらない。

## 未解決
- 無し。

## 提案
- `agent-docs/adr/0075-tiered-build-cache.md` の付記 12・本文は `l1_max_gb` を欄として挙げたまま。番号付きの ADR は R6 の write set
  の外なので変えていない。読む人が欄が今もあると誤らないよう、ADR を書き換えられる工程で付記を 1 行足す（「健全性v2 R6 で欄を消し、
  書いた config は読み込みエラー」）。
