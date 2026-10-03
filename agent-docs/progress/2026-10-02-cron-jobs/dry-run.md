---
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
---
# 日次整理 job の実機 dry-run 記録（2026-10-03）

## 結果

**daemon 経由の staging 整理 run は未実行。** この worktree では
`knowledge-curation` と cron API/CLI が実装されているが、現在本番に配置された
`~/.local/celeris/current/bin/celerisctl --help` に `cron` サブコマンドがない。新しい CLI の
`celerisctl cron run daily-curation` は `POST /api/v1/cron-jobs/{id}/run` で task と履歴を DB に作る。
CLI に `--dry-run` オプションはなく、`dry_run` は job 雛形の `mode` で指定する。現行 daemon を
相手に安全な手動起動はできず、本番 DB への書き込みもこの run では許可されていない。

LLM 認証は `codex login status` で `Logged in using ChatGPT`、本番 KB の読み取りも確認した。
daemon 経由の検証を止めた理由は、cron 対応 daemon/CLI が本番未配置であり、
本番 DB を書かない隔離実行環境がまだないこと。人の決定に従い、下記の本番 KB 写しに対する
オフライン dry-run で差分と要約を生成した。本番 KB と DB は変更していない。

前回の読み取り専用の事前集計では、`_inbox` に Markdown が **264 件**あり、front matter の `path` は
**217 種**（現存する宛先 48、未作成の宛先 169、複数候補が同じ宛先を指すもの 25 種）。
`op` は `create` 176、`update` 56、`merge` 20、`retire` 12 件。これは整理案の確定数ではない。
同じ宛先を指す候補には内容の異なるものがあり、機械的に最後の候補で上書きできない。

| 種別 | 今回の差分 | 確認する出力 |
| --- | --- | --- |
| 統合 | 写しで 42 件の候補を既存ページへ統合 | `curation-plan.json` の `merge` と `curation.diff` の統合元・先 |
| 新規 | 112 ページを提案 | `curation-plan.json` の `new` と `curation.diff` の追加ページ |
| 削除 | 218 件を提案（候補 212・重複ページ 6） | `curation-plan.json` の `delete` と、削除する path・理由 |
| 1 日要約 | `daily-summary.md` を生成 | `daily-summary.md`、人に残る判断、受信箱の提案 |

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


## 実機 dry-run の結果

人の決定 `dry-run-path = a` に従い、2026-10-03 に本番 KB の内容 hash を取り、
`cp -a` で run の `artifacts/kb-copy/` へ複製した。保存先で POSIX permission の保存だけが
`Operation not supported` となったため `cp -a` は非 0 だったが、複製されたファイル数は
本番と同じ 3,565 件。差分対象の元本文は全件 hash が一致し、変更対象外 3,257 ファイルも
本番と byte 単位で一致した。KB の正本・DB・daemon には書いていない。

- 入力: `_inbox` の Markdown 265 件（宛先 218 種）。
- 計画: 統合 42、**新規ページ 112**、削除 218（候補 212・既存の重複 6）、
  旧状態の修正 1、保留 11。新規ページの生成と元候補の削除は同じ候補について重なるため、
  これらの件数を単純合算して入力件数とはしない。
- 古い候補 32 件は新しい正本と照合して採用しない。`browser-capability-adr-0078-phase1.md` の
  Phase 2 差し戻しは 2026-09-29 時点の過去形に修正し、後の Phase 1〜4 完了と整合させた。
- 重複の寄せ先: ETXTBSY の 2 ページを `etxtbsy-stub-executable.md` へ、remote worktree の
  2 ページを `remote-worktree-git-commit.md` へ、negative grep と差分範囲の各 1 ページを
  それぞれの正本へ統合した。6 ページの path と削除理由は `curation-plan.json` に記録した。
- 人の判断 11 件: `human` 出所の候補、`user/` の候補、TOTP 運用との整合確認が要る
  Sirius SSH の候補。これらは写しの `_inbox` に保持し、計画では `keep` とした。
- `index.json` と `README.md` は写しで再生成し、索引の 327 ページと実ファイルが一致した。
  `curation.diff` は D11 の計画対象 415 path だけを含め、派生物の差分は `derived-index.diff` に分けた。
- この offline run には `inputs/inbox.json` と `inputs/reports.json` がないため、受信箱の
  R1〜R4 抑止件数は未計測で、受信箱の状態も変えていない。daemon 経路は昇格後に検証する。

代表的な差分は `curation.diff` の `_inbox/` 候補の消去、ETXTBSY と remote worktree の
重複ページ削除、Browser Phase 2 の文言修正。`curation-plan.json` は D11 の 7 項目
（変更前 content hash を含む）を各操作に持ち、全 hash と差分 path 集合は照合済み。

前 hash: `f021751f33d53f0127a62fd1d233ebf0de9562139e544f360b294b04f50cccb4`

後 hash: `f021751f33d53f0127a62fd1d233ebf0de9562139e544f360b294b04f50cccb4`

prod-kb-unchanged: yes

### 1 日要約の全文

```markdown
# 日次整理 2026-10-03（dry_run）

## KB（写しへの提案・本番未反映）

- `_inbox` 265 件を分類: 既存へ統合 42、新規ページ 112、候補を削除 212、人の判断まで保持 11。
- 重複する正本 6 ページを削除。古い候補 32 件は採用せず、Browser Phase 2 の過去状態を現在形に見せる記述を 1 件修正。
- `index.json` と `README.md` は写しで再生成し、索引 327 ページと実ファイルを一致させた。

## 削除するページ

- `projects/agent-platform/task-worker-etxtbsy-executable-stub.md`、`task-worker-etxtbsy-stub-executable.md`: ETXTBSY の正本へ集約。
- `environment/celeris/remote-worktree-commit.md`、`environment/clusters/sirius-task-worktree-git.md`: remote worktree の正本へ集約。
- `experience/2026/10/negative-grep-check-exclude-sibling-tests-rs.md`: negative grep の正本へ集約。
- `projects/agent-platform/wu-diff-scope-base-check.md`: 差分範囲 check の正本へ集約。

## 受信箱

- 本 run には `inputs/inbox.json` と `inputs/reports.json` がないため、R1〜R4 の抑止件数は未計測。`curation-plan.json` の受信箱提案は 0 件で、状態は変えていない。

## 人が判断すべき残り

- 11 件: `human` 出所の候補、`user/` に関わる候補、Sirius の SSH 公開鍵のみという記述（TOTP 運用との整合）。候補は写しの `_inbox` に保持した。
- 本番 KB への適用は未承認・未実施。本番 KB の前後内容 hash は同一。
```
