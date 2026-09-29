# Phase browser-4: isolated runtime・trusted injection・backend routing（P4-A〜C）

---
tasks: [01M3Q49ZTST3XQ9DGF6AGNR0XG]
---

- 状態: **P4-A/B/C の受け入れは未完**。runtime 方式・H7 は人の回答を採用済み（ADR-0085）。旧 broker IPC の秘密返却を廃止。実 runtime・CDP sink・backend 適合は継続実装が必要。本番未昇格。
- 更新: 2026-09-29
- ADR: [ADR-0084](../adr/0084-browser-phase4-isolation-injection-routing.md) D6、[ADR-0085](../adr/0085-browser-phase4-runtime-selection.md)

## 行ごとの判定

| 行 | 判定 | 証拠・限界 |
|---|---|---|
| P4-A isolated runtime | 未達。egress transport は実 socket / 独立プロセスまで追加 | `cargo test -p task-worker browser_egress --lib` → 9 passed、`cargo test -p task-worker --test browser_egress_process` → 6 passed。proxy が `check_egress` を呼ぶ。実 bwrap/subuid browser、namespace から唯一の出口への接続、runtime orphan 回収は未。proxy の親死亡 SIGKILL / waitpid 回収のみ実証。 |
| P3-A identity 復元（隔離下のみ） | 未達。API の契約テストだけ | `cargo test -p task-api restore_is` の前回結果は2 passed。`restore_isolated` は稼働中 runtime に未結合。`--restore` / `--state` / `--profile` は利用しない。 |
| P4-B stronger injection | 未達。攻撃の純関数テストだけ | `cargo test -p celeris-credentiald injection` の前回結果は6 passed。実 CDP sink / IPC peer UID role は未接続。旧 plugin bridge と resolve.sock 自体を固定拒否に変更。有効 lease を持つ別 worker process の実 IPC 拒否を検証。実注入は未達。 |
| H3 観測停止の維持 | 実装維持。機密起動は拒否 | `cargo test -p task-worker browser --lib` → 31 passed。`browser_auth_section_forward_events_drops_progress_artifact_and_live` と LiveEmitter の抑止試験を含む。API 結合テストの store auth_section / takeover 拒否も成功。実注入中の end-to-end 検証はP4-A/B待ち。 |
| P4-C backend routing | 一部接続。公開操作の既存 loop 再利用と未適合機密要求の拒否 | worker browser 関連31件と `cargo test -p task-api --test browser_e2e` → 4 passed。`CredentialUse` を `CredentialInjection` 必須能力へ写像し、wait操作・承認消費・起動より前に拒否する。`IdentityRestore` にも `InjectionAttackSuite` を要求。静的 fixture 登録は実 backend の適合証拠ではなく、specialist・実 fixture runner・同一task実評価は未。 |

## attempt 3 の挙動と検査

- 公開操作だけの policy は ACP / 明示 Claude の既存 loop で実行する。`CredentialUse` を含む effective policy は、承認状態を問わず `browser backend lacks required conformance` で拒否する。task が公開操作を行うには管理者がその policy を明示する。要求能力を黙って落とさない。
- API 結合テストは Phase 2 から残った durable wait を store に作り、実API・brokerによる手動登録と一回承認後も worker が起動しないことを確認。承認は未消費、broker の grant/use は0件、run directoryも作らない。DB/WAL・vault・event・worker出力の sentinel 非露出も確認する。
- 旧 trusted-local の認証成功・redirect 中の注入・認証区間の注入成功を期待するテストは、現在拒否する挙動に合わせて更新した。新しい拒否テストを P4-B の TOCTOU/redirect/iframe/DOM 実攻撃試験や隔離実行の証拠として数えない。H3 の production `auth_section` と `forward_events` の配線は変更していない。
- `cargo test -p task-worker browser --lib` → exit 0、31 passed。
- `cargo test -p task-api --test browser_e2e` → exit 0、4 passed。
- `cargo test --workspace` → exit 0（2957 passed、0 failed、既存のignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。workspace 検査に isolation 14件、backend 7件、injection 6件、auth_section 名の7件を含む。
- 初回の sandbox 内実行は sccache 接続が EPERM。Celeris の環境変数は変更せず、sandbox 外で同じ検査を実行する。結合テストの `owner_id` 型エラーを修正し再実行済み。

## 人の回答反映と旧 IPC 拒否（run 01M3QCTV524JJ41X9MSFS0756V）

