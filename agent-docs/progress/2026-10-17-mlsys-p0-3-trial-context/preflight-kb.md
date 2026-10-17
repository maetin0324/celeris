---
title: "MLSys P0-3 runbook: 測定専用環境の作成・リセット・再開と preflight の canary 検査"
tasks: [01M54HYGQPFNRX6PNPJJG475EB]
status: done
updated: 2026-10-17
completed: 2026-10-17
---
# 測定専用環境の runbook（WU preflight-kb）

通常運用の KB・記憶・会話履歴・状態、他試行、grader 私有領域が A/B/C の試行に入らないようにする。汎用の権限基盤は足さない。
構成（別 config・別 DB・別 KB・別 dir）と `celeris-campaign preflight` の検査、canary で保証し、直読みは残余リスクとして扱う。
本番 host の変更（systemd・本番 config・本番 DB）は**人が**行う。この文書はその手順。

## 1. 何を preflight が見るか

`celeris-campaign preflight|run ... --measure-config <測定 daemon の config.toml> --normal-config <通常運用の config.toml> --canaries <canaries.json>`
（`--normal-config` 既定は `~/.config/celeris/config.toml`。読むだけ）。無いと `measure-env` で不足になり、`run` も始まらない。

| 検査 | 内容 | 落ちる例 |
|---|---|---|
| (1) 測定 config | `knowledge.root` を**明示**し通常運用の root と重ならない。root は空か `--kb-src` と同一 hash。`default_mounts` を明示し `kb:` だけ（`memory`・`repo:`・`dir:` を含めない）。`[memory]` は無し（推奨）か、別 path で空。`db`・`workspace_root` が通常運用と別。`[knowledge.langmem]`・`[knowledge.gc]` の enabled・`[[cron.seed]]` 無し | root 省略、mount に `memory`、DB 共有 |
| 接続の保全 | `[[clusters]]` が通常運用と同一。`[api]` に `listen` と `token_file` がある（task-api 認証） | cluster 欠落・差異 |
| (2) 試行用 KB | `--repo-src/--materials-src/--kb-src` から A/B/C の試行木を `prepare_trial` で作り、`kb/` の hash が 3 者で同一 | A/B/C で KB が違う |
| A のアカウント dir | `--a-account` の dir に `CLAUDE.md` が無く `projects/` が空（`account_dir_residue`） | 過去 transcript・自動記憶が残る |
| (3) canary | 試行木（`kb/`・`materials/`・repo、再開時は既存 `--out` 全体＝output 含む）に `forbidden` が無い。`kb/` に `required` が全部在る | 通常 KB の目印が混入、試行用 KB の目印が配られていない |

`canaries.json`: `{"forbidden":["<通常 KB・他試行・grader 私有領域にだけ置いた識別文字列>",…],"required":["<試行用 KB(--kb-src)にだけ置いた識別文字列>",…]}`。
目印は秘密ではない。キーの値は載せない。

## 2. 作成

1. 測定専用 dir を決める（例 `/local/celeris/measure/<campaign>/`）。下に `kb/`（空 or 固定初期 KB）・`celeris.db`・`ws/`・`token` を置く。通常の `~/.local/celeris`・`~/.local/share/celeris` は使わない。
2. 通常 config をコピーして測定 config を作り、次だけ変える: `db`、`workspace_root`、`[knowledge] root` と `default_mounts = ["kb:..."]`、`[api] listen`（別 port）と `token_file`（測定用 token）。`[memory]` は書かない。`[[clusters]]` はそのまま残す（実クラスタ接続を保つ）。
3. 測定 daemon は `CELERIS_CONFIG=<測定 config>` を付けて起動する（worker が `celerisctl knowledge` の根を測定 config から引くため。付けないと worker は `~/.config/celeris/config.toml` の通常 KB に落ちる）。起動は人が行う。
4. grader 私有データ（`--grader-root`）と out dir は、試行の実行中は試行から読めない場所に置く（別 uid か権限 000 の dir、または全試行の終了後に grade 用に配置する）。repo-src/materials-src/kb-src の下に置かない。
5. A の `CLAUDE_CONFIG_DIR`（人の決定 a-claude-config-dir）: **測定専用の claude_oauth アカウント dir** を用意し、A/B/C の測定だけで使う。dir の作成は GUI のアカウント login か `POST /api/v1/llm/accounts` 系の API で行い、**資格情報の投入（ログイン）は人が行う**（worker は資格情報に触れない）。ログイン待ちは既存の通知で知らせる。確認: `GET /api/v1/llm/accounts` で `logged_in: true`、dir の `projects/` が空、`CLAUDE.md` と自動記憶（`projects/*/memory`）が無い。A の HOME は bwrap 内で `HOME=/tmp/home`（tmpfs）で、アカウント dir だけを rw bind する（`harness_exec.rs` の Sandbox）ので、このアカウント dir を測定専用にすることが A の記憶隔離の本体。
6. `celeris-campaign preflight ...`（上の 3 フラグ付き）が exit 0・`ok: true` になるまで直す。

