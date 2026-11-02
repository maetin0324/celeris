---
title: S3 adr-fast — TS だけの変更の高速検証経路の設計 ADR
tasks: [01M6EVP95S9VM3DBMZFRD8FS35]
status: done
completed: 2026-11-02
updated: 2026-11-02
---
# S3 adr-fast — TS だけの変更の高速検証経路の設計 ADR

完了日: 2026-11-02（task 01M6EVP95S9VM3DBMZFRD8FS35、unit adr-fast。親 task は 01M6DCMB75G5F0DSE58ZNZMBAK）

## 結論
- [ADR 2026-11-02-ts-fast-verify](../../adr/2026-11-02-ts-fast-verify.md) を書いた。節は (1) 変更の分類、(2) TS-only の段、
  (3) `script-host-runner` の CLI、(4) cache key・置き場所・golden の無効化、(5) 選択規則と grader の保護、
  (6) Host API・Rust の変更と全体試験、(7) 計測の定義、境界 ADR との関係。コードは変えていない。
- 読んだ根拠: 親 task の S3 節（`celerisctl show 01M6DCMB75G5F0DSE58ZNZMBAK`）、S0 の計画 §2 S3、境界 ADR と付記 S1・S2-1〜S2-12、
  S2 の進捗（未解決・提案）、`scripts/dev/test-affected.sh`、`control-ts/`（`package.json`・`build-bundle.mjs`・`compare.mjs`・試験）、
  `crates/script-runtime/src/bin/`（`script-deploy`・`script-load-check`・`script-bench`）、`crates/script-runtime/src/manifest.rs`、
  `task-core` の example `dump-lane-fixtures`・`dump-route-priority-fixtures` と試験 `script_fixtures`、S3 の実行計画の各 unit の objective。

## 設計の前に測った秒（この worktree、1〜2 回。ADR (7) の計測ではない）
| 段 | 秒 |
|---|---|
| `pnpm install --offline --frozen-lockfile`（store から） | 0.4 |
| `tsc --noEmit` | 0.08 |
| bundle（`buildBundle({ outDir: "$TMPDIR/m/dist" })`） | 0.07 |
| `vitest run`（3 file） | 0.47〜0.49 |
| `node --test scripts/build-bundle.test.mjs scripts/compare.test.mjs scripts/gen-host-types.test.mjs` | 0.33 |
| `node --test scripts/test-affected.test.mjs` | 0.52 |
| `node scripts/compare.mjs lane_rules`（19,683 件一致） | 0.17 |
| `node scripts/compare.mjs route_priority`（2,026 件一致） | 0.10 |
| cache key 用の hash（`crates/script-runtime`・`crates/task-core` の 516 file と `Cargo.toml`・`Cargo.lock`・schema） | 0.02 |
| `rustc -V` | 0.02 |
| `cargo build -p celeris-script --bin script-bench`（debug、deps は build 済み） | 16.6 |
| `cargo build --release -p celeris-script --bin script-bench`（deps は build 済み） | 25.1 |
| `script-bench --iters 20000`（debug）: p50 0.274 ms・p99 0.330 ms、全体 12.2 秒 | — |
| `script-bench --iters 20000`（release）: p50 0.084 ms・p99 0.145 ms、全体 3.6 秒 | — |
| release の binary の sha256 の計算（68.8 MB） | 0.04 |

- 見込み（未計測）: golden 2 回で約 43,400 呼び出し。release の p50 0.084 ms なら runner の統合は数秒。TS の他の段の和は約 2 秒。
  warm p50 30 秒の目標に収まる見込みだが、measure-warm unit の 20 試行で確かめるまで未確認。
- `script-bench` の 1 呼び出しは async op を 1 回含む `host_selftest`。op を呼ばない `lane_rules`・`route_priority` の秒は未計測。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- 受け入れ 0（節がある）: `f=agent-docs/adr/2026-11-02-ts-fast-verify.md; for s in TS-only 'Host API' script-host-runner 'cache key' Cargo.lock rustc golden grader warm cold; do grep -q "$s" "$f" || { echo "missing: $s"; exit 1; }; done` → 出力なし、exit 0。
- 受け入れ 1（境界 ADR と矛盾しない）: ADR の「境界 ADR との関係」節に、矛盾しない点と違い 2 件（activate の代用、S2-7 の load 時の capability 拒否が daemon 側で見つからない）を書いた。
  境界 ADR はこの unit の範囲外なので、付記は close unit が書く。
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh` → exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path が ADR とこの dir だけ）→ 範囲外なし。
- コードを変えていないので test-affected・clippy・fmt は対象なし（下の結果を参照）。

## 未解決
1. 付記 S2-7 の「許可表が空の領域の manifest が `capabilities` を宣言していたら load で拒否」は、
   `crates/celeris/src/daemon/script*.rs`・`crates/script-runtime/src/manifest.rs` の grep では見つからなかった。daemon 側に在るかは未確認。
2. runner の値の検査の上限は `task-dispatch` の `classify_*` の写しになる。一致を固定する試験の置き場（`crates/celeris/tests`）が S3 の unit の範囲に無い。
3. nightly の gate がこの repo に在るかは確かめていない。
4. op を呼ばない領域での本物の host の 1 呼び出しの秒は未計測。

## 提案
1. close unit: 境界 ADR に付記を足す（(f) の activate は TS だけの変更の計測では配備と load で代用する、S2-7 の拒否の実装の有無）。
2. 別 task: `crates/celeris/tests` に、runner の値の検査の上限（`rule_id` 128 byte・`condition` 512 byte・候補 256 件）と
   `task-dispatch` の `classify_lane_rules`・`classify_route_priority` の上限が同じことを確かめる試験を置く。
3. S6: 自己改善の pilot の WorkUnit に、daemon 側の範囲 check（`^control-ts/src/` だけを許す）を置く。branch 自身の `scripts/dev/` の書き換えは試験では防げないため。
4. S4: 新しい領域（digest）を足すときは、runner の採点の表・`compare.mjs`・fixture を同じ差分で足す。足さないと runner の領域の検査（manifest の `domains` と表の一致）で落ちる。
