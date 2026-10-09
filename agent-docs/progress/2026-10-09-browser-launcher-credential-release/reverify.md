---
title: "stutter 競合修正後 HEAD の再検証（reverify）"
tasks: [01M4FRFZ2MVCC9BZBR86VS71K5]
status: done
updated: 2026-10-09
---

# stutter 競合修正後 HEAD の再検証（reverify）

- 完了日: 2026-10-09
- 対象 HEAD: `ac737717`（`stutter 台本: 試験 group の終了と競合した signal（ESRCH）を失敗にしない`）。reclose 葉が差し戻しで取り消されたため、この HEAD で再検証した。
- 範囲: 検査と記録のみ。`crates/`・`docs/ops/` に差分は入れない（開始時 snapshot からの変更 path は無し、`CELERIS_WU_SCOPE_PATHS` 出力 0 行）。実装を直していない。

## 証拠（HEAD `ac737717`）

| 検査 | コマンド | 結果 |
|---|---|---|
| stutter 台本試験（3 回連続） | `sh crates/task-worker/scripts/tests/launcher-admission-evidence-stutter.sh` ×3 | 全回 exit 0。各回は success / failure / repeat 10 の各 `--stutter 3` を走り、`STUTTER[stutter-N]: stops>0`・末尾 `EXIT: 0` を確認 |
| launcher credential-request wait の試験 | `cargo test -p task-worker --lib launcher_credential_request` | 5 passed / 0 failed（`launcher_credential_request_` 接頭辞 5 件） |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`） |
| 試験 target の compile | `cargo check --workspace --tests --keep-going` | exit 0 |
| 文書のリンク | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| 文書の配置 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | `check-doc-layout: ok`、exit 0 |
| ADR 番号の衝突 | `sh scripts/dev/check-adr-numbers.sh` | `ok (172 files)`、exit 0 |
| 進捗 index | `sh scripts/dev/progress-index.sh --check` | `ok`、exit 0 |
| アーキテクチャ索引 | `python3 -I scripts/dev/check-architecture-map.py` | exit 1（3 件）。`main`（0ba92ea5）と checker・対象行の差分がゼロで同一に落ちる既知の既存問題（親進捗の「文書検査の未解決事項」参照）。本差分起因ではない |

stutter 台本試験の 3 回連続は、ESRCH 競合の再発（偽の失敗）を防ぐための再確認で、3 回とも exit 0。

## ADR 付記の確認

[ADR 2026-10-09-browser-launcher-credential-release](../../adr/2026-10-09-browser-launcher-credential-release.md) に、final review の差し戻し 2 件（launcher credential-request wait、stutter 台本）に対応する日付付きの付記が既に存在する（『付記 2026-10-09: launcher runtime の credential-request → WaitingForAuth wait（共有段）』と『付記 2026-10-09: stutter 台本の signal 競合（ESRCH）』）。実装と食い違わないことを確認した：

- `credential_wait`（`browser.rs:686`）と `shim_request_wait`（`browser.rs:725`）が 1 箇所で定義され、daemon 経路（`browser.rs:1985`）と launcher 経路（`browser_launcher_run.rs:932`）の両方が呼んでいる。
- stutter 台本は `kill -STOP -"$pgid" 2>/dev/null || break` の形（dash で `--` を使わない）で、ESRCH は loop を抜けるだけ（台本内の comment で理由を記載）。両台本の file mode は `100755`。

よって付記の追記は不要（既存の付記で差分を説明済み）。

## 未解決事項（運用セッションの作業）

1. host の必須モード実証: `CELERIS_LAUNCHER_TESTS=require` で `crates/task-worker/scripts/launcher-admission-evidence.sh --stutter 3` を 3 回流す（各回 `STUTTER[stutter-N]: stops>0`・末尾 `EXIT: 0`）。手順は [docs/ops/browser-launcher-admission-evidence-run.md](../../../docs/ops/browser-launcher-admission-evidence-run.md)（人が host で実行する）。
2. merge 後 HEAD での `ADMISSION[real-session]` の再取得。同じ手順書による。
3. 本番での解放（`[browser]` の設定変更と daemon の差し替え）。手順は [docs/ops/browser-launcher-credential-release.md](../../../docs/ops/browser-launcher-credential-release.md)。本番操作は人が行う。
