---
tasks: [01M44BFF0EDZZQ4MAPQZB1GARV]
---
# 知識ベースの sources の human を移行する — 運用手順（人が 1 回だけ実行する）

- 対象: [ADR-0047 付記（2026-10-04、sources の human を分ける）](../../agent-docs/adr/0047-knowledge-base.md)。
  旧形の `human` を、KB の git 履歴から `human:authored`（人が書いた・保護）と `human:instruction`
  （人の指示由来・保護しない）に書き換える。判別できないページは `human` のまま（保護のまま）残し、一覧に出す。
- 実行者: 人。本番 KB（`[knowledge] root`、既定 `~/.local/share/celeris/knowledge`）への書き込みは人の承認で 1 回だけ。
- 前提: このサブコマンドを含む release が current になっていること（daemon unit の PATH の `celerisctl` を使う）。
  まだなら、その release の `celerisctl` を絶対パスで呼ぶ。

## 判別の規則（H4）

| 履歴 | 書き換え |
| --- | --- |
| author `Celeris (human)` の通常の commit（GUI の KB 編集 = `PUT /knowledge/page`）、または Celeris 以外の author（人の直接 git）が 1 件でもある | `human` → `human:authored`（単数形 `source: human` は `author: human`） |
| author `Celeris (knowledge)` の commit と、`…を取り込む`（run の候補の accept）だけ | `human` → `human:instruction`、`task:<id>` が無ければ最初の run の commit の task id を足す |
| `init` の雛形の commit（`知識ベースを作る`・`雛形を追加`）がある／履歴が無い／単数形を指示由来にしたい | 書き換えない（一覧に出す） |

## 手順

1. 日次整理が走っていないことを確かめる（`celerisctl cron list` で daily-curation の直近 run が終わっている）。
2. 退避: `git -C ~/.local/share/celeris/knowledge tag before-human-sources-migration`
3. dry-run（何も書かない）:
   ```sh
   celerisctl knowledge migrate-human-sources
   celerisctl knowledge migrate-human-sources --json > ~/human-sources-dry-run.json
   ```
   「人が書いた」「人の指示由来」「判別できない」の 3 節が出る。指示由来に人が書いたページが混ざっていないかを見る。
4. 承認したら適用（1 commit、author `Celeris (knowledge)`、索引も作り直す）:
   ```sh
   celerisctl knowledge migrate-human-sources --apply
   ```
   最後の行が `applied: <sha>`。
5. 確認: もう一度 dry-run して、「判別できない」の節だけが残り、他の 2 節が 0 件であること。
   `git -C ~/.local/share/celeris/knowledge show --stat HEAD` で `sources` 行だけが変わっていること。
6. KB に remote があれば `git -C ~/.local/share/celeris/knowledge push`（日次整理の自動適用も次の commit で push する）。
7. 「判別できない」に残ったページは人が印を決める（決めるまでは日次整理の自動の削除・統合から守られ、
   `human_decisions` に回る）。人が書いたページなら GUI の KB 編集で保存するだけでよい（`PUT /knowledge/page` は
   旧形 `human` を `human:authored` に置き換える）。人の指示由来なら KB を直接 git で編集して `human:instruction`
   と `task:<id>` に直す（GUI で保存すると `human:authored` が付くため）。

戻すとき: `git -C ~/.local/share/celeris/knowledge revert <sha>`（索引は `celerisctl knowledge reindex`）。

## 2026-10-04 時点の本番 KB の写しでの dry-run（参考）

本番 KB を一時ディレクトリに clone して流した結果（本番には書いていない）: 人が書いた 9（`user/` 4・
`projects/benchfs/primary-sources.md`・`framing-candidates.md`・`projects/README.md`・`projects/agent-platform/design.md`・
`environment/clusters/pegasus.md`）、人の指示由来 6（`environment/clusters/sirius-job-submission.md` ほか run が作った
ページ）、判別できない 3（`environment/clusters/{fern03,sirius}.md`・`experience/README.md`。init の雛形）。
`--apply` は 15 ファイルの `sources` 行だけを変える 1 commit で、2 回目は書き換え無し。
「Phase K-1 整理」の `Celeris (human)` commit は人の編集として数える（GUI・API の人の編集の記録と区別できないため、
保護側に倒す）。
