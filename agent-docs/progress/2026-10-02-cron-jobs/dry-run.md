---
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
---
# 日次整理 job の実機 dry-run 記録（2026-10-03）

## 結果

**実機の整理 run は未実行。統合・削除・新規の差分と 1 日要約は未生成。** この worktree では
`knowledge-curation` と cron API/CLI が実装されているが、現在本番に配置された
`~/.local/celeris/current/bin/celerisctl --help` に `cron` サブコマンドがない。新しい CLI の
`celerisctl cron run daily-curation` は `POST /api/v1/cron-jobs/{id}/run` で task と履歴を DB に作る。
CLI に `--dry-run` オプションはなく、`dry_run` は job 雛形の `mode` で指定する。現行 daemon を
相手に安全な手動起動はできず、本番 DB への書き込みもこの run では許可されていない。

LLM 認証は `codex login status` で `Logged in using ChatGPT`、本番 KB の読み取りも確認した。
したがって停止理由は認証や KB の権限ではなく、cron 対応 daemon/CLI が未配置であることと、
本番 DB を書かない隔離実行環境がまだないこと。KB と DB は変更していない。

読み取り専用の事前集計では、`_inbox` に Markdown が **264 件**あり、front matter の `path` は
**217 種**（現存する宛先 48、未作成の宛先 169、複数候補が同じ宛先を指すもの 25 種）。
`op` は `create` 176、`update` 56、`merge` 20、`retire` 12 件。これは整理案の確定数ではない。
同じ宛先を指す候補には内容の異なるものがあり、機械的に最後の候補で上書きできない。

| 種別 | 今回の差分 | 確認する出力 |
| --- | --- | --- |
| 統合 | 未生成 | `curation-plan.json` の `merge` と `curation.diff` の統合元・先 |
| 新規 | 未生成 | `curation-plan.json` の `new` と `curation.diff` の追加ページ |
| 削除 | 未生成 | `curation-plan.json` の `delete` と、削除する path・理由 |
| 1 日要約 | 未生成 | `daily-summary.md`、人に残る判断、受信箱の提案 |

## 人が隔離環境で実行する手順

1. cron 対応版を本番とは別の staging daemon に配置する。**本番 DB の写し**を staging DB に使い、
   staging の knowledge root は本番 KB から作った**写し**に設定する。staging の API listen と token を
   指す設定ファイルを `<staging-config.toml>` とする。本番の daemon/DB/KB を staging の出力先にしない。
   worker を動かすので staging daemon は隔離した通常 mode で起動する（`--mode verify` は dispatch しない）。
   設定中の他の listener と外部連携も staging 専用にするか無効にする。
2. staging の job を `celerisctl cron --config <staging-config.toml> show daily-curation` で確認する。
   `template.extra.mode` が `dry_run`、`overlap` が `skip`、`harness` が `knowledge-curation`、
   `lane` が `cheap` であることを確認する。job が無ければ
   [設定例](../../../config/celeris.example.toml) の `[[cron.seed]]` を staging 用設定に入れ、空の
   cron job テーブルで起動する。
3. `celerisctl cron --config <staging-config.toml> run daily-curation` を **1 回**実行する。
   `--dry-run` は現行 CLI に存在しない。応答の `task_id` を控える。staging DB には task と履歴が
   書かれるが、`mode = dry_run` のため KB の正本への適用は行われない。
4. `celerisctl cron --config <staging-config.toml> history daily-curation` と
   `celerisctl --db <staging-db> show <task_id>` で終端状態を確かめ、task の作業場所の
   `artifacts/curation-plan.json`、`artifacts/curation.diff`、`artifacts/daily-summary.md` を読む。
   `inputs/kb/` が staging KB の写しであり、`inputs/inbox.json` と `inputs/reports.json` が
   揃っていることも確認する。計画の path と差分の path、削除理由、保護対象の
   `human_decisions`、1 日要約の件数を照合し、この記録に実数と代表的な差分を追記する。
5. staging と本番の KB の内容 hash を実行前後で比較し、本番 KB が変わっていないことを確認する。
   本番 DB の更新や job の `apply` への変更は、この dry-run の作業には含めない。

参照: [ADR-0131 D10/D11](../../adr/0131-cron-jobs.md) と
[`celerisctl cron` 実装](../../../crates/celerisctl/src/commands/cron.rs)。
