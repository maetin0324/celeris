---
title: CoS 全変更操作の領域 registry と網羅性試験（registry WorkUnit）
tasks: [01M4F24B0GAEVZQPP35830PA0F]
status: done
updated: 2026-10-09
---
# registry WorkUnit

完了日: 2026-10-09。registry の基礎のみ。全 API 登録完了ではない。

[ADR D2/D3](../../adr/2026-10-09-cos-operations-all-mutations.md) に従い、
`crates/task-api/src/cos/ops/{tasks,decisions,projects,admin,surface}.rs` へ ALLOWED・EXCLUDED・PENDING と dispatch を分離。
既存 12 操作の method/path/action と共有操作関数を維持した。
`ops/mod.rs` の固定 REGISTRIES を operations.rs が連結・照合する。領域の登録追加は各 file だけで行える。

| 領域 | ALLOWED | EXCLUDED | PENDING |
|---|---:|---:|---:|
| tasks | 5 | 1 | 16 |
| decisions | 5 | 0 | 6 |
| projects | 1 | 8 | 14 |
| admin | 0 | 8 | 37 |
| surface | 1 | 19 | 21 |
| 計 | 12 | 36 | 94 |

除外は 422 `cos_operation_not_allowed` の detail に ADR の理由コードと本文を入れ、rejected 行と監査 event を記録する。
除外・未知操作の本文を保存前に伏せ、元の本文による request_hash の冪等検査を維持。
除外の照合を instructed_by と body の領域検証より先に行う。
既存仕様どおり、同じ key/hash の再送は同じ rejected operation を 200 で返す。拒否には applied card を作らない。

網羅性試験は router の Rust AST と gui-api.md の endpoint 表の和集合を取得し、全行が
ALLOWED/EXCLUDED/PENDING の一方だけに入ることを検査する。BASE/format!・put_route 別名・引数名の差を扱う。
ADR の領域割当と除外理由の一致も検査する。PENDING が空であることはこの unit の条件にしない。

既存の領域試験を `tests/common/cos_ops.rs` の helper へ移し、直接の 422・rejected 行/event と
経由の applied 行/event/card を検査。POST/PATCH/PUT/DELETE と expected_revision を扱う。
cos-operator の SKILL.md と operations.md は 5 領域の節へ分け、操作表と ALLOWED の一致試験を維持した。

## 検証

すべて `repos/agent-platform` で実行。cargo の target は run の `$TMPDIR/cargo-target`。

| コマンド | 結果 |
|---|---|
| `cargo check -p task-api --tests` | exit 0 |
| `cargo test -p task-api --lib cos_ops` | exit 0、3 passed（網羅性・ADR 照合・除外 matcher） |
| `cargo nextest run -p task-api --lib --test cos_operations --test cos_operations_domains --test cos_ops_registry --test cos_auth --test cos_triage --test cos_live_fix_d2 --test cos_live_fix_d1 --test cos_chat_attach_pin --test cos_chat_triage_scenarios --test cos_triage_override --test inbox_notifications -E 'test(/cos_/)' --test-threads 4` | exit 0、59 passed。skill-table 一致と既存 CoS の回帰を含む |
| 旧 ALLOWED と各領域 ALLOWED の method/path/action 集合比較 | exit 0、12 tuples identical |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p task-api --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh`（`CELERIS_TEST_JOBS=4`） | 長い TMPDIR の SUN_LEN 超過で停止（exit 130）。短い symlink は既存 containment 検査が拒否するため停止（exit 130） |
| `cargo nextest run --workspace --no-fail-fast --test-threads 4`（短い実 TMPDIR を workspace 直下に用意） | credentiald の既存 fixture readiness timeout / browser_doctor Io を確認し停止（exit 130）。原因の全体切分けは verify へ引継ぎ |
| `cargo test --doc --workspace` | exit 0 |
| `sh scripts/dev/check-doc-links.sh` / `sh scripts/dev/check-adr-numbers.sh` / `sh scripts/dev/progress-index.sh --check` | exit 0 |
| `cargo fmt --all -- --check` / `git diff --check` | exit 0 |

## 後続

PENDING 94 行の監査付き実装、ctl-wrap、skill-docs、PENDING 空の verify は後続 WorkUnit。
領域実装の引継ぎと検証ログは workspace の artifacts に保存。本番 host・daemon・DB は変更していない。
