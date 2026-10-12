---
tasks: [01M4Q53E9WQEKMAS448GZMWTFB]
title: eval-harness 締め — routing 評価 runner の全体検査と ADR・architecture-map・進捗
---
# eval-harness 締め

評価 runner（crate `routing-eval`、bin `celeris-routing-eval`）と根拠表（`routing_evidence`、migration 0071）の全体検査を流し、ADR 付記と architecture-map の行を書いた。実装は足していない（この unit の変更は文書だけ）。

## 実装の要約（前の unit の成果の確認）

| 機能 | 所在 | 状態 |
|---|---|---|
| 7 model の候補表（30×2・12×1・quota 15%） | `crates/routing-eval/src/candidates.rs` | 定数 7 件 |
| 評価集合 manifest（時期分割・層別・部分集合・features・sha 凍結） | `manifest.rs` | 実装済み。同じ version の sha 違いは拒否 |
| recheck（base で失敗・head で成功の再実行） | `recheck.rs` | 実装済み |
| dry-run（呼び出し数・定価換算・quota 窓・縮小案） | `dryrun.rs` | 実装済み。codex の単価は未定義で USD は「未算定」 |
| 評価 run の起動（Celeris API 経由） | `run.rs`、task-api `routing_eval`、celeris の tick（付記 2026-10-12 eval-dispatch） | 実装済み。本番では未実行 |
| aggregate・routing_evidence 書き込み | `aggregate.rs` | 実装済み。`--write-evidence` |
| 統計（Wilson・bootstrap・McNemar・Pareto・同等集合・k=1 規則） | `stats.rs` | 実装済み。CLI `stats` は単独入力なし |
| export（features 結合 JSONL） | `export.rs` | 実装済み |
| 通し試験（stub） | `crates/routing-eval/tests/pipeline_stub.rs` | 1 test passed（前の unit の記録） |

## 使い方（人が手で回す順。本番 DB は読み取り専用）

```
celeris-routing-eval manifest --db <本番 DB の read-only path> --out <dir> [--cutoff 2026-10-05] [--seed 0] [--version 1]
celeris-routing-eval recheck --manifest <dir>/manifest.json --repo <path>
celeris-routing-eval dry-run --manifest <dir>/manifest.json --db <path> [--measured <usage-delta.json>]
celeris-routing-eval run --api <url> --manifest <dir>/manifest.json --candidates <id,...> [--token-file <path>] [--repo <path>] [--wait] [--out <file>]
celeris-routing-eval aggregate --db <path> --manifest <dir>/manifest.json --out <dir> [--write-evidence]
celeris-routing-eval export --db <path> --manifest <dir>/manifest.json --out <file.jsonl>
```

`run` を本番で実行する前に、dry-run の受信箱決定（呼び出し数・費用・quota）と eval-budget の人の回答が要る（下の「未解決」）。

## 証拠（この unit で実行したコマンドと結果）

すべて `TMPDIR=/local/x`（8 byte）で流した。`test-parallel.sh` は TMPDIR の下を約 97 byte 掘るので、長い TMPDIR では browser_e2e の unix socket が 108 byte を超えて落ちる（下の「失敗した試行」）。

