---
title: "launcher 経路の browser CredentialUse 解放（統合後の検証と取りまとめ）"
tasks: [01M4FRN13Z206RAWBZ0NHZQEGX]
status: done
updated: 2026-10-09
---

# launcher credential 解放: 統合後の検証（close-out）

- 完了日（コードと試験の検証）: 2026-10-09
- 完了日（人の決定の記録）: 2026-10-09（ADR の「人の決定（2026-10-09）」節、未解決事項 5）
- 対象 HEAD: `8d0d52b1`（integrate wu/ledger-fixture）。`ops/ledger-fix2`（3d2ea2f5・679cab36）は祖先に入っている。
- 決定の根拠: ADR [2026-10-09-browser-launcher-credential-release](../adr/2026-10-09-browser-launcher-credential-release.md)（人の決定 2026-10-09）。付記は ADR-0116・ADR-0138・ADR-0080 にある。
- 段ごとの記録: [`2026-10-09-browser-launcher-credential-release/`](2026-10-09-browser-launcher-credential-release/)（adr、merge-ledger-fix2、launcher-credential、ledger-gen、ledger-fix、ledger-wiring、ledger-fixture、ops-runbook）

## 結論

- コードと試験は統合後 HEAD で通る。clippy、試験 target の compile、nextest 全体（4937 件）、doc-test はすべて exit 0。
- 文書検査 4 本のうち 3 本が通る。`check-architecture-map.py` だけが 3 件で落ちる。原因は launcher の変更ではなく、`main` にある行の記述であり、詳細を後述する。
- 残りは運用セッションの作業である。host の必須モード（stutter 3 回）、merge 後 HEAD での `ADMISSION[real-session]` の再取得、本番での解放は、まだ行っていない。

## 証拠（統合後 HEAD `8d0d52b1`）

作業ツリーは clean のまま、検査だけを行った。

| 条件 | 実行したコマンド | 出力の要点 |
|---|---|---|
| clippy が警告ゼロ | `cargo clippy --workspace -- -D warnings` | exit 0、`Finished dev profile` |
| 試験 target が compile する | `cargo check --workspace --tests --keep-going` | exit 0、error 0 |
| 全体試験（nextest + doc-test） | `TMPDIR=/var/tmp bash scripts/dev/test-parallel.sh` | exit 0、`passed 4937, failed 0, ignored 14`、`doctest_exit 0`、`tmp_leftovers 0` |
| 文書の配置 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | `check-doc-layout: ok`、exit 0 |
| 文書のリンク | `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok`、exit 0 |
| ADR 番号の衝突 | `sh scripts/dev/check-adr-numbers.sh` | `ok (172 files)`、exit 0 |
| 試験の一時 dir の残り | `TMPDIR=/var/tmp sh scripts/dev/check-test-tmp-leftovers.sh -p celeris --lib` | 415 passed、`no leftovers`、exit 0 |
| アーキテクチャ索引 | `python3 -I scripts/dev/check-architecture-map.py` | exit 1、下記の 3 件 |

生成ログは `artifacts/` の `clippy.log`、`check-tests.log`、`test-parallel-final.log`、`tmp-leftovers.log` にある。

close-out の再確認（同じ HEAD `8d0d52b1`、2026-10-09）:

- `cargo clippy --workspace -- -D warnings`: exit 0（キャッシュ由来の `Finished`）。
- `TMPDIR=/var/tmp bash scripts/dev/test-parallel.sh`: exit 0、`CELERIS_TEST_SUMMARY` は `passed 4937, failed 0, ignored 14, nextest_exit 0, doctest_exit 0, tmp_leftovers 0, summary_parsed true`。
- `check-doc-links.sh`・`check-adr-numbers.sh`（172 files）・`progress-index.sh --check`・`check-doc-layout.sh`: すべて ok、exit 0。
- `ops/ledger-fix2`（3d2ea2f5・679cab36）は HEAD の祖先。

