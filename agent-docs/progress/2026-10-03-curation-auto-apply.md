---
title: 日次整理の apply を承認なしで自動適用し、知識ベースの git に commit して private repository へ push する（段階 close）
tasks: [01M421KVXKGEE8PWER7HAJJFZY]
status: done
updated: 2026-10-04
---

# 日次整理の apply 自動適用: 段階 close（main 取り込み・文書検査・workspace 検査）

完了日: 2026-10-04。設計判断は [ADR-0131 付記（2026-10-04、日次整理の自動適用）](../adr/0131-cron-jobs.md)。
各 unit の詳細は `2026-10-03-curation-auto-apply/` 以下の unit 記録（`auto-apply.md`、`kb-commit-push.md`、`ops-docs.md`）を正とする。
この文書は段の close の記録（main 取り込み、文書検査、workspace 検査）と、段全体の要約。

## 状況

- 段の深さ 1: 日次整理の apply を人の承認なしで自動適用し、知識ベースの git に commit して private repository へ push する。
- この unit（深さ 2、close-sync）: main を task branch に取り込み、文書検査 3 本と workspace の試験・clippy を通して段を閉じた。

## 各 unit の成果

- **kb-commit-push（ADR-0131 付記と部品）**
  - `agent-docs/adr/0131-cron-jobs.md` 末尾に付記。承認なしの適用、`curation-human` は自動承認しない、検証失敗・元ページ変更・hash/snapshot 不一致では適用しない、1 commit の題の形式、push の扱い、救出手順。40 件上限（D12）は不変。
  - `crates/task-ops/src/knowledge.rs`: `commit_curation`（KB agent 作者の 1 commit。変更なしなら HEAD を返す）と `push_remote`（非 force・非対話 `BatchMode=yes`・時限付きの push。結果は `NoRemote` / `Pushed` / `Failed`）。
  - 試験 `crates/task-ops/src/knowledge/curation_git_tests.rs`（4 件）。remote は tempdir の bare repo のみ。
- **auto-apply（承認なし自動適用）**
  - `crates/celeris/src/knowledge_curation.rs`: `mode = apply` の計画は承認 decision を出さず、適用時に本番 KB を再検証してから適用し、1 commit、push。再検証・適用の失敗は `Stale` として適用しない。push の失敗は apply を失敗にしない（報告と event に残す）。
  - `Event::KnowledgeCurationApplied`（task-core）を追加。`docs/api/v1` を再生成し、gui・web の型と realtime の種類名・invalidation map を更新。
  - `curation-human` は従来どおり人へ出す。`request_approval` 系は撤去。
- **ops-docs（運用文書と予算）**
  - `docs/ops/cron-jobs.md` §6 を書き直し（承認なしの自動適用、1 commit と push、§6.2 切り替え、§6.3 救出手順）。§1・§2.1 も新方針に合わせた。
  - `config/celeris.example.toml` の `knowledge-curation` の budget を `max_turns = 120`（旧 30）に。1 回 40 件の上限は不変。本番 `config.toml` は変えていない。

## main の取り込み

- `git merge --no-ff main`（main は HEAD の 7 commit 先）。衝突なし（`git merge-tree` で事前に確認して exit 0）。
- 取り込み後、`git merge-base --is-ancestor main HEAD` → exit 0。

## 証拠

- 条件 0（main が HEAD の祖先）: `git merge-base --is-ancestor main HEAD` → exit 0
- 文書検査 3 本（条件 1）:
  - `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0
  - `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (135 files)`、exit 0
  - `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0
- migration 番号の重複: `crates/*/migrations` を走査して重複なし（出力なし）。
- `cargo test --workspace`（条件 2・3）→ exit 0。集計 3858 passed / 0 failed（全 test result 行の合計）。
- `cargo clippy --workspace -- -D warnings` → exit 0（`Finished dev profile`、警告なし）。

## 未解決事項

- ADR 番号 0078 が 2 件ある（`agent-docs/adr/0078-browser-execution-capability.md` と `0078-ssh-master-persist-independent-of-daemon.md`）。両方とも main に既にあり、この merge で増えたものではない。check-adr-numbers は通る。番号を振り直すかどうかは人の判断（参照の更新が要る）。
- 本番 KB に private remote を足すこと、daemon UID で非対話に push できる認証を置くことは人の作業（`docs/ops/cron-jobs.md` §6）。remote が無いと push は `NoRemote` で止まり、commit は残る。
- 本番 release の昇格と daemon の再起動は人が行う。`config.toml` への harness 追記（budget 120）と `mode = apply` への切り替えも人の手順（`docs/ops/cron-jobs.md` §1・§6.2）。

## 提案

- ADR 0078 の重複は、後から入った側（`ssh-master-persist-independent-of-daemon`）を次の空き番号へ振り直し、参照を更新する案を人へ。検査のため番号を現状のまま残すなら、その旨を ADR-0128 の番号規則に書く。
