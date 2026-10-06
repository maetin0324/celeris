---
title: 孤児 run の WorkUnit 回収
tasks: [01M47JRMM9JZ6YBM1E6384350C]
status: done
updated: 2026-10-06
---

# 孤児 run の WorkUnit 回収

run の中断時に WU の回収を共通化し、起動時・定期照合で終端/欠落 run の Running WU を修復した。
コメント割り込みで task が先に Ready になり工程 lease が消えても、checkpoint があれば
NeedsContinuation、なければ Ready に戻り、新しい run を配車できる。
検査中の Interrupt/Cancel も検査の停止と WU の回収を組にした。
store は新 run・replan・Cancel が先行した場合に古い回収を拒否する。

不変条件・対象・クラッシュ時の回復規則は
[ADR-0070](../adr/0070-task-failure-visibility-and-handoff-safe-runs.md) と
[ADR-0074](../adr/0074-parallel-work-units-checkpoints-milestones-quota.md) の 2026-10-06 付記に記録した。

## 検査結果（2026-10-06）

| コマンド | 結果 |
| --- | --- |
| `bash scripts/dev/test-parallel.sh` | exit 0。最終版: nextest 4,067 件合格、失敗 0。doctest 合格。既定の skip は計 14 件。 |
| `cargo clippy --workspace -- -D warnings` | exit 0、警告なし。 |
| `cargo fmt --all -- --check` | exit 0。 |
| `git diff --check` | exit 0。 |

追加した `dispatcher/tests/orphan_work_units.rs` の 9 試験も全体検査に含む。
一時 SQLite DB、偽 worker の開始通知、差し替えた時計で、コメント→孤児回収（v1/v2）、
終端記録と WU 回収の間の停止、run 索引/ID 消失（Running/Ready の両方）、pause/resume の各順序、
checkpoint 継続、新 run・replan・Cancel との競合、生きた daemon/run/検査・子 task の保護、
検査中の Interrupt/Cancel、定期回収の時刻境界を確認した。回復後に実際の新 run の開始まで検証している。
CPU 焼き負荷・実 LLM は使っていない。

生ログ: run 成果物ディレクトリの `test-parallel-final.log`、`clippy.log`、`format.log`、`diff-check.log`。
終了コードは同じ場所の `validation-final.json` と `validation.json` にも記録した。
本番 DB・daemon・サービスの変更は行っていない。

## 配置修正の検査（2026-10-06、attempt 2）

ADR-0128 に従い本ファイルを `agent-docs/progress/` へ移し、必須の `title` を補い、
ADR-0074 と本ファイルの相対リンクを更新した。

| コマンド | 結果 |
| --- | --- |
| `sh scripts/dev/progress-index.sh --check` | exit 0。全対象ファイルの front matter 合格。 |
| `sh scripts/dev/check-doc-links.sh agent-docs/progress/2026-10-06-orphan-work-unit-recovery.md agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md` | exit 0。変更文書のリンク合格。 |
| `git diff --check` | exit 0。 |

この再試行は文書のみの変更。`b1916ad8` からコード・依存関係の変更がないことと、
上記の全体テスト・clippy の成功ログおよび終了コード記録を確認したため、両検査は再実行していない。
