---
title: CoS chat run usage telemetry
tasks: [01M4D3XKDVFHSEHD0JVBR10PMN]
status: done
updated: 2026-10-08
---

# CoS chat run usage telemetry

## 実装

migration 0061（schema 61）で chat_runs に usage_json・skill_reads・first_output_at を追加した。worker の既存 Terminal Usage（input/output/cache read/cache creation/名目 cost_usd/session_resumed/duplicate_reads）を CoS ChatRunSink の finish で終端状態と同一 transaction に保存し、終端 run event にも含める。既存 started_at・finished_at と resolved_config_json の harness・model・session_mode はそのまま利用する。

既存の resume 拒否 → fresh 再試行では、同じ chat run の観測を引き継ぐ。usage を報告した試行の token・cost・duplicate_reads を加算し、複数の報告のいずれかで欠けた項目は不明にする。session_resumed は最後に usage を報告した試行の値。usage 全体が無い場合は usage を省略する。旧 run の未知の計測値を 0 で埋めない。

skill_reads は stream の tool_use の Skill と .claude/skills 配下 SKILL.md の Read の観測回数。tool_result・通常ファイル・Bash 経由は含めない。harness が明示しない内部読み込みは不明。first_output_at は最初の空でない text delta（thinking/status/tool は除外）。latency_ms と time_to_first_output_ms は保存した開始・終了・最初の本文時刻の差をミリ秒で返す。本文差分が無ければ first output 指標を省略する。

GET /api/v1/chat/threads/{t}/runs/{r} の run と、新設 GET /api/v1/chat/threads/{t}/runs の items が同じ telemetry を返す。一覧は最新順、limit 既定 50 / 最大 200、before に前ページの next_before を渡す。不明または別 thread の cursor は 400。終端の再送で保存済み telemetry を上書きしない。

docs/api/v1 を UPDATE_SCHEMA=1 で再生成し、web/api/generated と gui/app/celeris/types.ts を更新した。docs/protocol も再生成試験で照合し、変更は無い。web 表示は追加していない。

prompt、skill 配送、session 選択、rollover 条件、node_sessions.approx_tokens の既存加算は変更していない。一般 worker の Usage 型・adapter の挙動も変更していない。

## 検証

cos_chat_usage_ で始まる試験は、一時 DB の旧版移行・永続性・不明値・終端再送、固定時計の skill 回数・latency・再試行集計、fake adapter の実行から保存まで、REST の詳細・一覧・ページング・usage 欠落時の省略を検証する。sleep や実 LLM の応答には依存しない。

- UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker committed_schema_matches_generated: 成功。
- cargo clippy --workspace -- -D warnings: 成功。
- node web/scripts/gen-types.mjs --check: 成功。
- 指定チェック `cargo test -p task-core -p task-dispatch -p task-api -p celeris cos_chat_usage_ 2>&1 | tee /dev/stderr | grep -qE 'test result: ok\. [1-9]'`: 成功（6 件、pipefail も有効）。
- `bash scripts/dev/test-parallel.sh -E 'package(task-core) or ((package(task-dispatch) or package(task-api) or package(celeris)) and test(chat)) or test(committed_schema_matches_generated)'`: 成功（対象 1,101 件全件成功、対象外 3,716 件除外。workspace doc-test も成功）。
- GUI gen:types の再生成前後の SHA256 比較: 差分無し。GUI typecheck: 成功。
- cargo fmt --all -- --check / git diff --check: 成功。

本番 config/DB/KB/release/systemd は変更していない。fixture の一時 DB と fake adapter のみ使用し、subscription/API 課金とも LLM 呼び出しは無い。実モデルの cache hit 率・skill 内部読み込み・実利用時 latency・請求額は不明。
