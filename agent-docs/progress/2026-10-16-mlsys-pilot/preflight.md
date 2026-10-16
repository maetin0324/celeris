---
title: "MLSys pilot preflight: 認証可否の確認と pilot 条件固定"
tasks: [01M52BTDJVYDZT6KGPNQV2KE6H]
unit: preflight
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# MLSys pilot preflight

本番は変更していない（読み取りと隔離した試験のみ）。worker は LLM CLI・API を起動しない制約があるため、実 LLM 呼び出しは行っていない。

## 1. 認証が使えるか（結論: 「実呼び出しで使えると確認」はできていない。人の確認が要る）
| 観点 | 事実 | 根拠コマンド |
|---|---|---|
| claude-code / codex の実行ファイル | あり（`~/.local/bin/claude`、`~/.local/bin/codex`） | `which claude codex` |
| 認証ファイル | あり（`~/.claude/.credentials.json` 523B、`~/.codex/auth.json` 3860B。内容は読んでいない。codex 側の更新は 10/11 で古い） | `ls -la` |
| 外部到達性 | `curl https://api.anthropic.com` は HTTP 404 を返す（到達可。認証は未使用） | `curl -w %{http_code}` |
| API token env | worker の環境に `CELERIS_CAMPAIGN_TOKEN` は無い（人が設定する。Part P-4） | `env` の名前だけ確認 |
| 本番 daemon | 127.0.0.1 の 18200/18100/7710/7720 等が LISTEN 中。どれが task-api かは未確定（port は config 次第。人が H で確定） | `ss -ltn` |
| 隔離 daemon | worker sandbox は userns 不可の記録あり（KB: celeris-worker-sandbox-userns…）。起動可否は sandbox 外でのみ確かめられる。未確認 | — |
| 有効性（トークン失効の有無） | **未確認**。確認には LLM を 1 回呼ぶ必要があり、worker は禁止。人が `claude -p "ok"` 相当を 1 回（数十 token）実行して確かめる | — |

→ pilot 本番実行（WU pilot-run）は「認証の有効性」と「API token・port」を人が確認するまで待機。done と報告して消さない。

## 2. Part V（mock）の実行結果
- `celeris-campaign validate --manifest crates/campaign/fixtures/pilot.json` → exit 0、sha256 `2088a891…d916`。
- `dry-run` → exit 0、within_budget true、llm_calls_made 0。
- `run --executor mock` → exit 0、executed 12、tokens_total 113000。`check` → ok true、violations 空。再実行で executed 0・resumed_skipped 12（再開の冪等）。
- mock 出力は `evidence_kind: mock`。実性能ではない。

## 3. pilot manifest と task-template（条件固定）
- 置き場: `agent-docs/progress/2026-10-16-mlsys-pilot/pilot-live.json`、`pilot-task-template.json`（input は `../../../crates/campaign/fixtures/input-manifest.json` を相対参照。範囲外の crates 配下には置かない）。
- 内容: campaign_id `mlsys-pilot-1`、split dev、problems 2 件（`impl-benchfs-chunk-index`、`review-numeric-consistency`）、primary A/B、C は subset 1 件（Jev smoke 兼用）、repeats 1、max_tokens 1,000,000、per_case 100,000、per_arm 400,000、max_parallel 1、試行 5（承認の 12 以内）。model は `claude-sonnet-5-5`。fable は不使用（`grep fable` ゼロ）。
- **validate sha256: `a3b3ec43e56dab511f18e0d06bae3603d0d3590e4ba121c3ffb70b5cf773840e`**
- 検証コマンド: `celeris-campaign validate|dry-run --manifest agent-docs/progress/2026-10-16-mlsys-pilot/pilot-live.json`（exit 0）。
- **dry-run: within_budget true**（total_trials 5、推定 251,673 tokens、llm_calls_made 0。推定は仮置き係数）。
- 注意: B/C の `harness_version` はプレースホルダ（`<release sha12 at H1>`）、C の `evidence_table_id` 未設定。昇格後の release sha12 と `celerisctl routing evidence` の版で埋め、**埋めたら hash が変わる**ので再 validate して控え直す（これが実条件の固定）。
- 入力は fixture の問題（課金前の配線確認用）。実 K1〜K4 case を使う場合は未接続 3（manifest 生成 CLI なし）により手書きが要る。

## 4. キー未登録 → 受信箱 → 自動再開（隔離・偽 webhook）
- 隔離試験（一時 DB、偽 webhook。本番キー・本番 DB に触れない）: `cargo test -p celeris --test credential_wait_notify` → 1 passed（wait → 通知 1 件 → 登録 → 検証 → 一度だけ再開）。exit 0。
- 実 daemon で見る手順（人、隔離 daemon 上。本番 `jev_api_key` は消さない）:
  1. 隔離 daemon を別 DB・別 port で起動（`scripts/selfdeploy` の verify と同様の staging。本番 llm-proxy port と相乗りしない: ADR-0139）。`CELERIS_DB` と data dir を一時 dir に向ける。
  2. 隔離側で `GET /api/v1/secrets` が `jev_api_key` 未登録であることを確認（本番側は触らない）。
  3. B/C の task を 1 件投入 → WU が `waiting_for_auth` になり受信箱に `credential_wait` が 1 件、偽 webhook に 1 通知（値は出ない）。
  4. 隔離側へだけ `PUT /api/v1/secrets/jev_api_key` で登録 → `secret_stored` event → 同じ WU が一度だけ再キュー。
  5. 隔離 daemon と一時 DB を破棄。

## 5. 人が行う手順（pilot-run の前提）
1. H1: 対象 release を GUI で昇格（または pilot 用の隔離 daemon を起動。上記 4-1）。sha12 を控える。
2. H2: `GET /api/v1/secrets` で `jev_api_key` の fingerprint 有無を確認。無ければ GUI `/accounts` で登録（値はチャットに貼らない）。
3. H4: 自分の shell で `export CELERIS_CAMPAIGN_TOKEN=<admin/API token>`（`~/.config/celeris/api.token` が既存。値は argv・file に残さない）。
4. 認証の有効性確認: claude-code・codex それぞれ最小の 1 呼び出しが通ること、7 日使用率（codex 21%・claude 24% 上限）の開始値 `U0` を控える。
5. 上記の結果とともに manifest の placeholder を埋め、`$B validate`／`dry-run` を再実行して hash を更新、runbook Part P-4 を実行（`--api-url` は task-api の実 port）。

## 未解決
- 認証の実有効性・task-api の port・隔離 daemon の起動可否（人の確認待ち）。
- 実 case の manifest 生成 CLI なし、`grade` runner 未接続（runbook 未接続 1〜5）。
- 実 B/C は完了後に超過が分かるため、pilot で `max_overshoot_*` を実測する。

## 提案
- pilot-run は上の 1〜4 の人の確認後に再開。確認が取れない間は WU を blocked のまま待機（done にしない）。
- 初回は A/B 各 1 試行（task 1 件）で係数が読めるか確かめてから残りを投入する。
