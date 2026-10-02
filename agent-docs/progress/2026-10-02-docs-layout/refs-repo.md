---
title: docs 再構成 — CLAUDE.md・.claude・scripts・architecture-map・web/gui の参照追従（refs-repo）
tasks: [01M3YBGM64RYPEY9NZANF79A0M]
status: done
updated: 2026-10-02
---
# docs 再構成 — CLAUDE.md・.claude・scripts・architecture-map・web/gui の参照追従（refs-repo）

完了日: 2026-10-02。移動（move-docs）後の新配置に、リポジトリ側の生きた参照を合わせた。crates/ は触っていない（refs-crates・refs-worker の範囲）。

## 変えたもの

- `CLAUDE.md`
  - 最初に読むもの: `docs/SPEC.md` → `agent-docs/README.md` → `docs/architecture-map.md` → `agent-docs/adr/` → `sh scripts/dev/progress-index.sh`。
    `docs/DESIGN.md`・`docs/PROGRESS.md` の行を外した（人の決定 design-md）。
  - 作業の進め方: ADR は `agent-docs/adr/YYYY-MM-DD-<slug>.md`（D5）、進捗は task ごとの進捗ファイル（並列 WU は `<slug>/<wu-key>.md`、D3）、
    提案は進捗ファイルの「提案」節、試験規則は `agent-docs/guides/testing.md`。
  - 禁止: 「DESIGN.md の書き換え」の行を外した。DESIGN.md §1 の設計原則のうち CLAUDE.md に無かったもの
    （状態は SQLite でワーカーはステートレス、完了はレビュー／決定的な検査が決める、events は追記専用）を 1 行ずつ足した。
    協調判断に LLM を使わない（既存の「ディスパッチャやストアに LLM 呼び出しを入れる」）、Phase 順（既存の「次の Phase の準備を先回りしない」）は既にある。
    SPEC.md の扱いは変えていない（書き換え禁止の対象にはしていない）。
- `.claude/agents/auditor.md`・`implementer.md`: DESIGN.md の参照を SPEC.md・関係 ADR・CLAUDE.md の禁止事項へ。
- `docs/architecture-map.md`: `adr/…` を `../agent-docs/adr/…`、`progress/…` を `../agent-docs/progress/…`、`selfdeploy.md` を `ops/selfdeploy.md`、
  web の implementation plan を `../agent-docs/web/` へ。DESIGN 節へのリンクは対応する ADR（0001・0002・0003・0004・0005・0006・0008・0010・0013・0020・0026・0027）と SPEC へ置き換えた。
- `scripts/`: selfdeploy（`docs/ops/selfdeploy.md`・`docs/ops/web-parallel-operation.md`、`SD_SENSITIVE_PATTERNS` の ADR-0040/0041 を `agent-docs/adr/`）、
  `sync-gui-docs.sh`（正本を `docs/api/v1/gui-api.md` に。写しへの名前置換と、正本と同じディレクトリへの相対リンクの向け直し）、
  `check-browser-acceptance.sh`、`check-model-routing.mjs`（出力先 `agent-docs/gui/model-routing`）、`rdc/`、`dev/test-parallel.sh`・`source-size-report.toml`、
  `dev/check-architecture-map.py` の注記、`dev/check-doc-links.sh`（`check-adr-numbers.sh` を移行期間の対象外に追加。旧 ADR ディレクトリを名指しするため）。
- `web/`: `check-parity.mjs`（と試験）が読む表を `agent-docs/web/feature-parity.md` に。コメントの参照、`api/generated/schema.json` の説明文
  （`docs/api/v1/gui-api.md`・`docs/guides/knowledge.md`）。
- `gui/`: コメントの参照（`docs/api/v1/gui-api.md`・`docs/guides/mcp.md`・`agent-docs/adr/…` など）、`check-run-log.mjs` の既定出力先の書き方、
  `gui/docs/celeris-api-v1.md`・`gui/docs/adr/0002-frontend-stack.md` の壊れたリンク。
- `docs/api/v1/gui-api.md` 3 行目の壊れたリンク（`../celeris-api-v1.md` → `overview.md`）。
- `docs/README.md`・`agent-docs/README.md`: 一覧を最終の配置に合わせ、旧 `docs/adr/`・`docs/progress/` が移行期間の案内だけであること、検査 3 本を書いた。

## 証拠

- `sh scripts/dev/check-doc-links.sh CLAUDE.md .claude scripts web gui docs/architecture-map.md docs/README.md agent-docs/README.md` → `check-doc-links: ok`、exit 0（作業前は 194 件）
- `sh scripts/dev/check-doc-links.sh --self-test` → ok、exit 0
- `python3 scripts/dev/check-architecture-map.py` → `OK: 187 件のパスを確認した`、exit 0（作業前は 37 件 NG）
- `sh scripts/dev/check-adr-numbers.sh` → ok (113 files)、`sh scripts/dev/progress-index.sh --check` → ok
- `python3 -m unittest discover -s scripts/tests -p 'test_*.py'` → 43 tests OK
- `bash -n` で selfdeploy・sync-gui-docs.sh・gui/scripts/celeris.sh、`node --check` で変更した .mjs の構文を確認

## 未解決

- `web/api/generated/schema.json` は `docs/api/v1/api-v1.schema.json`（crates の doc comment から生成）の写しで、`web/scripts/gen-types.mjs --check` が一致を見る。
  この branch では web 側だけ新パスに直したので、refs-crates が Rust の doc comment を同じ新パス（`docs/api/v1/gui-api.md`・`docs/guides/knowledge.md`）に直して
  schema を再生成するまで、単独では一致しない。integrate-refs で `node web/scripts/gen-types.mjs --check` を確かめる。
- `gui/docs/celeris-api-v1.md`（GUI 側の写し）はこの task 以前から正本とずれている（`sh scripts/sync-gui-docs.sh --check` は差分あり）。API 説明の整理は cleanup-api に任せ、ここでは同期していない。
- web・gui の node_modules が無いので vitest（`web/test/scripts/check-parity.test.ts`）は走らせていない。
- crates に変更が無いので `cargo test --workspace`・`cargo clippy` はこの WU では走らせていない（統合の段で走る）。

## 提案

- 移行期間が終わったら `check-doc-links.sh` の `MIGRATION_EXCLUDE` から `scripts/dev/check-adr-numbers.sh` も外し、同台本の旧ディレクトリの列挙を消す。
