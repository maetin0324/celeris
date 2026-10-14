---
title: "評価の実施（eval-run）: 見積りを受信箱で決めてから 7 model を走らせ、根拠表と学習データを出す"
tasks: [01M4VAAKFJHRSVKZ3996NFKSKS]
status: done
updated: 2026-10-14
---
# 評価の実施（eval-run）

完了日: 2026-10-14。ADR 付記: [2026-10-11-llm-source-model-routing-evaluation.md](../../adr/2026-10-11-llm-source-model-routing-evaluation.md) の「付記 2026-10-13: 評価の実施（eval-run）」。結果: [eval-results.md](eval-results.md)。unit ごとの記録: [eval-run/](eval-run/)。

## 要約

- manifest-v2（問題 30 件）を本番 DB から凍結（sha256 `118b413ebba94d51a0ee6d6703c9dd7b726d84ade3bbd4009c137f903d21e7e7`、evidence_version `…-2026-10-14`）。
- dry-run → codex pilot（旧 batch は仕組みの不具合で全 error、集計除外。やり直し batch 01M4W7JDSJ は passed 6・failed 4）→ 実測で見積り（estimate.md）→ **受信箱で人の承認（2026-10-14）の後に** 本評価 276 試行（pilot 10 を含む）を投入（submit.md）。
- 投入中: ディスク満杯による停止と再開、codex の 7d 超過で 93 試行 cancel → 人の指示で上限 21% に上げて再投入、仕組み側 error は試行ごと最大 2 回再投入（18・3 試行）。
- 全試行終端: passed 251・failed 20・error 36・cancelled 93。export 271 行。
- cell（coding、n・pass・Wilson 95%）: opus 24/24 0.862–1.000、sonnet 24/24 0.862–1.000、haiku 22/23 0.790–0.992（根拠立たず）、sol 27/30 0.744–0.965、astra 11/12 0.646–0.985、luna 21/29 0.543–0.853。費用（定価換算 USD 合計）opus 41.42・sonnet 20.46・haiku 17.46。7d: claude 0.09→0.18/0.19、codex 0.02→0.20。

## 証拠（closeout、2026-10-14）

- 人の承認: 受信箱の回答「estimate.md の表で本評価を投入してよい」（2026-10-14、claude 144 試行・7d 15%、codex 132 試行・7d 13.2%）の後に submit。上限の変更（codex 21%、claude は投入前 + 15%）も人の回答・指示による（eval-run/collect.md）。
- `TMPDIR=/local/x bash scripts/dev/test-parallel.sh` → exit 0、`test-parallel: ok`、nextest passed 5085・failed 0・ignored 13（binaries 181）、doctest exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決

- routing-eval の aggregate・export が outcome の無い試行（error・cancelled）を不合格に数える不具合。集計は手元で直した runner で行い、修正は follow-up task（差分 `artifacts/eval/aggregate-error-exclude.patch`）。この task の branch の runner は未修正。
- 本番 `routing_evidence` への書き込み（`aggregate --write-evidence`）は人の手順。上の修正が main に入った後に流す（eval-results.md）。
- haiku の 1 問は yielded 3 回連続で有効な結果なし。fable は今回 0。quota_per_run は空。

## 提案

- 評価試行の run にも quota 実測 event を残し、quota_per_run を根拠表に入れられるようにする。
- 予算内に終わらない問題（yielded 連続）は manifest 凍結時か集計で印を付け、yielded の扱い（除外／不合格）を ADR に決めておく。
