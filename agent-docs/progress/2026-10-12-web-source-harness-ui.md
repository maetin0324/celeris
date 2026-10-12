---
tasks: [01M4R2CDTXZ4N7XZ8SN2M8NHEH]
status: done
title: web のプロバイダ画面を消し source・harness の画面にする（ui-v2）
updated: 2026-10-12
---

# web のプロバイダ画面を消し source・harness の画面にする（ui-v2）

ADR `agent-docs/adr/2026-10-11-llm-source-harness-selection.md` の「API と画面」節に従い、web/ の `/providers` を
読み取り専用の `/sources`・`/sources/$sourceId`・`/harnesses` に置き換えた。旧 `gui/` の同じ provider 画面と
provider API の呼び出しも消した。`crates/` は変えていない。ADR の 2026-10-12 付記（ui-v2）に実装範囲と残りを書いた。

## 各 unit の記録

| unit | 内容 | 記録 |
|---|---|---|
| gui-purge | 旧 gui の `/providers` 画面・provider API 呼び出し・試験を消す | `agent-docs/progress/2026-10-12-web-source-harness-ui/gui-purge.md` |
| web-purge | web の provider 前提（assignment 画面・providers query key・`/api/providers`）を消す | `agent-docs/progress/2026-10-12-web-source-harness-ui/web-purge.md` |
| gui-fixtures | 旧 gui の e2e 用 config fixture を新形（`[llm_sources]`・`[harnesses]`・`[[llm_routes]]`）へ移す | `agent-docs/progress/2026-10-12-web-source-harness-ui/gui-fixtures.md` |
| web-screens | `/sources`・`/sources/$sourceId`・`/harnesses` の読み取り画面、nav・台帳・偽 daemon | `agent-docs/progress/2026-10-12-web-source-harness-ui/web-screens.md` |
| run-selection | run 詳細に選ばれた harness・source・model・口座を並べて出す | `agent-docs/progress/2026-10-12-web-source-harness-ui/run-selection.md` |
| closeout | 文書の `/providers` 記述の更新、ADR 付記、parity 台帳 R30 の修正、全検査 | この文書 |

## closeout で行った変更

- `docs/frontend/UX_AUDIT.md`: `/providers` の画面節を「削除済み」にし、before の記録だと明記。台帳の `/providers` 行を `/sources`・`/sources/$sourceId`・`/harnesses` の行に置き換えた。before の friction 文の `/providers` 参照に「（削除済み）」を付けた。
- `docs/frontend/DESIGN.md`: 管理系の route 一覧を `sources`・`harnesses` を含む 8 route に直した。
- `agent-docs/web/feature-parity.md` R30: `/providers` の行を後継（`/sources`・`/harnesses`）の行に直した。確認方法の試験を `web/e2e/parity/sources-harnesses.spec.ts` の `parity: /sources /harnesses 一覧・詳細を読み取り専用で表示する` に合わせた。
- `web/e2e/admin/sources-harnesses.spec.ts` を `web/e2e/parity/sources-harnesses.spec.ts` へ移し、試験の題を `parity: ` で始めた。`check:parity` は `web/e2e/parity/` の題だけを見るため。
- `agent-docs/adr/2026-10-11-llm-source-harness-selection.md`: 「2026-10-12 付記（ui-v2）」を追加（実装した画面、読み取り専用であること、残り）。

## 証拠コマンドと結果

前回までの記録に加え、この run（01M4R62NHP2BAK8201WY488RN3）で次を再実行した。

| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| 範囲 check | 範囲 check の cmd を `CELERIS_WU_SCOPE_PATHS` 無しの git 差分で実行し、許可外の path を filter | out=[]（exit 0） |
| parity | `corepack pnpm@12.6.0 run check:parity` | exit 0 |
| web | typecheck・test・build・check:boundaries・check:secrets | 各 exit 0。test は 91 files・654 tests 通過 |
| gui | typecheck・lint・build・fixture（`bash gui/scripts/check-fixture-configs.sh`） | 各 exit 0。fixture は 9 件 ok |
| gui test | `corepack pnpm@11.27.0 run test` | 長い TMPDIR で 1 件失敗（上の提案の socket path 長）。`TMPDIR=/local/x/ui` で 92 files・1307 tests 通過 |
| 文書検査 | check-adr-numbers・check-doc-layout・check-doc-links・progress-index --check | 各 exit 0 |
| crates | `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/` | exit 0（差分ゼロ） |

作業ディレクトリは worktree（`TMPDIR=/local/x/ui`）。`CELERIS_WU_BASE=7e6f8344f3417ffb002e7049e1a840b27472ab88`。

