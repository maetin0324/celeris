---
title: リリースの説明（notes.json / notes.md）と昇格の要約（ADR 2026-10-04-release-notes）
tasks: [01M420NG5Y1FX1T6G3B2GDGKRW]
status: done
updated: 2026-10-04
---

# PROGRESS — リリースの説明と昇格の要約（ADR 2026-10-04-release-notes）

正本: [ADR 2026-10-04-release-notes](../adr/2026-10-04-release-notes.md)。

## Phase 1（完了 2026-10-04、単一 phase）

人の要望（2026-10-04）: リリースごとに何が入ったか分かるようにし、昇格に複数のリリースが入るならまとめて見たい。

### 入れたもの

- `crates/celeris/src/release_notes.rs`: `generate`（`base..sha` の first-parent を task 単位にまとめる。
  task は配送記録の head〜配送の `base`、`celeris/<ULID>` の branch 先端、merge commit の題から判別。
  題と要約は `GET /tasks/{id}`。直接 commit・migration・schema_version・ADR・config 例・gate の skip）、
  `aggregate`（current から対象までの全リリース。base を辿る・current で切る・中間リリース・task の重複排除）、
  markdown 描画、`HttpLookup`。LLM は使わない。
- API: `GET /releases` の `items[].notes` / `items[].promotion`、`GET /releases/{sha12}/promotion-preview`、
  `GET /deliveries`。schema・gui 型・web 生成物を再生成。
- `celerisctl release notes` / `celerisctl release preview`（store を開かない）。
- `release.sh`: gate.json の後で `$STAGE/bin/celerisctl release notes` → `notes.json` / `notes.md`（失敗は警告だけ）。
- `promote.sh`: 昇格前に preview をログへ、`promoted.json` に `included`（releases・tasks・complete）。
- GUI `/releases`: 各リリースの「このリリースの内容」と「昇格したら入るもの」。
- 文書: `docs/api/v1/gui-api.md`（§3.67a/§3.67b ほか）、`gui/docs/celeris-api-v1.md`（sync-gui-docs.sh）、
  `docs/ops/selfdeploy.md` §4d、ADR。
- 試験: `release_notes/tests.rs` 12 件（一時 git repo で task branch merge・早送り配送・直接 commit・migration・
  ADR・config、複数リリースの要約で task が 1 回）、`releases_api.rs` 2 件、`celerisctl` 5 件、
  `scripts/selfdeploy/tests/release_notes_promote.sh`（15 項目）。
- main e0faa46d を取り込み（gui-gate-regression の修正で mobile-audit / e2e:mock が通る）。

### 証拠

| コマンド | 結果 |
|---|---|
| `cargo test --workspace` | exit 0、3851 passed / 0 failed / 13 ignored |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check` | exit 0（fmt 適用後） |
| `cargo test -p celeris release_notes` | 12 passed |
| `cargo test -p celeris --test releases_api` | 10 passed |
| `sh scripts/selfdeploy/tests/release_notes_promote.sh` | exit 0、all ok |
| gui `pnpm typecheck` / `test` / `build` | exit 0 / 1298 passed / exit 0 |
| gui `pnpm mobile-audit` | exit 0、routes=28 schemes=2 violations=0 |
| gui `pnpm e2e:mock` | exit 0 |
| `celerisctl release notes --repo . --sha e0faa46d --base 6ceec985 --api http://127.0.0.1:7710 …`（本番 API への読み取り GET） | tasks=52 direct=0 migrations=8、題と要約入り（本番 daemon は `/deliveries` を持たないので branch 名で判別） |

### 未解決事項

- `POST /releases/{sha12}/promote` は `current` に同梱の promote.sh を起こすので、ログと `promoted.json.included` はこの変更を含むリリースが current になった次の昇格から効く。`notes.json` は次の release.sh から作られる（それより前のリリースは「説明なし」）。
- `GET /deliveries` は新 daemon から。昇格前のリリースでは配送記録が読めず branch 名だけで task を判別する（`deliveries_known: false`）。
- deliveries 表は task ごとに最新の配送だけを持つので、同じ task の古い配送の区間は別 task に寄ることがある。
- gui の生成型で `ReleasePromotionPreview1` / `ReleaseNoteSchema1` という別名が出る（構造は同じ。gen-types の命名）。
- 本番への昇格・設定変更はしていない。

### 提案

- 配送時に head と task id を git notes などで repo 側にも残せば、deliveries 表の上書きに依らず task を引ける。
