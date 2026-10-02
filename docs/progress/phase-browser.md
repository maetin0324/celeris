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

## 2026-09-28 追記: 最新 main の再統合と再検証

`7725ed6` による再統合後に main が `6fe2871`、さらに `06e9a03`（F5-fix7）へ進んだため、main を再度 merge した。
`EVENT_TYPES` は `browser_updated` と main の認可イベントをともに保持し、task 画面は Browser Live View と実行の形の操作をともに保持した。
GUI lint で検出した lazy import の重複を解消してから、以下を sandbox 外で再検証した。

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all -- --check` | exit 0 |
| Rust tests | `cargo test --workspace` | `06e9a03` の取り込み前は 2673 passed、取り込み後は exit 0、2678 passed / 0 failed / 7 ignored |
| Rust lint | `cargo clippy --workspace -- -D warnings` | exit 0 |
| GUI lint | `cd gui && pnpm lint` | 初回は重複定義 2 件で exit 1、修正後 exit 0 |
| GUI types | `cd gui && pnpm typecheck` | exit 0 |
| GUI tests | `cd gui && pnpm test` | exit 0、1173 passed / 0 failed |
| GUI build | `cd gui && pnpm build` | exit 0 |
| Mobile audit | `cd gui && pnpm mobile-audit` | exit 0、27 routes × 2 schemes、0 violations |

検証に使った依存関係は `cd gui && pnpm install --frozen-lockfile` で導入した。
release.sh / verify.sh の結果と最終 SHA は WorkUnit の `artifacts/release.md` に記録する。本番昇格は人が GUI で行う。

## 2026-10-02 追記: phase3_control の負荷下での繰り返し確認（task 01M3XSER5YCVRWJTCHP0XGB8AP、stress-verify WorkUnit）

`phase3_control_converges_rejects_competition_and_cancel_stops` の flaky（`fix-control-test`、commit
`dafffeb1`）の修正後ブランチ（HEAD `775251d7`）で、負荷下での再現性を確認した。

根因は daemon の tick 時刻依存ではなく、celeris が `SO_REUSEPORT` で bind するため並走する別の e2e テストが
同じポートを選べてしまい、別 DB の daemon に `agent/begin` が届くこと（詳細は `dafffeb1` のコミットメッセージ）。
修正は `tests/e2e/src/lib.rs` に `PortReservation` を追加して空きポートを予約し、他プロセスの bind(0) から
見えなくした。本 WorkUnit の役割は、その修正が負荷下で安定して通ることを確かめ、再現手順を
`scripts/dev/stress-e2e-phase3.sh` として残すこと。

### 負荷のかけ方

`scripts/dev/stress-e2e-phase3.sh`（dash 互換、引数なしで実行）:

1. `nproc`（本機では 24）本の `timeout <cap> sh -c 'while :; do :; done'` を並走させ CPU を飽和させる。
2. バックグラウンドで `cargo test -p task-dispatch --lib` を失敗を無視しながら繰り返し実行し、別クレートの
   test 実行・ビルドキャッシュ参照による負荷を足す。
3. 上記の負荷をかけたまま `cargo test -p e2e --test api_scenarios phase3_` を 20 回連続で実行（1 回でも
   落ちたら即 `exit 1` し、その回の出力を表示）。
4. 続けて同じコマンドを 8 プロセス同時に起動し、全プロセスの exit code を集計（1 つでも非 0 なら
   `exit 1` で各プロセスのログを表示）。
5. `trap` で CPU 負荷プロセスと `cargo test` 負荷プロセスを `EXIT INT TERM` で必ず kill・wait し、作業用
   一時ディレクトリも削除する。

いずれも `set -eu` + `trap cleanup EXIT INT TERM` で、途中で落ちても負荷プロセスが残らないようにしている。

### 実行結果（HEAD `775251d7`、修正後ブランチ）

`sh scripts/dev/stress-e2e-phase3.sh` を 2 回実行し、どちらも exit 0。

| 実行 | 結果 | 所要時間 | 備考 |
| --- | --- | --- | --- |
| 1 回目 | exit 0、20 serial + 8 parallel すべて ok | 2m43s（real）、user 47m38s | `time` で計測 |
| 2 回目 | exit 0、20 serial + 8 parallel すべて ok | 計測なし（ログのみ確認） | 1 回目と合わせ phase3_ グループ（4 試験）を 56 回分（(20+8)×2）負荷下で実行、すべて pass |

実行後に `ps aux` で CPU 負荷プロセス（`while :; do :; done`）が残っていないことを確認した（0 件）。

### 修正前 commit での再現

`fix-control-test` の親（`7f3482a3`、SO_REUSEPORT の修正前）での再現は、この run では行っていない。
理由: 修正者が `dafffeb1` のコミットメッセージに、使い捨ての実験テストで「同じポートに 2 つ目の celeris を
起こし begin 直後の状態を読むと 20 回中 7 回 in_flight=0」という再現記録をすでに残しており、根因（ポート
衝突）も製品コードの該当箇所（`bind_reuseport`、ADR-0040 D4）も特定済みだったため、同じ検証を別 worktree で
繰り返すコストに見合わないと判断した。必要なら `git worktree add <path> 7f3482a3` で親 commit を取り出し、
同じ `scripts/dev/stress-e2e-phase3.sh` を走らせれば再現確認できる。

### 試験の意図への影響

`scripts/dev/stress-e2e-phase3.sh` はテストのコードやアサーションを一切変更していない（既存の
`cargo test -p e2e --test api_scenarios phase3_` をそのまま繰り返し・並走させるだけ）。収束前 takeover の
`not_converged` 拒否・競合の `not_lease_holder` 拒否・`cancel` での停止という試験の意図は変更していない。

### stress-verify run 01M3XVJ3Y7R5H0MX9AK3BXRNZH の追記（修正済み）

2026-10-02 の再確認では `time sh scripts/dev/stress-e2e-phase3.sh` が exit 1（real 43.217s）となった。原因は
`cargo test -p e2e --test api_scenarios phase3_ --no-run` が e2e の試験バイナリしか生成せず、fixture が起動する
`target/debug/celeris` / `celerisctl` を用意しないため、まっさらな target では全試験が `celerisctl not found` で
落ちることだった。

### fix-stress-build WorkUnit（01M3XSER5YCVRWJTCHP0XGB8AP）での実行結果

人の介入で共用 host の CPU 負荷を抑えるため、台本の既定を焼き 2 本・300 秒・serial 5 回・parallel 2 本にした。
並走 e2e 数も `STRESS_E2E_PHASE3_PARALLEL` で指定する。全負荷と e2e は `nice -n 19` で起動し、task-dispatch
cargo 負荷は既定で無効、必要な場合だけ `STRESS_E2E_PHASE3_CARGO_LOAD=1` で有効にする。background shell の
`EXIT` trap を明示的に解除し、親が所有する一時 directory を子が消さないようにした。workspace bin の build は
serial loop より前に行う。

検証前の `unshare -U -r true` は `unshare: write failed /proc/self/uid_map: Operation not permitted`（exit 1）。
指定された一度だけの既定 stress 実行 `time sh scripts/dev/stress-e2e-phase3.sh` は exit 1。workspace bin と e2e
試験バイナリの build は成功したが、serial 1/5 で fixture の `celeris` が ADR-0095 worker DB guard の namespace
probe に失敗し、4 件の phase3 試験がすべて起動前に落ちた。user namespace が無効な sandbox では試験結果を得られず、
重い条件での追加 stress は行っていない。

stress-e2e-phase3 結果: exit 1（serial 0/5、parallel 0、real 17.894s）

### 人に依頼する重い負荷の検証

user namespace が使える専用環境で、焼き本数と時間を増やして一度に検証する。例:

```sh
STRESS_E2E_PHASE3_PARALLEL=8 STRESS_E2E_PHASE3_LOAD_SECONDS=1800 STRESS_E2E_PHASE3_ITERATIONS=20 time sh scripts/dev/stress-e2e-phase3.sh
```

`exit 0` と `all 8 concurrent processes: ok` を確認する。必要なら別途 `STRESS_E2E_PHASE3_CARGO_LOAD=1` を付けて
task-dispatch の cargo 負荷も有効にする。これは CPU を長時間使うため、共用 host では実行せず、専用または空いている
環境で行う。