- `bash scripts/dev/test-parallel.sh`（最終 run）: exit 0。nextest `5207 tests run: 5207 passed, 13 skipped`（173 binaries）。doc-test 11 binary 全部 ok（`task_core` の compile-fail doc-test 3 件を含む）。`CELERIS_TEST_SUMMARY {"passed": 5210, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0, "tmp_leftovers": 0}`。
- `cargo clippy --workspace --all-targets -- -D warnings`: exit 0。
- `cargo clippy --workspace -- -D warnings`（CLAUDE.md の既定コマンド）: exit 0。
- `cargo fmt --check`: exit 0（差分ゼロ）。
- `sh scripts/dev/check-adr-numbers.sh`: exit 0（183 files）。
- `sh scripts/dev/check-doc-links.sh`: exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`: exit 0。
- `python3 scripts/dev/check-architecture-map.py`: 初回は exit 1（3 件の表内パスが存在しない: `<workspace>/runs/<run_id>/tmp/`、`check-test-tmp-leftovers.sh`、`test-parallel.sh`。前の unit の行 67・68 で、この checker は表の backtick 内の `/` や `.sh` を実在パスとして扱う）。行 67 の placeholder を外し、行 68 の 2 件を `scripts/dev/` 付きの完全パスに直した。直後は exit 0（`OK: 379 件のパスを確認した`）。
- `sh scripts/dev/check-test-tmp-leftovers.sh -p routing-eval`: exit 0（`no leftovers`）。

### 失敗した試行（記録）

- TMPDIR を `/local/x/c1`・`/local/x/c2`（11 byte）にして `test-parallel.sh` を流すと、`task-api::browser_e2e` の 4 件が `timed out waiting for fixture readiness`（60 秒）で落ちた（2 回とも再現）。同じ binary を `cargo test -p task-api --test browser_e2e` で単独に流すと、両 TMPDIR とも 4 passed。
- TMPDIR を `/local/x`（8 byte）にすると、同じ suite が全 pass した。このため最終 run は `/local/x` で取った。
- 結論: 試験の失敗は routing 評価のコードと無関係で、TMPDIR の長さに依る。`/local/x` 直下だけを使う（`/local/x/<k>` は使わない）。

## 変更したファイル

- `agent-docs/adr/2026-10-11-llm-source-model-routing-evaluation.md`: 付記 2026-10-12（eval-harness）を追加。harness_version の major の決め方、manifest の形、根拠表（migration 0071）と evidence_version、評価 run の DB 契約、dry-run の単価と quota の扱い、統計の規則、本番実行の手順が人の決定待ちであること。
- `docs/architecture-map.md`: routing 評価 runner の行を追加（crate・module・CLI・試験・ADR）。行 67・68 の checker 不通過（上記）を直した。
- `agent-docs/progress/2026-10-11-llm-source-model-routing/eval-harness.md`: この進捗（新規）。

## 未解決

1. **本番で評価を走らせるのは人の決定待ち**: eval-budget（LLM 呼び出しの予算）の人の回答、dry-run の受信箱決定（実行直前の呼び出し数・費用・quota の見積り）、codex の quota を測る 10 run の実測（usage endpoint の前後差）。これが揃うまで `run` は本番で走らせない。今回の run も LLM を呼んでいない。
2. **codex の定価換算 USD が出ない**: `dryrun.rs` の単価表は claude 4 model だけ。`gpt-6.1-sol`・`gpt-6-luna`・`gpt-6-astra` は「未算定」になる。単価の出所（価格表の版）を決めて定数を足すのは人の確認が要る。
3. **CLI `stats` は単独で使えない**: `celeris-routing-eval stats` は説明つきの失敗を返す（`aggregate` が呼ぶ library 処理）。単独入力の形式を決めるなら別の unit。
4. **`crates/routing-eval/src/lib.rs` の `not_implemented`**: 実装が済んだ後の未使用の pub helper が残っている（`grep -rn not_implemented crates/routing-eval` で定義だけ）。消すのは次の小さな変更で行える。この unit では実装に触れないため残した。
5. **`held_out_reverses` の範囲**: 関数は渡された dev と held-out の並びを全体で比べる。ADR の「層ごとに反転したら不成立」の層別比較は、呼び出し側（aggregate）が層ごとに渡す形になっているかを次の unit で確かめる。
6. **evidence_version は集計日を含む**: 同じ manifest でも別の日に aggregate すると別版の行が増える（追記。同日の再集計は置換）。根拠表の版が古いかの判定（ADR D3）は daemon 側の段階の unit で扱う。

## 提案

- 本番で評価を走らせる前に、人が次を決める: (a) eval-budget の上限、(b) codex の単価の出所、(c) `[routing_eval] reserved_runs` の値（既定 0 = 評価 run を起こさない）。決まったら、dry-run → 10 run の実測 → 受信箱決定 → `run --wait` → `aggregate --write-evidence` の順で回す。
- `scripts/dev/test-parallel.sh` の TMPDIR の掘り方を、web/e2e と同じく短い固定 path に寄せるか、browser_e2e の socket path を短くする修正を別の unit にする（今回は触れていない）。
