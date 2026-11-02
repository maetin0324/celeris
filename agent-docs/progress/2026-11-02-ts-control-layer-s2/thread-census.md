---
title: S2 thread-census — script_runtime_valid_bundle_loads の flaky を直す（親の名前を引き継ぐ間を actor と数えない）
tasks: [01M6E6HEH7CPZX9KX4Y2SAZKTG]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S2 thread-census

## 結論
- `crates/celeris/tests/script_runtime.rs` の sampler を tid 単位の観察に直した。本番コード（`crates/script-runtime`・`crates/celeris/src`）は変えていない。
- 各標本は全 thread の `(tid, comm)` の列。純粋関数 `census_actors(&[Sample])` が、tid ごとに最後に読めた名前が `celeris-script` のものを actor と数える。
- `script_runtime_valid_bundle_loads` は `seen.actors == 1` を確かめる。actor が 2 本走れば 2 になり、今までどおり落ちる。

## 原因
Linux の新しい thread は親の comm を引き継ぐ。actor（`celeris-script`）が watchdog・V8 worker を起こすと、子が名前を付けるまでの間、子も `celeris-script` と読める。旧 sampler は名前の一致だけを標本ごとに数えて最大値を取ったので、この瞬間を 2 本と数えた。

## 依頼の文面との違い（判定規則）
依頼は「一度でも別の名前で読めた tid は actor と数えない」だった。この規則のままだと actor 自身を落とすので、「最後に読めた名前が `celeris-script`」に変えた。

actor 自身も、改名する前は spawn した thread の comm（`tokio-rt-worker`）で読める。下の 20 µs の観察では `["tokio-rt-worker", "celeris-script"]` と読めた tid があり、依頼の文面どおりの規則は 40 回中 6 回で 0 本を返した。この変化の向きは `script_thread_census_counts_actor_read_before_its_own_naming` で固定した。

## 試験（名前に script_thread_census を含む）
純粋関数:
- `script_thread_census_excludes_child_before_naming`: 改名前の子を除いて 1 本。
- `script_thread_census_counts_child_only_seen_renamed`: 改名後の子しか読めなくても 1 本。
- `script_thread_census_counts_actor_read_before_its_own_naming`: 改名前の actor も 1 本。
- `script_thread_census_counts_two_actors_as_two`: 同時に走る 2 本は 2。
- `script_thread_census_counts_actors_in_turn_as_two`: 順に走る別 tid の 2 本も 2。
- `script_thread_census_no_thread_is_zero`: 標本なし・空の標本・`celeris-script` なしは 0。

本物の thread:
- `script_thread_census_real_child_inherits_then_renames`: `celeris-script` と名付けた thread から名前なしの子を spawn し、channel で止めた間に標本を取る（名前の一致は 2 本）。子が `/proc/thread-self/comm` に書いて改名した後にも標本を取り、1 本と判定する。順序は channel だけで決め、sleep を使わない。
- `script_thread_census_daemon_sampler_detects_second_actor`（検出力）: `celeris-script` の thread をもう 1 本走らせたまま daemon を動かし、`run_daemon` の sampler が 2 と数えることを確かめる。

## sampler 間隔 20 µs での確認
`SAMPLE_INTERVAL` を一時的に 20 µs に縮め、観察の `eprintln!` を足して通した。確認後に両方を戻した（戻した後の値は 5 ms）。

| 対象 | 回数 | 失敗 | 内訳 |
|---|---|---|---|
| `script_runtime_valid_bundle_loads` だけ | 40 | 0 | 新しい判定は 40 回とも 1 本。1 回の標本は約 3,900 件 |
| `script_runtime` の file 全体（14 試験） | 40 | 0 | 40 回とも 14 passed |

同じ標本を他の規則で数えた結果（`script_runtime_valid_bundle_loads` の 40 回）:
- 旧 sampler の規則: 2 本が 17 回、1 本が 23 回。つまり旧規則なら 17 回落ちる。
- 依頼の文面どおりの規則: 0 本が 6 回、1 本が 34 回。

## dispatch-hook の follow-up との重複
dispatch-hook の run が同じ修正を follow-up「script_runtime_valid_bundle_loads の flaky を直す（thread 名の継承を 2 本と数える）」として起票している（`followups.*.applied.json` に記録あり）。この unit がその内容を実施したので、あの follow-up は重複である。task の id と現在の状態はこの run からは確かめていない。人か CoS が、この unit の取り込み後に取り消す。

## commit（件名）
1. `phase s2: thread-census: thread の観察を tid 単位にして親の名前の引き継ぎを actor と数えない`

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
CHECKS_PLACEHOLDER

## 未解決
1. 上記の follow-up の取り消し。
2. `assert_eq!(script_threads(), 0)`（join の直後の 1 回の読み取り）は変えていない。Linux では join が返った後も thread が `/proc/self/task` に短い間残る可能性があるが、未確認である。今回の file 全体 40 回では 1 度も落ちなかった。

## 提案
なし。
