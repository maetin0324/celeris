# Browser capability の main 統合と ADR 番号対応

---
tasks: [01M3VSNWDCD1TD5KAG3MCVZFE0]
---

Browser Phase 1〜4 のブランチ `478e86c4` にあった Phase 3/4 の ADR は、リファクタ後の main が使用した 0081・0082・0083・0089 などの番号と衝突していた。main の既存 ADR と参照を維持し、browser 側の 16 文書を main の最大番号 0098 の後に振り直した。旧番号 0087・0088・0094 は browser ブランチ内でも各 2 文書に重複していたため、ファイルごとに対応を示す。

| 旧番号 | browser 文書 | 新番号 |
|---|---|---|
| 0081 | [Phase 3 control lease](../adr/0099-browser-phase3-control-lease.md) | 0099 |
| 0082 | [Phase 3 live proxy ACL](../adr/0100-browser-phase3-live-proxy-acl.md) | 0100 |
| 0083 | [Phase 3 identity contract](../adr/0101-browser-phase3-identity-contract.md) | 0101 |
| 0084 | [Phase 4 isolation/injection/routing](../adr/0102-browser-phase4-isolation-injection-routing.md) | 0102 |
| 0085 | [Phase 4 runtime selection](../adr/0103-browser-phase4-runtime-selection.md) | 0103 |
| 0086 | [Browser egress transport](../adr/0104-browser-egress-transport.md) | 0104 |
| 0087 | [P4-A same-UID bwrap runtime](../adr/0105-browser-p4a-same-uid-bwrap-runtime.md) | 0105 |
| 0087 | [P4-C conformance dispatch](../adr/0106-browser-phase4-conformance-dispatch.md) | 0106 |
| 0088 | [Fallback candidate preparation](../adr/0107-browser-fallback-candidate-preparation.md) | 0107 |
| 0088 | [P4-A relay/supervisor/restore](../adr/0108-browser-p4a-relay-supervisor-launch-restore.md) | 0108 |
| 0089 | [P4-B injection IPC/CDP sink](../adr/0109-browser-p4b-injection-ipc-cdp-sink.md) | 0109 |
| 0091 | [P4-B H3 shared CDP/trusted selector](../adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md) | 0110 |
| 0092 | [P4-B redisplay guard](../adr/0111-browser-p4b-redisplay-guard-wiring.md) | 0111 |
| 0093 | [P4-B conformance evidence/unlock](../adr/0112-browser-p4b-conformance-evidence-unlock.md) | 0112 |
| 0094 | [P3-C control gate/action server](../adr/0113-browser-p3c-control-gate-action-server.md) | 0113 |
| 0094 | [P4-A restore deliver state](../adr/0114-browser-p4a-restore-deliver-state.md) | 0114 |

Phase 1/2 の browser ADR-0078 と ADR-0080 は既存の番号を維持する。main に元からある ADR-0078 の重複は、この統合作業の対象外。main 由来の ADR-0081（web SPA）、ADR-0082（dispatcher 分割）、ADR-0083（source size）、ADR-0089（CoS 並列度）などの参照も維持する。Browser の機密能力や本番設定の扱いは [Phase 4 記録](phase-browser-4.md)と[追跡表](phase-browser-acceptance.md)を参照。

## 統合後の検証（2026-10-01）

統合ブランチで `git merge-base --is-ancestor 478e86c484267834efe5e103479e07c214b2f0d1 HEAD` と `git merge-base --is-ancestor 2eb1b03027b1 HEAD` はともに exit 0。`test -z "$(git grep -nE '^(<<<<<<<|=======|>>>>>>>)' -- crates gui docs scripts)"` も exit 0。browser と main の双方が履歴に残り、衝突マーカーはない。

- **既定無効・本番設定**: `crates/celeris/src/config/mod.rs` の `Config.browser` は `#[serde(default)]`、`BrowserRuntimeConfig.egress.resolver` は `Option` で既定 `None`。`Config.api.browser_site_policies` も空が既定で、`browser_settings_default_to_unconfigured_and_site_policy_validates` が空 TOML を検査する。さらに `CELERIS_BROWSER_CONFORMANCE_FILE` がなければ conformance ledger はなく、`released_ledger_still_refuses_unconformant_backend_and_unisolated_runtime` は resolver/runtime 不足を拒否する。従って新しい欄を本番 TOML に足さなくても旧設定は解析でき、既定では browser の実行経路は有効にならない。本番で browser を使う前には、運用者が egress resolver、適合記録、必要な site policy と credentiald 接続を評価して明示設定する必要がある。本番の実設定・環境変数はこの作業で検査・編集していない。
- **機密能力は本番では未解放**: `sensitive_declaration_requires_p4a_and_p4b_conformance` は `CredentialInjection` と `IdentityRestore` の P4-A/P4-B 証拠欠落を拒否する。`credential_use_is_released_only_by_p4b_evidence_in_the_ledger` は試験用の完全な ledger なら解除可能な条件を示すが、証拠が欠ければ拒否する。`released_ledger_still_refuses_unconformant_backend_and_unisolated_runtime` は未適合 backend と隔離 runtime 不在を拒否する。`credential_injection_sameuid_rejected_in_production`（credentiald）・`identity_restore_sameuid_rejected_in_production`（worker）・`restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal`（API）は同一 UID の実 session を `SameUid` / `isolation_required` として拒否する。既定構成ではこの二能力を解放できず、別 host UID 実証と本番での解放は未確認・後続である。これは機能コードの恒久禁止を意味しない。
- **P3-B frame は未達**: [受け入れ追跡表](phase-browser-acceptance.md) の P3-B-2/3 は「未達」。`crates/task-worker/src/browser_live.rs` の `LiveEmitter::emit` は frame を破棄し、`crates/task-worker/src/browser.rs` の `live_view_url` は `None`。frame の実配線と実 process 試験は追跡表の後続事項のまま。
- **DB migration**: `crates/task-core/src/store/migrations.rs` は main の `0034_cluster_job_waits.sql` の後に browser の `0035_browser_phase3_store.sql` と `0036_browser_trusted_login.sql` を割り当て、`SCHEMA_VERSION = 36`。本番 DB では検証していない。昇格時の schema 34→36 適用は運用側の手順で実施する。
- **source size**: `python3 scripts/dev/source-size-report.py --strict` は exit 0、active warning 0 件、`task-api/src/types.rs` の既存例外 1 件。統合で 300 行を超えた `task-api/src/browser_identity.rs` の inline test は `browser_identity/tests.rs` に外出しした。