- runtime は bubblewrap + subuid/subgid、host trusted controller/proxy の分離を採用。H7 は固定 agent-browser 0.38.1 + 既存 harness の browser-specialist を採用。方式・候補の回答待ちは解消し、再決定要求は出していない。
- 旧 `resolve.sock` は `trusted_injection_required` を返す。control 許可 PID、有効 binding/lease、JSON の `role: injector` 自己申告、同じ UID の別 worker process からも秘密を返さない。grant 2件に対し use 0件で lease 未消費を確認する。旧 bridge は socket に接続せず固定失敗を返す。
- API の実手動登録・承認・grant 後の旧秘密取得成功テストも拒否へ更新した。`CredentialProvider` と broker 内部の単回 lease の契約・試験、H3/auth_section 実装は変更していない。これらの拒否は実 trusted injection 成功や実CDP攻撃試験を意味しない。
- 初回の限定テストは sandbox 内 sccache が EPERM。環境を変更せず sandbox 外で実行。control 拒否前に request を読まない変更では Unix socket の未読データによる切断が `Io` となったため、既存の bounded read を維持して固定応答を返すよう修正した。
- 最終検査: `cargo test --workspace` → exit 0、2957 passed / 0 failed / 既存 ignored 7件。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all --check` → exit 0。
- workspace 実行に `retired_plugin_bridge_never_connects_or_echoes_input`、`daemon_rejects_worker_secret_retrieval_even_with_valid_lease`、`invalid_control_process_id_is_rejected`、API の `real_broker_registration_keeps_sentinel_out_of_db_events_artifacts_and_worker_output` を含み、すべて成功。P4-A の実 runtime と P4-B の実 CDP 攻撃、P4-C の実 backend 適合は未実施なので、workspace 成功をそれらの達成証拠とはしない。

## 未解決

- P4-A: 実 bwrap/subuid browser 起動・事実採取・broker/CDP/IPC 分離・namespace と新 egress transport の接続・runtime orphan 回収・稼働中隔離 session への identity 復元。subuid mapping はこの run の親 user namespace の範囲外で EPERM。proxy 部分の成功だけで受け入れない。
- P4-B: P4-A の後、別 injection-only IPC の SO_PEERCRED role/session 認可・実 CDP sink・実攻撃負例・H3 の端から端の検証。旧 endpoint は再開しない。
- P4-C: 選択済み specialist と既存 loop を実 fixture runner/同一 task 評価へ接続し、能力を失わない fallback を実行経路で検査する。機密機能は P4-A/B の実適合まで拒否する。
- 3件の継続小タスクを run の `delegate.json` に提案した。P4-B は P4-A に依存し、公開能力の P4-C は独立。採用・実行・完了はこの run では確認できていない。提案を実装済みとして数えず、親の受け入れ条件0/1/2は未達のままとする。
- 内部 origin の追加、本番昇格、リモート実行は行っていない。


## 実 egress transport（run 01M3QDM7H5RYRZF2RNCARHQWX6）

- ADR-0086 を先に追加し、`task-worker::browser_egress` と独立実行 `celeris-browser-egress` を実装。既存の機密 routing 拒否・旧 resolve IPC 拒否・CredentialProvider・H3/auth_section は変更していない。
- proxy は controller から継承する接続済み AF_UNIX stream（FD 3）と stdin の JSON policy だけを受ける。host の listener は開かない。policy 入力 64 KiB / 5秒、HTTP header 8 KiB、DNS応答 16 KiB、setup 10秒、tunnel 300秒 / 各方向64 MiBで制限。stderr は固定拒否コード、stdout は空。proxy は no_new_privs / dumpable=0、親死亡 SIGKILL を設定する。
- CONNECT authority を DNS 前に拒否できるようにした。指定 resolver の TCP/53 へ直接照会し、A/AAAA の両方・ID/question・CNAME/owner・圧縮pointer・応答サイズを検査。全候補に public 判定を行い、検査済みIPへ直接connectする。proxy環境変数・OS再解決・上位proxyは利用しない。IPv6無効時に公開AAAAを含む回答も拒否するため、dual-stackサイトへの互換性は限定的。この初期の拒否境界は緩めていない。
- IANA registry を確認し、IPv4 の廃止済み6to4 relay、IPv6のdiscard-only・benchmark・documentation・SRv6等を拒否へ追加。IPv6有効時も通常のglobal unicast以外とspecial-purpose範囲を保守的に拒否する。出典と方針はADR-0086。
- 実Unix/TCP fixture 9件: DNS rebinding・private/v6・proxy auth/転送/本文header・DNS/DoT port・IP literal・非許可host・不正DNS/循環CNAME・TLS初期byte/half-close。実子プロセス6件: inherited socket必須・worker指定のprivate/proxy拒否・不正/過大policy・policy入力期限・peer切断終了・親死亡時SIGKILLとsubreaperのwaitpid回収。すべて外部ネットワークを使わない。
- `cargo test -p task-worker browser_egress --lib` → exit 0、9 passed。`cargo test -p task-worker --test browser_egress_process` → exit 0、6 passed。
- 初回検査でDNS未知recordの扱い、clippyのtest module配置と不要borrow、ハイフン付きbinary名のcompile-time env参照を修正。最後のものは既存credentiald試験と同じruntimeの環境参照へ変更した。親死亡試験はNoNewPrivsが継承されるケースを考慮し、dumpable=0による/proc/fd読取り拒否も待ってから親を終了する。reap後のPIDへsignalを送らない。
- sandbox外の `bwrap --unshare-user --unshare-net --ro-bind / / true` は exit 0。`unshare --user --map-auto --map-user=1 --map-group=1 id` は exit 1（newuidmap EPERM）。親uid_mapは `0:100000:1001, 1001:1001:1, 1002:101002:64534`、subuid/subgidの rmaeda 割当ては `165536:65536` で親の範囲外。管理者設定は変更していない。同UIDのbwrap成功を別UIDの実証として扱わない。
- P4-A全体、P3-A復元、P4-B実CDP/IPC、P4-C実fixture/specialistは未達。production workerは新proxyをまだ起動しない。既存browserのネットワークがこのproxyで制限されるとは主張しない。namespace接続・trusted controllerの運用配線と実適合が必要。

- このrunの最終検査: `cargo test --workspace` → exit 0（2972 passed / 0 failed / 既存 ignored 7件）。`cargo clippy --workspace -- -D warnings` → exit 0。`cargo clippy -p task-worker --all-targets -- -D warnings` → exit 0。`cargo fmt --all --check` / `git diff --check` → exit 0。機密機能の実適合・production接続の証拠ではない。
