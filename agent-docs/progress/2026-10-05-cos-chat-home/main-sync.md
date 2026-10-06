---
title: CoS チャットホーム main 追従
tasks: [01M46VVAD0ZAVZ9C4Q0KJM9ESV]
status: done
updated: 2026-10-06
---
# CoS チャットホーム — main-sync WorkUnit

`main` の `2a167a3f` を `git merge --no-ff main` で取り込んだ。並行 web task の下部タブ・最初の画面（`d56869d4`）、アカウント残量表示（`f79a134d`）、成果物表示（`bc7ff505`）はいずれも取り込み元の祖先に含まれる。

唯一のテキスト衝突は `web/components/content/artifact-preview.test.tsx` の import だった。既存の overlay timeout 指定と、新しい `artifactKind` の試験を両方残した。自動マージされた `web/e2e/support/fake-daemon.mjs` は task 件数 fixture と成果物の範囲取得を保持し、生成 schema の参照先 `RoutingAudit` が存在することを確認した。`docs/api/v1/api-v1.schema.json` と `web/api/generated/schema.json` は同一である。

## 検証

| コマンド | 結果 |
|---|---|
| `pnpm -C web typecheck` | exit 0 |
| `pnpm -C web test` | exit 0、Vitest 59 files / 363 tests と server 47 tests が成功 |
| `pnpm -C web e2e` | exit 0、243 passed / 8 skipped |
| `git diff --check` | exit 0 |
| `cmp docs/api/v1/api-v1.schema.json web/api/generated/schema.json` | exit 0 |

追加で `sh scripts/dev/progress-index.sh --check` を実行したところ、取り込んだ `main` の `2026-10-05-web-artifact-viewer.md` と `2026-10-06-web-tabbar-first-screen.md` の front matter に対して exit 1 になった。これらは並行 task の進捗ファイルで、この WorkUnit では変更していない。

この WorkUnit は main 追従を担当する。CoS の継続 run と新しいホームの実装・検証は後続 WorkUnit が担当する。
