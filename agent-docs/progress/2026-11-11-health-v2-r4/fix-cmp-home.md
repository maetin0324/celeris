---
title: "健全性v2 R4 fix-cmp-home: script_compare の試験の config が host の release 置き場を読まないようにする"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: running
updated: 2026-11-12
---
# 健全性v2 R4 fix-cmp-home: script_compare の試験の config が host の release 置き場を読まないようにする

完了日: 2026-11-12

## 概要

- own-routing の test-affected が、変更と関係ない `celeris::script_compare_cos_digest` の 6 件で落ちた（exit 100）。
- 原因: `crates/celeris/tests/support/script_compare.rs` の `config_text` が `[selfdeploy]` を書かず、`releases_dir` と `repo` が既定値
  （`~/.local/celeris/releases` と `~/workspace/agent-platform`）になる。試験の dispatcher の停滞の検出
  （`task_ops::stagnation::read_unpromoted_releases`）が host の release 置き場を読み、verify から 3600 秒を過ぎた未昇格の release が
  あると受信箱に `release_unpromoted-selfdeploy` を足す。受信箱を比べる assert がこの 1 行の分だけ落ちる。
- main は件名「script_compare の試験の config が host の release 置き場を読まないようにする」の commit で同じ直しを入れた。この unit は
  main の版から次の 2 つだけを同じ行で写した（`git diff main -- crates/celeris/tests/support/script_compare.rs` に、この 2 か所の差は無い）。
  - `config_text` の `[harness_selection]` の後の注釈 3 行と `[selfdeploy]`・`releases_dir = "releases"`・`repo = "repo"`・`detach = "inline"`。
    相対の path は `SelfdeployConfig::resolve_paths` が config の dir（試験の一時 dir）からの path にする。
  - `write_config` の、`releases_dir` と `repo` が一時 dir の下にあることの assert。
- main の他の変更（`run_daemon`、`Env::with_config` の KB の fixture など）は写していない。後の unit sync-policy が main を取り込む。
- 基準は `$CELERIS_WU_BASE`（WU の base）。本体のコードは変えていない。

## 守る不変条件と試験

- 不変条件: `support/script_compare.rs` の config で動く試験は、host の `releases_dir`・`repo` を読まない。結果は host の release 置き場の
  状態に依らない。
- 確かめる試験: `write_config` の assert が、`support::script_compare` の config を使う試験（`script_compare_*`・`script_exhaustive`・
  `script_generation_e2e`・`project_controller_*`・`project_cos_shadow`・`long_run_soak`・`tick_scale_baseline` のうち `write_config` を
  通るもの）で、`releases_dir` と `repo` が一時 dir の下にあることを毎回確かめる。`[selfdeploy]` の行を消すと、`write_config` を通る
  試験は config を読んだ直後にこの assert で落ちる。
- 既存の試験で検出できなかった理由: 受信箱を比べる assert は、host に verify から 3600 秒を過ぎた未昇格の release がある間だけ落ちる。
  release が昇格された後や、release 置き場が無い host（CI・他の開発機）では合格するので、host の状態に依存していることが見えなかった。

## 変更

- `phase 1: script_compare の試験の config が host の release 置き場を読まないようにする` — 上の 2 か所を main と同じ行で写した。
- `phase 1: fix-cmp-home の進捗を書く` — この file。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- 偽の HOME での再現（host の本番の置き場には触れない）: `$TMPDIR/fakehome/.local/celeris/releases/aaaabbbbcccc/` に
  `manifest.json`（`built_at` 2026-11-01T00:00:00Z）と `verify.json`（`ok: true`、`at` 2026-11-01T01:00:00Z）を置き、
  `cargo test -p celeris --test script_compare_cos_digest --no-run` で作った試験の binary を `HOME=$TMPDIR/fakehome` で直に走らせた。
  - 直す前（HEAD の版の `script_compare.rs`）: 9 件中 2 件 FAILED（`script_compare_cos_digest_different_answer_keeps_rust_digest`・
    `script_compare_cos_digest_throwing_bundle_keeps_rust_digest`、`script_compare_cos_digest.rs:324` の assert。right 側に
    `release_unpromoted-selfdeploy` の行）。
  - 直した後: 9 件すべて合格。
- `cargo nextest run -p celeris --test script_compare_cos_digest --test script_compare_lane_rules --test script_compare_execution_gate`
  → 20 件すべて合格。

## 未解決

- 無し。

## 提案

- 無し。