front matter の `status` を `verified-open-host` から `done` に直した（progress-index の許容値は running・done・blocked・abandoned のみ）。`done` は code 側の close-out が完了したことを指す。host 実証と本番解放は下の未解決事項として残す。

### 試験の途中経過（失敗の原因）

1. 既定の run TMPDIR（93 文字）で全体試験を流すと、71 件が失敗した。原因は Unix socket の path が `SUN_LEN`（107 バイト）を超えることだった（`path must be shorter than SUN_LEN`）。
2. `TMPDIR=/var/tmp/cu-closeout`（19 文字）でも 4 件が残った（`task-api::browser_e2e`）。`test-parallel.sh` は `TMPDIR` を `$TMPDIR/celeris-test-parallel.XXXXXX/tmp` に差し替えるため、テストの control socket の path が約 111 バイトになり、bind が失敗していた。この 4 件は単独実行では 2 回とも合格した。
3. `TMPDIR=/var/tmp` にすると、socket の path が上限内に収まり、全件が合格した。

「全体試験の TMPDIR が長いと browser の socket 試験が落ちる」問題は、この run でも再現した。実装の不具合ではなく、検査の入れ物の問題である。

## 文書検査の未解決事項

`check-architecture-map.py` が次の 3 件で落ちる。
- `<workspace>/runs/<run_id>/tmp/`
- `check-test-tmp-leftovers.sh`
- `test-parallel.sh`

これらは `docs/architecture-map.md` の「試験の一時 dir の後片付け」の行にある。この行は `main` にも同じ内容で入っており、checker も `main` と同一である（`git diff main HEAD` で差分ゼロ）。よって、本 task の変更が原因ではない。解消には、表の記述を実在のパス（`scripts/dev/...`）に直すか、checker が `<workspace>` を扱えるようにする必要がある。今回は実装をしない方針のため、直していない。

## 未解決事項（運用セッションの作業）

1. host の必須モード実証: `CELERIS_LAUNCHER_TESTS=require` で `crates/task-worker/scripts/launcher-admission-evidence.sh --stutter 3` を 3 回流し、各回で SIGSTOP/SIGCONT を 2 回ずつ入れる。手順は [docs/ops/browser-launcher-admission-evidence-run.md](../../docs/ops/browser-launcher-admission-evidence-run.md)（人が host で実行する）。
2. merge 後 HEAD での `ADMISSION[real-session]` の再取得。同じ手順書による。
3. 台帳の credential 証拠（launcher runtime 分）の生成。`release.sh` / `browser-ledger.sh` の段は `ledger-wiring` で配線済みだが、host 上の実データでは未確認。
4. 本番での解放（`[browser]` の設定変更と daemon の差し替え）。手順は [docs/ops/browser-launcher-credential-release.md](../../docs/ops/browser-launcher-credential-release.md)。本番操作は人が行う。
5. 人の決定の記録は [ADR 2026-10-09-browser-launcher-credential-release の「人の決定（2026-10-09）」節](../adr/2026-10-09-browser-launcher-credential-release.md) に移した。`preconnect-meaning` は子 task `01M4FS9M260F7VBBPANZCF8M23` の計画で出され、2026-10-09 に運用セッションが a（二段 gate）と回答した。この task の `confirm-two-stage-gate` は 2026-10-09 に「承認する（推奨どおり）」と回答された。二段 gate は launcher-credential の段で実装済みである。

## 提案

- `test-parallel.sh` は、run の TMPDIR を子の一時 dir に差し替える前に、短い専用 dir を使うか、差し替え後の path 長を検査して明確な理由で止まるようにする。今回のような 71 件の失敗（実際の原因は path 長）を見分けやすくできる。
- `docs/architecture-map.md` の試験の後片付けの行を、実在のパスで書き直す。`check-architecture-map.py` の `<workspace>` 扱いも決める。
