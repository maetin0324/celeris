---
title: 運用 runbook の棚卸し（docs/ops/・docs/web/）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# cleanup-ops: 運用 runbook の棚卸し

対象は `docs/ops/`・`docs/web/`（`docs/ops/sccache-l1.md` は別 task 01M3YD2Z58 の担当なので触っていない）。
各 runbook の命令・パス・unit 名を `scripts/selfdeploy/*.sh`・`scripts/dev/test-parallel.sh`・`deploy/systemd/*.service`・
`web/server/app.js`・`.config/nextest.toml`・`tools/nextest/VERSION` と照らした。repo 内から参照されているので
3 本ともパスは動かしていない（`install-units.sh`・`release.sh`・`test-parallel.sh`・`.config/nextest.toml`・
`crates/celeris/src/releases.rs`・`gui/scripts/e2e-check.mjs`・`docs/architecture-map.md`・`docs/README.md` が参照）。

全体リンク検査を通すために、完了済みの兄弟 WU `cleanup` のブランチ（`c4a69296`。全体リンク検査で残った旧パスを直したもの。
`docs/ops/web-parallel-operation.md` の 3 か所も含む）を先に merge した。同じ行を別々に直して統合時に衝突するのを避けるため。

## 処理結果

| 処理 | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | `docs/ops/selfdeploy.md` | `docs/ops/selfdeploy.md` | 済んだ移行の手順を削除: §9「改名の移行（`migrate-to-celeris.sh`）」全体（`~/taskd` は既に無い）、§1 の `--remove-old` の段落、§4a の「いまの本番は `target/debug/celeris` を手で起こしたもの」。§5b は DB が既に `/var/lib/celeris/celeris.sqlite3` にあることを書き、再移動の手順だけに縮めた。台本に合わせて直した: install-units.sh が置く unit 5 本、`SD_SENSITIVE_PATTERNS` の `agent-docs/adr/0040-`/`0041-`、verify の検査 4c（`SD_VERIFY_WEB_HOOK`）と `ok` の条件、`promote.sh --pre-start`、`[handoff] drain_timeout_secs` の既定（「Phase 47 で入る」は古い）。§4 の小節を 4a〜4e の順に並べ直し、4a/4b の題を停止→起動・ライブ引き継ぎの選ばれる条件にした。ADR の参照を `agent-docs/adr/` へのリンクにした | `a42f9a54` |
| 修正 | `docs/ops/web-parallel-operation.md` | `docs/ops/web-parallel-operation.md` | §1.1（promote 時の追従）は selfdeploy.md §4e と同じ運用の重複なので表の 1 行とリンクに寄せた。§4「検証（この葉で実行したもの）」は agent の作業記録で、同じ内容が `agent-docs/progress/phase-web.md` の P6-02 節にあるので削除。P6-02 の残作業としての書き方・記録先（旧 `docs/PROGRESS.md`）・`git revert <P6-02 の commit>` を除き、今の手順（unit の構成、web 用の鍵、verify の検査 4c、unit での起動、公開 bind の条件、止め方）に書き直した。非 loopback の bind で password ファイル必須は `web/server/app.js` で確認 | `c4a69296` |
| 修正 | `docs/ops/nextest.md` | `docs/ops/nextest.md` | 命令・版・環境変数は `test-parallel.sh`・`release.sh`・`tools/nextest/VERSION`（0.9.146）と一致。経緯（2026-09-28 の導入日・約 50 分かかった話）を一般的な注意に縮め、版の直書きを `tools/nextest/VERSION` 参照にし、release.sh の実際のエラー文に合わせた。ADR・sccache-l1 への参照をリンクにした | `248d7888` |
| 不変 | `docs/ops/sccache-l1.md` | `docs/ops/sccache-l1.md` | 別 task 01M3YD2Z58 の担当（人の指示） | `87f07cb4` |

`docs/web/` は move-docs（`a42f9a54`）の時点で全 11 ファイルが処理済みで、今は空。各ファイルの行は
[move-docs.md](move-docs.md) の表にある（`docs/web/dogfood.md` は削除、`docs/web/parallel-operation.md` は
`docs/ops/web-parallel-operation.md` へ移動して上の表で整理、他 9 本は `agent-docs/web/` へ移動）。
`agent-docs/web/` の 9 本は agent の記録なので、この WU では内容を変えていない。

削除・統合・移動したファイルは無い（節の削除だけ）。

## 証拠

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0（cleanup の merge 前は 46 件の壊れた参照で exit 1。
  うち docs/ops の 3 件は cleanup と同じ直し）。
- `git diff --quiet ad454132 -- docs/ops/sccache-l1.md` → 差分なし。

## 未解決

- `docs/ops/selfdeploy.md` §4e「本番に一度だけ人が行う手順（2026-10-02 の一時回避を撤去する）」は残した。
  本番の `~/.config/systemd/user/celeris-web@ea86af6307f8.service.d/` がまだあり（読み取りで確認）、撤去が済んでいないため。
  人が撤去したら、この小節は削除してよい。

## 提案

- `deploy/systemd/celeris-web@.service` のコメントが旧パス `docs/web/parallel-operation.md` と `ADR-0096` を指している
  （リンク検査の対象外なので通っている）。`docs/ops/web-parallel-operation.md`・web ADR-W3 に直す。
  deploy/ は `SD_SENSITIVE_PATTERNS` に入る（昇格で sha12 確認になる）ので、この WU では変えなかった。
- `scripts/selfdeploy/migrate-to-celeris.sh` と `install-units.sh --remove-old` / `--remove-qwen-tunnel` は済んだ移行の道具。
  手順は docs から消したので、台本自体を消すかは scripts を扱う task で判断する。