検証コマンド・exit・テスト数は以下の通り。

| コマンド | exit | 結果 |
|---|---:|---|
| `cargo fmt --all -- --check` | 0 | 全ファイルの整形検査に合格 |
| `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib` | 101 | namespace が使えない sandbox で task-worker の 12 テストが `Operation not permitted`。schema 生成物は `git diff --quiet -- docs/api/v1 docs/protocol` で差分ゼロを確認。 |
| `UPDATE_SCHEMA=1 cargo test -p task-core -p task-api -p task-worker --lib schema` | 0 | 29 passed / 0 failed。生成後の `git diff --quiet -- docs/api/v1 docs/protocol` も exit 0。 |
| `cargo test --workspace` | 0 | 権限付きの namespace 実行で **3,206 passed / 0 failed / 12 ignored**（118 群）。上記の既定無効・機密拒否の引用テストも全て `ok`。初回は e2e の multi-account バリアが DB guard の読み取り専用 DB 親に作られて 60 秒待機に達したため、既存の書き込み可能な workspace 内へ移して再実行した。 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 警告 0 件。browser テストの型複雑度と mutex guard の保持を修正後に全 target で合格。 |
| `python3 scripts/dev/source-size-report.py --strict` | 0 | active warning 0 件、既存例外 1 件 |

未解決: P3-B frame、別 host UID の実証と `ptrace` を含む A13、機密能力の本番での適合、昇格・本番設定の整備。上記の試験環境の namespace 制約は権限付き再実行で切り分ける。

## release 準備（prepare）失敗の再現（2026-10-01、ref e768594c）

delivery の `scripts/selfdeploy/release.sh` が失敗したため、gate の step を `release.sh` と同じ順・同じコマンドで task の worktree（Celeris が渡した `CARGO_TARGET_DIR`）で実行した。共有の `.build/tree`、`~/.local/celeris/releases`、本番 service には触れていない。

| step（release.sh の名前） | コマンド | exit | 結果 |
|---|---|---:|---|
| cargo-fmt-check | `cargo fmt --all -- --check` | 0 | 差分なし |
| cargo-test | `bash scripts/dev/test-parallel.sh` | 0 | nextest 3,206 passed / 11 skipped、doctest exit 0（118 binaries） |
| cargo-clippy | `cargo clippy --workspace -- -D warnings` | 0 | 警告 0 |
| source-size-report | `python3 scripts/dev/source-size-report.py` | 0 | — |
| cargo-build | `cargo build --release -p celeris -p celerisctl -p celeris-credentiald` | 0 | 108 秒 |
| pnpm-install | `pnpm install --frozen-lockfile`（pnpm 11.27.0） | 0 | — |
| pnpm-typecheck | `pnpm typecheck` | 0 | — |
| pnpm-test | `pnpm test` | 0 | 84 files / 1,249 passed |
| pnpm-build | `pnpm build` | 0 | — |
| pnpm-mobile-audit | `MOBILE_AUDIT_SKIP_BUILD=1 timeout 600 pnpm mobile-audit` | 0 | routes=28 schemes=2 violations=0、111 秒 |
| pnpm-e2e-mock | `E2E_SKIP_BUILD=1 timeout 600 pnpm e2e:mock` | 0 | failures 0、12 秒 |

`cargo-workspace-clean` は共有 target 専用の段なので再現していない。全 step が通ったので、コードの修正は不要だった。

prepare 失敗が環境由来（共有 build tree の競合・中断）と見られる根拠:

- `prepare.log` は `pnpm-mobile-audit` までの 10 段がすべて `exit 0` で、`pnpm-mobile-audit` の開始行（17:56:07Z）で終わっている。`run_step` は失敗時も `step <name>: exit <rc>` を書き、gate が落ちると `.build/e768594c2d18/gate.json` を残すが、どちらも無い。release.sh が step の途中で外から止められたことを示す。
- 共有 tree の `.gate-pnpm-mobile-audit.log` は `$ node scripts/mobile-audit.mjs` の 1 行だけで、開始の約 1 秒後（17:56:08Z）に更新が止まっている。worktree では同じ段に 111 秒かかり、exit 0 だった。
- 同じ時刻帯に別 release `6ef01deff025` の directory が更新されている（17:58:55Z に `web/` が作られた）。共有の `.build/tree` と releases 配下を別の配送が使っていた。

あわせて、旧 ADR 番号の参照 2 箇所を直した: `crates/task-api/src/state.rs` の `ADR-0088 D5` → `ADR-0108 D5`（P4-A relay/supervisor/restore）、`crates/task-worker/Cargo.toml` の `ADR-0094 D2` → `ADR-0114 D2`（P4-A restore deliver state、D2 試験 admission）。browser は既定無効のまま、機密能力は解放していない。
