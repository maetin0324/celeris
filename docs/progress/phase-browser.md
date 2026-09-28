# Browser capability Phase 1

---
tasks: [01M3MBV3AKXZGEG5RXR60XC62J, 01M3MFS5T52FXA63W4V10XGC4S]
---

2026-09-28、[ADR-0078](../adr/0078-browser-execution-capability.md) の Phase 1 を実装。
運用手順は [browser-capability.md](../browser-capability.md)。本番の profile 設定と昇格は行っていない。

- 既存 profile の管理者 grant と `browser-enabled` skill の要求を分離。ACP/OpenCode を既定に、Claude Code を明示選択できる。
- task/run ごとの isolated session、非空の upstream action allowlist、domain policy、content boundaries を生成。
- supervisor の lifecycle と秘密を含まない操作監査、task artifacts 登録を追加。raw harness logs / RPC error反射を抑止。
- task/run GUI から既存 dashboard を開く。token URL を保存せず、実行終了後のリンクも無効化。旧 API では browser panel を省略する。
- browser agent loop、DOM/ref解決、画面転送は既存部品を利用。CredentialBroker、認証state再利用、durable wait、直接stream統合、container/egress境界は後続 Phase。

固定版 agent-browser 0.38.1 の実機 smoke で navigation/click/extract/screenshot/download、危険操作拒否、session 間の storage 分離を確認。
別 namespace の既存 dashboard も Playwright で live canvas を確認し、検証用 session/dashboard は終了した。
Rust は routing・profile・実APIのfilter/page・supervisor・raw log非露出、Python はCLI/secret sentinel、GUI は URL/旧API/終了stateを検証する。
最終 workspace gate、release/verify の機械向け証跡と検証済み SHA は task artifacts の result.json/gate.json/verify.json に保存する。

Phase 1 は公開・未認証ページ向け。同一 UID の任意 shell を隔離するものでも、任意 Web content/最終 summary の秘密を自動検出するものでもない。
機密業務への適用前に ADR の後続 policy/broker/隔離タスクと人間 decision を完了する。

既存の組織プロフィール編集は grant を保持する。初回 release の mobile audit で task タブの初期 JS が 533.1KB となり
532KB の既存予算を超えたため、browser panel は session がある場合だけ遅延ロードする。予算値は変更しない。

## 2026-09-28 追記: main への取り込み（task 01M3MFS5T52FXA63W4V10XGC4S）

上記 Phase 1 の commit（`996920d`/`ee40596`）は、`gui: run ログを会話形式で表示する（ADR-GUI-0013）`
（`87d3bae`）を含むブランチへ merge した。衝突は `gui/app/routes/tasks.$id.runs.$runId.tsx` の import
文 2 箇所のみ（`useMemo`/`CopyButton` と `lazy`/`Suspense`/`activeBrowserRunIds` の並記）で、run ログ
会話表示と browser Live View 導線の両方を残して解消した。詳細な手順とコマンド結果は WorkUnit の
`artifacts/adopt.md` にある。`gui`（lint/typecheck/test 1154件/build）と Rust（fmt 2 回収束・clippy・
`cargo test --workspace` 再実行で 0 failed・関係 crate 個別実行）・`scripts/tests/test_browser_cli.py`
（単体 10 件 OK）を確認済み。本番へは未昇格。

## 2026-09-28 追記: MVP 受け入れ監査（task 01M3MFS5T52FXA63W4V10XGC4S）

固定版 agent-browser 0.38.1 の実機で、policy ファイルが無い・壊れている・`allow: []` のとき `eval` が成功する
（fail-open）ことを確認した。shim（`browser_cli.py`）は呼び出し前に生成 policy（`default: deny`、非空で既知 action のみ）と
非空の `allowed_domains` を検査し、満たさなければ substrate を起動せず `policy_block` を記録する。
Python 単体 12 件、`scripts/browser-smoke.py` 実機 26 checks、`gui/scripts/browser-check.mjs` を確認した。

## 2026-09-28 追記: 完了 gate（task 01M3MFS5T52FXA63W4V10XGC4S、release WorkUnit）

完了日: 2026-09-28。統合後の HEAD（`2031861` = adopt/design-gap/mvp-audit の merge 後）で sandbox 外実行。

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| fmt | `cargo fmt --all -- --check`（2 回） | 2 回とも exit 0、差分なし |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| Rust test | `cargo test --workspace` | exit 0、2639 passed / 0 failed / 7 ignored（初回で成功、再実行なし） |
| GUI | `pnpm install --frozen-lockfile` → `pnpm lint` / `typecheck` / `test` / `build` | すべて exit 0、test 75 files / 1154 passed |
| shim | `python3 scripts/tests/test_browser_cli.py` | 12 tests OK |

release.sh / verify.sh の結果と検証済み SHA は WorkUnit の `artifacts/release.md` に記録する。本番へは昇格しない（人が GUI で行う）。

### 未解決事項（ADR-0078 D8）

- Phase 2: task policy と admin grant の交差（P2-A）、CredentialBroker の 1 provider 実装（P2-B）、承認・認証待ちの durable wait（P2-C）。
- Phase 3: project/origin 限定の Browser Identity（P3-A）、task 別 ACL 付き live view proxy（P3-B）、pause/takeover/resume/stop（P3-C）。
- Phase 4: container/別 UID と egress 境界（P4-A）、broker→injector の強い注入（P4-B）、Codex/Browser Use/browser-specialist への backend routing（P4-C）。
- 既知の限界: Phase 1 は公開・未認証ページ向けで、同一 UID shell を隔離しない。dashboard は operator 専用運用が前提。

### 人の決定点

- credential backend の選択（既存 vault 優先、無ければ専用 1Password vault が初期候補）、lease の承認頻度、認証区間の観測制限（Phase 2 着手前）。
- persistent identity の範囲と保存期間（Phase 3 着手前。個人 Chrome profile の共用は避ける）。
- 機密 task に使う前に P4-A（container/egress）を前倒しするか。
- 本番の profile へ `browser` grant を付けるか、および本番昇格（GUI）。

### 提案

- Phase 2 は P2-A（policy 契約）を単独 task として先に起票し、backend 決定を待たずに進める。
- agent-browser の版上げ時は `scripts/browser-smoke.py` の fail-open 負例（policy 欠落・破損・空 allow）を必ず再実行する。
