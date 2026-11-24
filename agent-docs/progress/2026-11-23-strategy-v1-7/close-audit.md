---
title: 戦略層 v1 子 7 close-audit — ADR の文と保証の表の監査・LoC・負の試験の再現・task の進捗
tasks: [01M7XQ9R1EHA8GQ0TZVRM4FBVX]
wu: close-audit
status: done
updated: 2026-11-24
completed: 2026-11-24
---
# close-audit（WorkUnit。実装しない。変えたのは ADR と進捗だけ）

完了日: 2026-11-24。task の進捗は [2026-11-23-strategy-v1-7](../2026-11-23-strategy-v1-7.md)。設計の正本は
[ADR 2026-11-23-project-observability-publish](../../adr/2026-11-23-project-observability-publish.md)。
基準は `git merge-base HEAD main`（task 全体の差分）と、この WU の `$CELERIS_WU_BASE`（この WU の差分）。

## したこと

1. ADR の監査（7 つの WU の進捗と code に照らした）。
   - 保証の表の試験名 74 件を `git grep -qE "fn <名前>\(" -- crates` で 1 件ずつ確かめた。74 件すべてが crates の 1 file に定義されていた。略記は 0 件。
     3 つの spec の path は表にあり、file も在る。表の名前は直していない（直す必要が無かった）。表の列の名を「予定の試験」から「試験」に変えた。
   - 実装していない挙動を実装済みと書いていた文を「未実装」に直した: D1.3 の O5・O7 の drill-down の `log`、O10 の `wall_secs` と壁時計の印、
     D1.4 の `artifact` の「上書きされた」の表示・`log`・`git` の `task_id`・`literature` の `path`・`chat` の `message` と引用の状態、
     D3.3 の「run が manifest と決定を出す」流れ（prompt・計画が無い）。D2.3 の tx の名前を実在の関数（`chat_message_post_as_also`）に直した。
     D4 に「人の login」の判定と gateway の 403 の置き換えの実装を書いた。
   - 予定の文を結果か未確認に直した: 冒頭の「source の変更は後の WorkUnit が行う」、未確認の「測定は close の WU か人が行う」と
     「web-overview の WU が確かめる」、WorkUnit の表の close の行（と、表の外に書いた file の一覧）。
   - 未確認に 6 件を足し、末尾に「付記（close-audit、2026-11-24）」を足した。
   - code で確かめた点: `Link::Log` は enum にあるが作る所が無い。`Link::Git` の `task_id` は常に `None`。戦略の文書の artifact の link の sha256 は空文字。
     O8 の引用の link だけが `state` を持つ。O12 は採否の無い案の message を数える。Web の概要の panel の chat の link は `search={{ thread }}` だけ。
2. LoC: base の版の `scripts/dev/source-size-report.py` と `scripts/dev/source-size-report.toml` を `git show` で `$TMPDIR` に取り出し、
   `--exclusive-classes --format json` を `--rev "$(git merge-base HEAD main)"` と `--rev HEAD` に当てた（2 回とも exit 0）。値は task の進捗の「## LoC」。
   json は run の artifacts（`loc-base.json`・`loc-head.json`。repo には置かない）。
3. 負の試験の再現: `signoff_actor(&row, by)?;` 2 行と `reviewer` の `Principal::Human` の照合を外し、
   `cargo nextest run -p task-api -p celeris-mcp -E 'test(/publish_signoff_|proposal_review_/)' --no-fail-fast` → exit 100、`35 tests run: 24 passed, 11 failed`。
   `git checkout --` で戻し、`git diff --quiet -- crates` → exit 0、同じ command → exit 0、`35 passed`。落ちた試験の名前は task の進捗の「## 試験」。
4. task の進捗 `agent-docs/progress/2026-11-23-strategy-v1-7.md` を作った（status: running。全試験の結果は close-verify が書き足す）。

## 検査

- ADR の表の check（task の受け入れ条件 1 の式）→ exit 0。名前 74 件、欠け 0、3 つの spec は表にあり file も在る、migration の差分 0。
- `cargo nextest run -p task-core -p task-ops -p task-api -p celeris-mcp -E 'test(/publish_manifest_|publish_signoff_|proposal_review_|project_overview_|cos_ops_registry_/)' --no-tests=fail`
  → exit 0、`79 tests run: 79 passed`（ADR を直す前の同じ code の HEAD。この WU は crates を変えていない）。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`・`sh scripts/dev/progress-index.sh --check`・`cargo fmt --all -- --check`・
  `cargo clippy --workspace -- -D warnings`・`bash scripts/dev/test-affected.sh` の結果は下の「提出時の HEAD の検査」。

## 提出時の HEAD の検査

提出時の HEAD で実行した。最後の commit（この節を書いた commit）は進捗の file だけを変えるので、code の検査はその前の HEAD と同じ対象である。
doc-links と progress-index は最後の commit の後にも実行した。

- `sh scripts/dev/check-doc-links.sh` → `check-doc-links: ok`、exit 0。
- `sh scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (481 files)`、exit 0。
- `sh scripts/dev/progress-index.sh --check` → `progress-index --check: ok`、exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（`test-affected: 試験対象の変更なし（docs のみ等）`、`repo_health_gate: all checks passed`、repo-health の自己試験 126 件 OK）。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` → 変えた path は ADR・task の進捗・この file の 3 件だけ。

## 範囲

- 変えた file: `agent-docs/adr/2026-11-23-project-observability-publish.md`・`agent-docs/progress/2026-11-23-strategy-v1-7.md`・この file。
- crates・web・scripts の差分は 0（負の試験の一時の書き換えは戻した）。

## 未確認・引き継ぎ

- 提出時の HEAD での 3 つの e2e の再実行は close-verify が行う（この WU は流していない）。
- 本番の大きさでの概要の応答時間は測っていない（本番の DB を読まない）。
- ほかは task の進捗の「## 未確認」。

## 提案

- 無し（task の進捗の「## 提案」に書いた）。