## 3. リセット（やり直し・汚染の疑い）

1. 測定 daemon を止める（人）。
2. 測定 dir の `celeris.db*`・`ws/`・`kb/`（固定初期 KB から作り直す）・out dir を捨てる。**通常運用の dir は触らない**。events は追記専用なので捨てるのは測定 DB だけ。
3. 測定専用アカウント dir の `projects/`（transcript・自動記憶）を空にする（資格情報は消さない）。
4. 作成の 2〜6 をやり直す。canary は新しい文字列に替えてよい。

## 4. 再開（run の中断後）

1. daemon と out dir は**捨てない**。同じ測定 config・同じ `--out` で preflight を実行する。`--out` が在ると既存試行木（output を含む）全体を禁止 canary で走査する。落ちたら、その試行は contaminated として記録し、リセットするか export で区別する（人の決定 bc-filesystem-confinement (2)：試行後に全 transcript・tool 呼び出しを走査し、許可外 path 参照を contaminated にする）。
2. 測定 config の変更（特に KB root・mount）は再開前に preflight が拒否する。変えたなら「リセット」へ。
3. 通れば同じコマンドで `run` を続ける（ledger と snapshot は再利用される）。

## 5. 残余リスク（受け入れ済み）

- B/C worker は同じ uid で host の他の dir を読める（bwrap 化は今回しない）。path 分離＋canary で「文脈・検索に出ない」ことを示し、直読みは上の (1) 実行中は読める場所に置かない、(2) 試行後の走査で contaminated 記録、で下げる。
- 試行 task 側の文脈（`run_context.rs` の KB 前置き・記憶・会話履歴）を試行用 KB に固定するコード修正は WU fix-context が担当。canary の実 router 試験は WU canary-e2e。この文書の preflight は構成と試行木を見るだけで、dispatcher が実際に組む文脈は見ない。

## 検証の区別

- 実装済み＋無課金試験: `crates/campaign/tests/preflight.rs` の `preflight_accepts_the_isolated_measurement_environment`・`preflight_rejects_a_measurement_config_that_reaches_normal_operation`・`preflight_rejects_a_measurement_kb_root_with_unknown_content_and_accepts_the_fixed_kb`・`preflight_keeps_cluster_and_api_connections`・`preflight_rejects_canaries_in_the_trial_tree_and_a_trial_kb_without_its_canary`・`preflight_requires_the_measurement_environment_inputs`。
- 未検証: 実 daemon を測定 config で起動しての接続確認、測定専用アカウントへの実ログイン、実 harness が `kb/` を読むこと。

## 証拠コマンド（WU preflight-kb）

- `cargo test -p celeris-campaign` → 全 pass（preflight 試験 17 件、うち新規 6 件）
- `bash scripts/dev/test-parallel.sh` → exit 0（passed 5623, failed 0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 変更の要約

- `crates/campaign/src/measure_env.rs`（新規）: 測定 config と通常 config の突き合わせ、`Canaries`。
- `crates/campaign/src/isolation.rs`: `scan_canaries`・`check_trial_context`（A/B/C の試行木・kb hash 同一・canary）。
- `crates/campaign/src/preflight.rs`: `Inputs.measure`（`MeasureEnv`）と `measure-env` 検査。無いと不足。
- `crates/campaign/src/main.rs`: `--measure-config/--normal-config/--canaries`。`Cargo.toml`: `toml`・`tempfile` を dependencies へ。

## 未解決事項

- 破壊的変更: preflight/run は `--measure-config` 等が必須になった。本番 config の変更は無い。
- `--grader-root` が repo-src 等の下に無い検査（R9）は未実装（棚卸しの提案 1）。runbook の手順 4 で人が守る。
- Claude Code 自動記憶を切る設定名は未確認。代わりに測定専用アカウント dir の `projects/` 空・`CLAUDE.md` 無しを preflight（`account_dir_residue`）で検査する。

## 提案

- grader root の位置検査（R9）を足す。