| 条件 | 実行したコマンド | 結果 |
|---|---|---|
| 受け入れ 0 | `grep -rn "/providers\|api/v1/providers" docs/frontend agent-docs/web/feature-parity.md \| grep -v 削除済み`、`grep -rln "api/v1/providers\|/providers" web gui/app gui/test gui/e2e gui/scripts` | 現行の記述 0 件（残る `/providers` は「削除済み」の注記と before 記録だけ） |
| web typecheck | `corepack pnpm@12.6.0 run typecheck` | exit 0 |
| web lint | `corepack pnpm@12.6.0 run lint` | exit 0 |
| web test | `corepack pnpm@12.6.0 run test` | exit 0。Test Files 91 passed (91)、Tests 654 passed (654) |
| web build | `corepack pnpm@12.6.0 run build` | exit 0 |
| web 境界・秘密・parity | `run check:boundaries`、`run check:secrets`、`run check:parity` | 各 exit 0（`check:parity` は R30 の題の欠落が解消） |
| web 移動した試験 | `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/parity/sources-harnesses.spec.ts` | exit 0。1 passed |
| gui typecheck | `corepack pnpm@11.27.0 run typecheck` | exit 0 |
| gui lint | `corepack pnpm@11.27.0 run lint` | exit 0 |
| gui test | `corepack pnpm@11.27.0 run test` | exit 0。Test Files 92 passed (92)、Tests 1307 passed (1307) |
| gui build | `corepack pnpm@11.27.0 run build` | exit 0 |
| 文書検査 1 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（ok, 182 files） |
| 文書検査 2 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（ok） |
| 文書検査 3 | `sh scripts/dev/check-doc-links.sh` | exit 0（ok） |
| 進捗の索引 | `sh scripts/dev/progress-index.sh --check` | exit 0（ok） |
| crates 差分 | `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/` | exit 0（差分ゼロ） |

`cargo clippy` と `scripts/dev/test-parallel.sh` は流していない（この unit は crates を変えていない）。

## 未解決事項
- 前回の範囲 check 不合格（web/e2e/admin → web/e2e/parity の移動）は解消。移動は中身を変えず、試験の題を台帳 R30 の確認方法に合わせた。範囲 check は両 path を許し、`git diff`・未追跡の一覧から out=[] を確認した。移動を web-screens の commit に入れるかは人が決める（提案の節）。

- 編集 API と編集画面は未実装。`/sources`・`/harnesses` は読み取りだけで、source と harness の変更は config で行う。ADR の「source と harness の編集画面は別にし」は未達。
- `GET /api/v1/llm/sources` の catalog 欄（source が持つ model 一覧と利用可否）は未実装（api-v2 からの持ち越し）。
- run の `adapter`・`model` 欄（`RunSummary`）と timeline・console の `worker_started` ラベルは、まだ adapter 由来の値を読む（run-selection の未解決）。
- gui/ の e2e 用 celeris fixture を新形へ移した後の e2e（Playwright、実 celeris 起動）は、この環境では全件を流していない。gui-fixtures の記録では `check-fixture-configs.sh` が 9 fixture を通している。
- web の `event-kinds.ts` に残る `provider_throttled`・`model_role_assignment_changed` は、生成 schema の Event enum に残っているため消していない（web-purge の未解決）。Rust 側で variant を消す作業の後に消す。
- web の `models-screen` の tier 表示・`celeris_tiers`（fake daemon）は ADR の決定 2（tier alias 撤去）の対象で、crates の変更を待つ。
- `gui/` の `accounts.tsx` の `provider-tier-models-heading`・`providerLlmSourceDisplay` は provider の語を残す内部名・表示。機能の provider 画面ではないので残した。

## 提案

- gui の `browser-phase2.test.tsx` 「the local control socket」は、TMPDIR が長い run 用 path（`runs/<id>/tmp`）だと Unix socket path の 108 byte 上限で `listen EINVAL` になり 5 秒で落ちる。短い TMPDIR（`/local/x/ui`）では 19/19 通る。試験の tmpdir を短くする直しは gui の別 unit で行う（この unit では gui の試験を変えない）。

- 編集 API の要否: 人が source の model 制約・harness の適格条件を web から変えたいなら、`PATCH /api/v1/llm/sources/{source_id}` と harness の編集 API を別 unit で切る。変えるのが config だけで足りるなら、この読み取り画面で完了とし、編集 API は作らない。どちらにするかは人が決める。
- 次の unit: run の `RunSummary.adapter`・`model` を `selection` へ寄せ、timeline・console のラベルも `selection` から出す（adapter 撤去の残り）。
- R30 の `/providers` の行は後継の試験（`parity: /sources /harnesses ...`）で閉じた。`check:parity` の廃止行の扱い（gate が廃止行を数えるか）は、web-purge の提案のとおり人が決める。
