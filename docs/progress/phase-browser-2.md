# Browser capability Phase 2

---
tasks: [01M3MZKB3DFYJNBH015MJGQ0BT]
---

完了日: 2026-09-28。契約は [ADR-0080](../adr/0080-browser-phase2-policy-broker-approval.md)（Phase 1 は ADR-0078、[phase-browser.md](phase-browser.md)）。
本番の昇格は行っていない（人が GUI で行う）。

## 実装したもの

- **policy 生成（D1）**: admin grant ∩ task policy ∩ backend が対応する action から実効 policy を作り、agent-browser の action policy（内部 action 名の空でない allow と default deny）と allowed-domains を生成する。空集合・未知の action・壊れた schema は起動前に固定コードで Err を返す。shim は許可外の action と許可外 host への open を substrate に渡す前に止める（否定テスト: `evaluate`・`cookies_get`・`state_save` が allow に入らないこと、許可外 domain は拒否されること、改ざんした policy は拒否されること）。
- **credential broker（D2/D3）**: `crates/celeris-credentiald`。`CredentialProvider` 抽象の上に最初の provider として手動登録を実装した。XChaCha20-Poly1305 で暗号化して保存し、権限を検査する。control IPC と resolve IPC を分ける。lease は task・run・session・origin に束縛した一回限りのもので、監査も付ける。固定版 agent-browser の plugin bridge（`agent-browser.plugin.v1` / `credential.resolve`）を使う。LLM には success/failure だけを返す。秘密値の保存方法は ADR-0080 D3 に書いた。
- **wait と承認（D4/D5）**: migration 0031（`browser_waits`・`browser_credentials`・`browser_approvals`。秘密値の列は無い）。task は Blocked のまま、wait の reason として WAITING_FOR_AUTH/APPROVAL を持つ。作成・解決・期限切れ・cancel は task の遷移と同じトランザクションで確定し、version の CAS で重複を防ぐ。人の操作には bearer に加えて GUI 専用鍵の Ed25519 human attestation を要求する。
- **結線（e2e）**: 承認後の run は承認 wait を一度だけ消費し、credentiald に bind と grant を求める。origin が合わないときや失敗したときは lease を失効させ、再試行しない Error にする。認証後の区間では観測系の action を外し、Live View も止める。
- **GUI（D5/D6）**: task 画面の「ブラウザの人待ち」に登録フォームと承認・拒否の操作を置いた。本人 session・exact Origin・CSRF・wait の version を検査する。Live View は `/browser/live/:taskId/:runId` の本人 guard を通る経路だけにし、loader data と SSE から raw `live_view_url` を消した。relay が未検証のため、本人にも 503 `live_view_relay_unavailable` を返す（D6 の安全側動作）。詳細は `gui/docs/PROGRESS.md`。

## 証拠（release WorkUnit、2026-09-28、base 29d933f）

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all` を 2 回実行してから `cargo fmt --all -- --check` | exit 0、差分なし |
| Rust tests | `cargo test --workspace` | exit 0、2745 passed / 0 failed / 7 ignored（3m19s） |
| Rust lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| GUI types | `cd gui && pnpm typecheck` | exit 0 |
| GUI tests | `cd gui && pnpm test` | exit 0、77 files / 1194 passed |
| GUI lint | `cd gui && pnpm lint` | exit 0（295 files、info 2） |
| 端から端 | `cargo test -p task-api --test browser_e2e`（e2e WU） | 3 passed（成功、拒否から failed、origin 不一致から deny。sentinel は全走査） |

### release.sh / verify.sh（ADR-0040 D5、2026-09-29）

- 検証した SHA は `18ca76d57fce26d965349e49835d84813fb41459`（全体検査結果を記録したコミット。この節の追記は docs だけを変える）。
- `scripts/selfdeploy/release.sh 18ca76d57fce26d965349e49835d84813fb41459`: exit 0、gate.json は ok=true、schema_version=32（5m43s。GUI の typecheck/test/build/mobile-audit/e2e-mock を含む）。
- `SD_REPO=$HOME/workspace/agent-platform scripts/selfdeploy/verify.sh 18ca76d57fce`: exit 0、**ok=true、live_ok=false**。check 1〜4・4b（gui-e2e）・6（smoke）は true。check 5（n-1-compat）だけが false になる。本番の現行 5fcb7eebbe9a は schema 29 までしか扱えず、このリリースは 0030（cluster_connection_log）・0031（browser_waits）・0032（browser_task_policies）を適用して 32 にするため、`SchemaTooNew` で起動しない。これは決定的に起きることで、一過性の失敗ではないので再実行していない。
- 本番へは昇格していない。昇格すると N-1 の rollback ができない（schema 32 の DB を旧 binary が読めない）ことを、人は昇格前に承知しておく必要がある。

## 未解決事項

- Phase 3〜4 に回すもの: persistent auth（認証 state の再利用）、GUI への live stream 統合（読み取り専用 relay。現状では Live View は本人にも 503）、container/egress 隔離。
- `auth login` の lease 参照 flag、plugin 設定の形、daemon 経由の FD 3 継承は fake substrate でしか確かめていない。実 agent-browser での確認が必要である。
- GUI の control socket は Node から SO_PEERCRED を読めない。file mode（0700/0600）だけで守っており、同一 UID の相手は区別できない。

## 提案

- Phase 3 の最初に、実 agent-browser 0.38.1 で手動登録から auth login までを 1 回通す smoke を行い、fake との差を潰す。
- Live View relay（HTTP/WS/asset の guard と token bootstrap の除去）は、persistent auth より先に単独の task として起票する。
