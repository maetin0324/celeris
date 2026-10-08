# ADR 2026-10-08: 本番で browser 実行を使える状態にする（適合台帳の release 組み込み・前提の gate・site policy の DB 正本化・task policy の自動付与・点検コマンド）

---
tasks: [01M4CDNAYX6J68WTX7SKF0DJ64]
---

- 日付: 2026-10-08
- 状態: 実装済み（2026-10-08。task 01M4CDNAYX6J68WTX7SKF0DJ64。突き合わせは末尾の付記）
- 関連: [ADR-0080](0080-browser-phase2-policy-broker-approval.md)（policy・broker・承認）、
  [ADR-0106](0106-browser-phase4-conformance-dispatch.md)（実測適合記録と fallback）、
  [ADR-0112](0112-browser-p4b-conformance-evidence-unlock.md)（P4-B 証拠と機密能力の解放）、
  [ADR-0116](0116-browser-launcher-implementation.md)（launcher）、
  [ADR-0138](0138-browser-prod-admission-confidential-release.md)（本番 admission）、
  [ADR 2026-10-05-browser-allowed-origins](2026-10-05-browser-allowed-origins.md)（origin 形式・browser-settings API）、
  [ADR 2026-10-07-browser-trusted-devices](2026-10-07-browser-trusted-devices.md)（owner session の端末）、
  ADR-0040 / ADR-0051（selfdeploy の release・verify・promote）、ADR-0070 D3（infra_requeue）、ADR-0095 付記 D-d（本番操作は人）

## 1. 文脈

人の指摘（2026-10-08）: manaba の課題監視 task（01M4CD11W51ADC4HHMMEKRQM71）が動かず、CoS に聞いても
どうすればよいか分からなかった。運用セッションの調べと、base `c8f0d4dd` のコードで確かめた事実:

- **台帳が本番に無い**: worker は `CELERIS_BROWSER_CONFORMANCE_FILE` を `std::env::var_os` で読む
  （`crates/task-worker/src/browser.rs` の `run_with_candidates`・`conformance_record_path`・`run_with_executable`）。
  本番の `celeris@.service` に該当の `Environment=` は無く、どの browser task も worker 起動直後に
  `AdapterError::Other("browser conformance record unavailable")` で返り、dispatcher はそれを
  infra 都合として `InfraRequeue`（`running → ready`、`consecutive_infra_requeues` を数える。
  `crates/task-dispatch/src/dispatcher/leases.rs`）で戻す。上限まで同じ失敗を繰り返し、LLM は一度も呼ばれない。
  人に見えるのは `infra_requeue: …` の文字列だけ。
- **台帳の形**: `{"schema":1,"source":"celeris-browser-conformance","results":[{backend_id,version,passed,evidence}]}`。
  `ConformanceLedger` は `deny_unknown_fields`。`version` は agent-browser の版（`SUPPORTED_VERSION = "0.38.1"`）だけで、
  celeris の版は記録されない。`task_core::browser_backend::certify` は `version` 不一致を `StaleVersion` にする。
- **台帳の作り方**: `scripts/browser-conformance.py`。
  - `--protocol-scripted`: 3 backend（`acp`・`claude-code`・`browser-specialist`）それぞれについて、Rust の本物の
    adapter（`AcpAdapter`・`ClaudeCodeAdapter`・`BrowserSpecialistAdapter`）を `cargo test -p task-worker --lib
    p4c_conformance_backend_protocol -- --ignored` で起動し、LLM CLI の代わりに台本
    （`scripts/browser-conformance-harness.py`）が protocol を話す。browser は**実 agent-browser 0.38.1**、
    相手は loopback の fixture server。`--fallback-scenario` で worker の fallback も実 browser で通す。
  - `--p4b-evidence <ledger> --p4b-backend <id>`: 実攻撃行列（`browser_injection_attacks`）と本番 H3 e2e
    （`browser_h3_injection`）を `cargo test` で走らせ、全件 `passed` のときだけ `injection_attack_suite`・
    `auth_section_observation_stop` を `passed` に入れる（ADR-0112）。これが無いと `CredentialInjection` が
    解放されず、`credential_use` を含む task（manaba の login）は routing で拒否される。
  - どちらも**ソース木と cargo が要る**（`--manifest-path <repo>/Cargo.toml`）。release の build worktree
    （`scripts/selfdeploy/release.sh` の `$BUILD`、同じ SHA）にはどちらもある。
- **release の流れ**: `prepare.sh <sha40> <dir>` が `release.sh`（7200 秒）と `verify.sh`（900 秒）を回す。
  `release.sh` は build worktree で gate（cargo build/test・gui・web）を通し、`$STAGE` に bin・gui・web・scripts を
  組み立てて `~/.local/celeris/releases/<sha12>/` にする。`promote.sh` は人だけが実行し、`celeris@<sha12>` を起こす。
  `celeris@.service` は `releases/%i/bin/celeris --config … --release %i` で動く。
- **site policy**: `[[api.browser_site_policies]]`（`crates/celeris/src/config/api.rs`）を daemon 起動時に
  `validate()` して `UnixCredentialBrokerControl.site_policies`（`Vec`）に写す（`crates/celeris/src/daemon/api.rs`）。
  変えるには config.toml を手で書いて daemon を再起動するしかない。
- **grant**: `PATCH /api/v1/org/{id}/browser-settings`（`crates/task-api/src/handlers/org.rs`）は
  `allowed_domains`・`credential_policy_ids`・`credential_identity_ids`・`harnesses`・`budget` を受けるが、
  `allowed_actions`（`credential_use` を grant に入れる欄）は受けない。`credential_policy_ids` が実在する site policy か
  どうかも見ない。
- **task policy**: `GET/PUT /api/v1/tasks/{id}/browser/policy`（`crates/task-api/src/browser.rs`）。PUT は admin、
  `draft`/`ready` のときだけ（`browser_task_policy_set`）。起票時に誰も付けないので、付け忘れると run 時に
  `BrowserPolicyRequired` で落ちる。`task_ops::retry::retry_task_with_execution` は `requirements`（
  `requirements.browser.allowed_domains`）は写すが、`browser_task_policies` の行は写さない。
- **前提の点検手段が無い**: `[browser] runtime`（`daemon`/`launcher`）・`launcher_socket`・`[browser.egress] resolver`・
  `api.browser_credentiald_control_socket`・`api.browser_attestation_public_key_file` が揃っているかは、
  run が `isolated_runtime_unavailable` などで落ちて初めて分かる。

ゴール: 人の作業を「web で owner session を立てる」「初回に manaba の ID と password を credential form に入れる」
「run 中の承認ボタンを押す」だけにする。それ以外の前提は release と web で済ませる。

## 2. 変えないこと（セキュリティ方針）

- `click`・`download`・`credential_use` は**使うたびに人の承認**（`BrowserAction::ALWAYS_APPROVED`）。
  この ADR のどの決定も standing approval（事前の包括承認・「今後ずっと許可」）を足さない。
  D4 の自動付与で task policy に `click`・`credential_use` が入っても、承認の要否は変わらない
  （`approval_actions` は `ALWAYS_APPROVED` と和を取るので、外す手段は無い）。
- 実効 policy = 保存 task policy ∩ `requirements.browser.allowed_domains` ∩ 担当 node の grant（ADR 2026-10-05）。
  自動付与・retry 引き継ぎは task policy を**作る**だけで、grant を広げない。
- 機密能力（`CredentialInjection`・`IdentityRestore`）は ADR-0112 の実測証拠つきの台帳からだけ解放する。
  手で書いた台帳・既定の適合は認めない（`source` が `celeris-browser-conformance` でない台帳は拒否のまま）。
- 本番 admission（ADR-0110 / ADR-0138: broker は `Attested` のみ、launcher の session 証明必須）は変えない。
- dispatcher・store に LLM 呼び出しを入れない。台帳の判定・gate・自動付与はすべて決定的な規則。

## 3. 決定

### D1. 適合台帳の生成経路・配置先・daemon への渡し方・作り直しの判定

**D1.1 実 LLM は要らない。protocol-scripted で足りる。実 agent-browser は必須。**

- 台帳が保証するのは「この release の adapter のコード・supervisor・policy・CLI shim が、固定版の実 browser と
  組み合わさって fixture の件を通す」ことで、LLM の賢さではない。LLM 側は protocol（ACP・Claude の stream-json）の
  話し方さえ本物なら、中身は台本で決定的に再現できる。ADR-0106 決定 4 もこの形を取る。
  したがって生成経路は `scripts/browser-conformance.py --protocol-scripted --fallback-scenario` の後に
  `--p4b-evidence … --p4b-backend claude-code --p4b-backend browser-specialist`（ADR-0112）を続けたものとする。
- `--scripted`（直接台本・harness protocol を通らない）は `source = celeris-browser-conformance-scripted` を書き、
  worker が拒否する。本番台帳には使わない。
- **実 agent-browser 0.38.1 は必須**（substrate の実測が台帳の意味そのもの）。host に無い・版が違うときは台帳を
  作らず、理由 `agent_browser_missing` / `agent_browser_version` を残す（D1.4）。
- **P4-B 証拠は userns と実 Chromium が要る**。release.sh は本番 host の daemon user で worker sandbox の外で動き、
  release gate はすでに `CELERIS_USERNS_TESTS=1` で userns 試験を流している。台帳の段もこの環境で回す。
  P4-B が通らなければ公開能力だけの台帳を置き（`passed` から 2 件を外したまま。ADR-0112 の fail closed）、
  `credential_use` を含む task は D2 の `ledger_lacks_credential` で止まる。
- **実 LLM での確認の扱い**: 台帳には入れない。本番昇格の後の実機確認（manaba の task を 1 回通す）は、
  manaba の資格情報が人のものなので**人が行う手順**とし、docs/ops の手順書（preflight 葉）に
  「owner session → task の起票（web）→ credential form → 承認ボタン」の順と、成否の見方
  （task 詳細の browser 節・受信箱・`celerisctl browser doctor`）を書く。認証が使える環境なら agent が
  loopback fixture に対して実 claude-code で 1 回走らせて証跡を進捗に残してもよい（CLAUDE.md・ADR-0009 P-34）が、
  受け入れの必須条件にはしない。

**D1.2 生成は release の段（release.sh の `browser-ledger` 段）。非 blocking。**

- `release.sh` は gate の後、`$STAGE` の組み立て中に `browser-ledger` 段を回す。中身は `scripts/selfdeploy/lib.sh` の
  関数 `sd_browser_ledger <build worktree> <out dir> <sha12>` に置き、release.sh と D1.5 の再生成台本が共有する。
  1. `agent-browser --version` を見る（無い・版違い → 理由を書いて段を終える）。
  2. `python3 $BUILD/scripts/browser-conformance.py --protocol-scripted --fallback-scenario
     --celeris-release <sha12> --output-dir <out>/browser.partial`（`CARGO_TARGET_DIR` は release の target）。
  3. `… --p4b-evidence <out>/browser.partial/conformance.json --p4b-backend claude-code
     --p4b-backend browser-specialist --output-dir <out>/browser.partial`。
  4. `<release>/bin/celerisctl browser ledger check --file <out>/browser.partial/conformance.json --release <sha12>`
     （daemon と同じ判定のコード。D1.4）で検査し、通れば `browser.partial` を `<out>/browser` に rename する。
  5. 結果を `<out>/browser/ledger-status.json`（`{ok, code, generated_at, agent_browser, backends, credential_backends}`）と、
     release の `manifest.json`／`gate.json` の `browser_ledger` 欄に書く。ログは `gate-logs/browser-ledger.log`。
- 段の上限は 1800 秒（`timeout --kill-after=30s`）。失敗しても release は失敗にしない（browser を使わない本番機能を
  巻き込まない）。失敗は D2 の gate で browser task だけを止め、理由は台帳の状態として見える。
- runner に `--celeris-release <sha12>` を足し、台帳に次の `generated_for` を書く（`schema` は 1 のまま、欄の追加）:
  `{"celeris_release":"<sha12>","celeris_sha":"<sha40>","agent_browser":"0.38.1","fixture":"p4c-local-v1","generated_at":"<RFC3339>"}`。
  worker の `ConformanceLedger` は `generated_for` を受けるようにする（`deny_unknown_fields` は残す）。

**D1.3 配置先は release dir: `~/.local/celeris/releases/<sha12>/browser/conformance.json`。**

- 台帳は「この release のコード」の測定結果なので、release と寿命を揃える。rollback（`previous` に戻す）でも
  その release の台帳がそのまま使われ、古い台帳が新しい release に流れ込む事故が構造上起きない。
- state dir（`~/.config/celeris` 等）に 1 つ置く案は、release ごとの版の突き合わせと切替時の書き換えが要り、
  promote の失敗・rollback で台帳と daemon の版がずれるので採らない。
- `promote.sh` は台帳が無くても昇格を止めない。昇格のログと `promoted.json` に `browser_ledger: {ok, code}` を写し、
  `GET /releases` 経由で web の release 一覧に出す（「browser の適合台帳: 未配置（agent-browser が無い）」）。

**D1.4 daemon への渡し方と古さの判定**

- daemon は起動時に台帳の path を決め、process 全体の設定として `task_worker::browser::configure_conformance(path)`
  で worker に渡す（`browser_credential::configure` と同じ形。`std::env::set_var` は使わない）。順:
  1. 環境変数 `CELERIS_BROWSER_CONFORMANCE_FILE`（試験・開発用の上書き。従来どおり）。
  2. `--release <sha12>` で動いているとき: `current_exe()` の `../browser/conformance.json`（= release dir。D1.3）。
  3. どちらも無い: 未配置。
- 判定は `task_worker::browser::ledger_status(path, expected_release, host_agent_browser)` 1 か所に置き、daemon・
  dispatcher の gate（D2）・`celerisctl browser ledger check`（D1.2）・doctor（D5）が同じ関数を使う。結果の code:

  | code | 条件 | 人に出す文 |
  |---|---|---|
  | `ok` | 下のどれにも当たらない | browser の適合台帳: 有効 |
  | `missing` | path が無い・file が無い | browser の適合台帳が未配置 |
  | `invalid` | 読めない・64 KiB 超・symlink・`schema`/`source` 不一致・backend 重複 | browser の適合台帳が壊れている |
  | `stale_release` | `generated_for.celeris_release` ≠ daemon の release（`generated_for` が無い旧台帳を含む。`--release` 無しの開発 daemon では見ない） | browser の適合台帳が古い（別の release 用） |
  | `stale_agent_browser` | `generated_for.agent_browser` か `results[].version` ≠ `SUPPORTED_VERSION`、または host の `agent-browser --version` ≠ 台帳の版 | browser の適合台帳が古い（agent-browser の版が違う） |
  | `agent_browser_missing` | daemon の PATH に agent-browser が無い | agent-browser が見つからない |
  | `no_conformant_backend` | `claude-code`・`browser-specialist` のどちらも公開能力で certify されない | browser の適合台帳に使える backend が無い |
  | `ledger_lacks_credential` | （task ごとの判定）task policy に `credential_use` があり、certify された backend のどれも `CredentialInjection` を持たない | browser の適合台帳に credential 注入の証拠が無い |

- 古さの判定に使う版は 3 つ: **celeris**（release の sha12。adapter・supervisor・policy のコードはすべてこれに含まれる）、
  **agent-browser**（host の実行ファイルの版）、**backend**（`backend_id` ごとの `results[].version`。現行は
  agent-browser の版と同じ値）。claude-code の CLI の版は台帳に入れない（protocol-scripted は CLI を使わないので、
  台帳が保証しない。CLI の互換は既存の adapter 試験と実機の run が見る）。
- 状態は daemon 起動時に計算し、以後は tick ごとに file の `(mtime, size)` だけを見て変わったら計算し直す。
  host の `agent-browser --version` は起動時と file 変更時と doctor（D5）のときだけ起動する（tick ごとに process を起こさない）。

**D1.5 作り直し**

- celeris が変われば新しい release で D1.2 が必ず回るので、通常は何もしなくてよい。
- release 時に agent-browser が無かった・P4-B が落ちた等で、release を作り直さずに台帳だけ作り直したいときは
  `scripts/selfdeploy/browser-ledger.sh <sha12>` を**人が**実行する（release dir への書き込みなので ADR-0095 付記 D-d の
  本番操作）。build worktree を `<sha12>` の commit に合わせて `sd_browser_ledger` を回し、`releases/<sha12>/browser/`
  を atomic に置き換える。daemon は D1.4 の mtime 監視で再起動なしに拾う。手順は docs/ops に書く。

### D2. 台帳が無い・古いときは、投入した browser task を理由つきで止める（infra_requeue を繰り返さない）

- **gate の位置**: dispatcher の dispatch の直前（lease・worker・LLM の前）。対象は worker が browser 経路に入るのと
  同じ条件（`requests_browser(skills)`・`kind == Execute`・planner/review の run でない）の run。tick の最初で
  `ready` になったものを止めるので、人から見ると「投入した直後に止まる」。
- **新しい trigger**: `Trigger::BrowserPrereqBlock { code }` — `Ready → Blocked`（gate で止めた）と
  `Running → Blocked`（gate の後に台帳が消えた等で worker が台帳の code を返した）、attempts 不変、
  `reason = "browser_prerequisite"`。worker の台帳由来の失敗（`browser conformance record unavailable|invalid`・
  `ledger_status` の code）は `InfraRequeue` に分類しない（dispatcher の分類表から外し、この trigger に回す）。
- **新しい event**: `Event::BrowserPrerequisiteBlocked { code, message }`（code は D2 の表の固定値。message は表の
  人向けの文。path・秘密は載せない）を遷移と同じ transaction で 1 回だけ追記する。止まっている間の tick では
  追記しない（同じ task・同じ code の再追記はしない）。再開時は `Event::BrowserPrerequisiteResumed { code }`。
- **再開**: dispatcher の tick で、`reason = browser_prerequisite` で `blocked` の task について gate を評価し直し、
  通れば `Trigger::BrowserPrereqResume`（`Blocked → Ready`、`reason = "browser_prerequisite_resolved"`、attempts 不変）。
  新 release の promote（新 daemon が有効な台帳で起動）や D1.5 の再生成の後、人の操作なしで動き出す。
  評価は cached の台帳状態の比較だけで、止まっている task が多くても process は起動しない。
- **受信箱**: `build_attention` に reason `browser_prerequisite` を足す。項目の文は event の message
  （例「browser の適合台帳が未配置。新しい release を作るか、docs/ops/browser-prod.md の手順で台帳を作る」）。
  CoS の受信箱 thread（ADR 2026-10-07-cos-inbox-thread-conversation）にも同じ文が入るので、CoS は「artifact を見ろ」ではなく
  この理由と次の手を答えられる。
- **API**: `GET /api/v1/browser/readiness`（読み取り、token のみ）が daemon の判定（台帳の code・path・
  `generated_for`・certify された backend・D5 の各項目）を返す。task 詳細（`TaskDetail`）の browser 節に
  `prerequisite: {code, message} | null` を足し、web は止まった理由を task 詳細の先頭に出す。
- 起票（`POST /tasks`）自体は拒否しない（release を作り直している最中でも task を積めるように）。
- D4 の `browser_policy_missing`（task policy が無く自動付与もできない）も同じ trigger・event・受信箱で止める。

### D3. site policy の正本は DB。API で再起動なしに反映。grant の credential 設定 API

- **正本**: 新しい表 `browser_site_policies(policy_id TEXT PRIMARY KEY, exact_origin, login_url, password_selector,
  submit_selector NULL, source TEXT CHECK(source IN ('api','config')), created_at, updated_at)`。migration の番号は
  実装時に全ブランチを走査して空きを取る（この ADR では決めない）。`policy_id` は一意（現行の lookup は
  `(policy_id, exact_origin)` だが、1 policy = 1 origin に揃える）。
- **API**（admin token。web は owner session を要求してから呼ぶ。D3 末尾）:
  - `GET /api/v1/browser/site-policies` → `{items: [TrustedSitePolicy + source, updated_at]}`
  - `PUT /api/v1/browser/site-policies/{policy_id}` → 作成または置換。`TrustedSitePolicy::validate()`（ADR-0110 D2）を
    通らなければ 422（固定 code）。
  - `DELETE /api/v1/browser/site-policies/{policy_id}` → いずれかの grant の `credential_policy_ids` が参照している、
    または開いている browser wait が参照しているなら 409 `site_policy_in_use`（参照元の node id を返す）。
  - 変更は `Event::BrowserSitePolicyChanged { policy_id, op: upsert|delete, source }` を同じ transaction で追記
    （selector・URL は event に載せない。値は表にだけある）。
- **再起動なしの反映**: `UnixCredentialBrokerControl` の `site_policies: Vec` を、store を引く口に替え、
  `register`（手動登録）のたびに DB から読む。実行中の wait・承認は ADR 2026-10-05 と同じく policy の binding hash を
  照合するので、途中で変えた policy は次の判定から効き、変更前に承認した注入は通らない。
- **config からの移行**: `[[api.browser_site_policies]]` は「空の DB に入れる種」（org.toml と同じ扱い）に格下げする。
  daemon 起動時、DB に同じ `policy_id` が無いものだけを `source = 'config'` で入れ、event を残す。DB にあれば DB が勝ち、
  内容が違えば D5 の doctor が `WARN` を出す。config の欄は互換のため当面読むが、docs/ops で「取り込み後は消してよい」と書く。
  起動時の `validate()` 失敗は従来どおり起動エラー（壊れた種を黙って捨てない）。
- **grant の credential 設定**: `PATCH /api/v1/org/{id}/browser-settings` に `credential_use: Option<bool>` を足す。
  `true` は grant の `allowed_actions` に `CredentialUse` を加え（`allowed_actions` が無い grant は `PHASE1` を実体化してから
  加える）、`false` は外す。`allowed_actions` の任意編集は開けない（業務 action の拡張は別の判断）。
  `credential_policy_ids` は各 id が `browser_site_policies` に実在しないと 422 `unknown_site_policy`。
  `credential_identity_ids` の鍵も `credential_policy_ids` の部分集合であること。既存の org browser event に
  `credential_use` の変化を載せる。grant に `credential_use` を入れても、run での使用は毎回承認（§2）。
- **web の扱い**: site policy の編集と grant の `credential_use`・`credential_policy_ids` の変更は、web の
  `/browser/settings` で **owner session があるときだけ**行える（run が要求できる範囲を広げる操作なので、承認と同じ本人確認に揃える）。

### D4. 起票・retry 時の最小 policy の自動付与と retry での引き継ぎ

- **対象**: `browser-enabled` の skill を持つ task が作られるすべての経路（`POST /tasks`、CoS の operations、
  delegate.json・followups・計画の子 task）。作成と同じ transaction で、保存された task policy が無ければ付ける。
  実装は task-ops の作成の共通経路 1 か所に置く（経路ごとに書かない）。
- **最小 policy の中身**（決定的）:
  - `policy_id = "auto"`（task ごとの表なので task 間で衝突しない）、`revision = 1`、`domain_mode = common_hosts`。
  - `network_domains = requirements.browser.allowed_domains`。requirements が無い・空なら**付けない**
    （grant 全体に広げない。ADR 2026-10-05）→ D2 の `browser_policy_missing` で止め、task 詳細で origin を入れれば再開。
  - `allowed_actions = BrowserAction::PHASE1`（公開の業務 action）。`approval_actions = []`
    （`click`・`download` は `ALWAYS_APPROVED` で毎回承認のまま）。
  - credential: `browser_site_policies` のうち `exact_origin` が `network_domains` のどれかと origin として一致し、
    かつ browser grant を持つ node のどれかの `credential_policy_ids` に入っている id を `credential_policy_ids` に入れ、
    1 つでもあれば `credential_use` を `allowed_actions` に加える（使用は毎回承認。§2）。
    担当の node は作成時には決まらないので、run 時の `EffectiveBrowserPolicy::derive` が担当の grant と照らす
    （既存の `CredentialPolicyNotGranted`）。
  - `artifact_policy_id` は付けない。
- **retry の引き継ぎ**: `retry_task_with_execution` は元の task の保存 policy を新しい task に写す（同じ transaction。
  `revision` は 1 に戻し、他は同じ）。元に policy が無ければ新しい task に D4 の規則で自動付与する。
  reopen は同じ task なので policy はそのまま。
- **出自の記録**: `Event::BrowserTaskPolicySet { policy_id, revision, source: auto|retry|human }` を新設し、自動付与・
  引き継ぎ・`PUT /tasks/{id}/browser/policy` のすべてで追記する（policy の中身は載せない。hash だけ）。
- **編集**: `PUT /tasks/{id}/browser/policy` が受ける状態に「D2 の `browser_prerequisite` で `blocked`」を加える
  （人待ち〈`waiting_for_auth`/`waiting_for_approval`〉の `blocked` は binding hash があるので変えない）。
  web の task 詳細で確認・編集する。

### D5. 点検コマンド

- 名前: **`celerisctl browser doctor`**（`[--json]`）。daemon の `GET /api/v1/browser/readiness` を呼び、daemon が
  自分の設定・release・PATH で評価した結果を出す（celerisctl が config を読み直して推測しない。本番 daemon の環境で
  見たものが正）。
- 出力は 1 項目 1 行、`<状態> <項目> <説明>`。状態は `OK`・`NG`（browser task が動かない）・`WARN`（動くが直すべき）・
  `SKIP`（この構成では見ない）。例:

  ```
  OK   ledger           /home/u/.local/celeris/releases/1a2b3c4d5e6f/browser/conformance.json release=1a2b3c4d5e6f agent-browser=0.38.1
  NG   ledger           browser の適合台帳が未配置（code=missing）
  OK   ledger-backends  public=claude-code,browser-specialist credential=claude-code,browser-specialist
  OK   agent-browser    /home/u/.local/bin/agent-browser 0.38.1
  OK   runtime          launcher
  NG   launcher         /run/user/1000/celeris-browser-launcher.sock に接続できない
  SKIP bwrap            runtime=launcher
  OK   egress-resolver  192.168.1.1 応答あり
  NG   credentiald      api.browser_credentiald_control_socket が未設定
  OK   attestation-key  ~/.config/celeris/web.attestation.pub
  OK   site-policies    1 件（manaba）
  WARN site-policy      manaba: config.toml の値は DB と異なる（DB が正）
  OK   grant            browser-execution: credential_use=on credential_policy_ids=manaba
  ```

- 項目と判定:
  - `ledger` / `ledger-backends`: D1.4 の `ledger_status`。`credential` が空なら `WARN`（公開 task は動く）。
  - `agent-browser`: daemon の PATH での実行ファイルと版。
  - `runtime`: `[browser] runtime`。
  - `launcher`（runtime=launcher のとき）: socket へ接続し、launcher の固定 IPC で害の無い問い合わせが通るか。
  - `bwrap`・`sandboxd`・`egress`（runtime=daemon のとき）: `isolated_runtime_ready` と同じ file 検査。
  - `egress-resolver`: `[browser.egress] resolver` の設定と、固定名の DNS 問い合わせ 1 回（2 秒）の応答。
  - `credentiald`: control socket の設定・存在・接続、daemon PID が admit されているか。
  - `attestation-key`: `api.browser_attestation_public_key_file` が読めて鍵として解釈できるか。
  - `site-policies` / `site-policy`: DB の件数、各 `validate()`、config との食い違い（D3）。
  - `grant`: browser grant を持つ node ごとの `credential_use` と `credential_policy_ids`、実在しない id は `NG`。
- 終了 code: `NG` が 0 件なら 0、1 件以上なら 1、daemon に届かなければ `NG daemon <url> に接続できない` の 1 行で 2。
  `--json` は `{items: [{status, check, detail}]}`。
- web の `/browser/settings` も同じ readiness を表として出す（preflight 葉ではなく web-site-policy 葉）。
- docs/ops に `docs/ops/browser-prod.md` を置き、doctor の各 `NG` に対する人の手順（どの file・どの設定・
  どの unit。本番操作は人が実行）を 1 項目ずつ書く。

## 4. 後続の葉と試験接頭辞

| 葉 | 決定 | 試験名の接頭辞 | 主な場所 |
|---|---|---|---|
| ledger-gate | D1.4（`ledger_status`・`generated_for` の読み取り・`configure_conformance`）、D2 | `browser_ledger_gate_` | `crates/task-worker/src/browser.rs`、`crates/task-core/src/transition.rs`・Event、`crates/task-dispatch/src/dispatcher/dispatch_run.rs`・`leases.rs`、`crates/task-ops`（inbox・TaskDetail）、`crates/task-api`（readiness） |
| site-policy-api | D3 | `browser_site_policy_db_` | `crates/task-core`（migration・store）、`crates/task-api/src/browser.rs`・`handlers/org.rs`、`crates/celeris/src/daemon/api.rs` |
| ledger-release | D1.1〜D1.3・D1.5（runner の `--celeris-release`、release dir からの path 解決、`sd_browser_ledger`・release.sh・promote.sh・`browser-ledger.sh`、`celerisctl browser ledger check`） | `browser_ledger_release_` | `scripts/browser-conformance.py`、`scripts/selfdeploy/{lib,release,promote,browser-ledger}.sh`・`tests/`、`crates/celerisctl`、`crates/celeris` |
| task-policy-auto | D4 | `browser_policy_autoattach_` | `crates/task-ops`（作成の共通経路・`retry.rs`）、`crates/task-core`（store の retry・Event）、`crates/task-api/src/browser.rs` |
| preflight | D5（`celerisctl browser doctor`・readiness の各項目・`docs/ops/browser-prod.md`） | `browser_doctor_` | `crates/celerisctl/src/commands/browser.rs`、`crates/task-api`、`docs/ops/` |

- 試験は外部ネットワークに出ない（egress resolver は loopback の偽 resolver、launcher・credentiald は一時 socket の偽物）。
  実 agent-browser・userns を要る試験は既存の `#[ignore]`・`CELERIS_USERNS_TESTS=1` の規則に従う。
- web の葉（web-site-policy・web-task-policy）は新しい Event（`browser_prerequisite_*`・`browser_site_policy_changed`・
  `browser_task_policy_set`）を `web/api/realtime/event-kinds.ts`・`invalidation-map.ts` に足す。

## 5. 帰結

- 本番の browser task は、release が台帳を作れた時点から動く。作れなければ、投入直後に「browser の適合台帳が未配置」等の
  理由で止まり、受信箱と task 詳細に出て、infra 再試行で無言のまま回り続けることは無くなる。
- 人の作業は owner session・初回の credential 入力・承認ボタンに減る。site policy と grant の credential 設定は web で、
  再起動なしに変わる。
- 台帳の生成が release の時間を延ばす（上限 1800 秒の段。cargo test のビルドは gate と target を共有する）。
- 代償: release dir に `browser/` が増え、D1.5 の再生成だけは release dir に後から書く。config の
  `[[api.browser_site_policies]]` は種に格下げされ、DB と食い違い得る（doctor が出す）。

## 付記（close-out の実装突き合わせ）

2026-10-08、統合後の HEAD（`a48a440c`）で D1〜D5 を突き合わせた。証拠は
[進捗](../progress/2026-10-08-browser-prod-enablement.md)、本番手順は
[docs/ops/browser-prod-enablement.md](../../docs/ops/browser-prod-enablement.md)。

- **D1（台帳の生成・配置・渡し方）**: runner の `--celeris-release`、`sd_browser_ledger`・release.sh の browser-ledger 段（非 blocking）・promote の記録・
  `scripts/selfdeploy/browser-ledger.sh`、`celerisctl browser ledger check`、daemon の `configure_conformance`（release dir の台帳）。試験 `browser_ledger_release_`（celerisctl 7・celeris 3）と selfdeploy の `browser_ledger_release_stages.sh`。
  実 agent-browser・実 Chromium での本番生成は未実施。
  - 食い違い（D1.3）: `promote.sh` は `promoted.json` に `browser_ledger: {ok, code}` を書き、release.sh も `gate.json`・`manifest.json` に
    同じ欄を書くが、`crates/celeris/src/releases.rs`（`read_release`）と `crates/task-api/src/releases.rs` はこの欄を読まず、
    `GET /releases` の応答にも web の release 一覧にも台帳の状態は出ない。台帳の状態は今は `celerisctl browser doctor`・
    `GET /api/v1/browser/readiness`・web の `/browser/settings`（readiness 表）で見る。
- **D2（台帳不足は理由つきで止める）**: `ledger_status`・`LedgerWatch`・`BrowserPrerequisiteCode` と dispatch での停止。試験 `browser_ledger_gate_`（12）。
  - 食い違い: readiness は実装したが、`TaskDetail.browser.prerequisite` 欄は未追加。web は既存 event から表示する。
  - 訂正（2026-10-08、葉 policy-missing）: close-out の時点で「受信箱の `browser_prerequisite` reason は実装した」と書いたが、
    `crates/task-ops/src/inbox.rs` には無かった（部署 reviewer の差し戻し）。葉 policy-missing で実装した:
    `build_attention` が `blocked` かつ直前の遷移の reason が `browser_prerequisite` の task について
    `AttentionItem::BrowserPrerequisite { task, code, message, at }`（API の `type` は `browser_prerequisite`）を出す。
    `message` は最後の `BrowserPrerequisiteBlocked` の文。`BrowserPrerequisiteResumed` で ready に戻った後は出さない。
    共通形の受信箱（ADR-0133）では新しい種類を足さず、既存の `browser_wait`（browser の人手）に写す。項目の id は
    `browser_prerequisite-<task>-<code>`、選択肢と native 操作は無い（受信箱では答えない。文が次の手を言い、前提が
    揃えば消える）。link は `/tasks/<id>`。試験は `browser_policy_missing_inbox_item_carries_message_and_disappears_after_resume`。
- **D3（site policy の DB 正本・grant の credential 設定）**: migration と store、`crates/task-api/src/browser.rs`、web の `/browser/settings`。試験 `browser_site_policy_db_`（6）。
  - 食い違い: `Event::BrowserSitePolicyChanged` は作らなかった。site policy には task_id が無く events 表は task 単位なので、
    `org_browser_events`（migration 0051）と同じ形の別表 `browser_site_policy_events`（migration 0060。追記専用）に
    `crates/task-core/src/store/site_policies.rs` の `append_event` が `{policy_id, op, source}` を同じ transaction で書く
    （selector・URL は載せない点は ADR どおり）。このため §4 が web の `event-kinds.ts`・`invalidation-map.ts` に足すとした
    `browser_site_policy_changed` は無く、web の site policy 一覧は SSE では更新されず、自分の PUT/DELETE の後の再取得で更新する。
    記録は葉の進捗 `site-policy-api.md`。
- **D4（最小 policy の自動付与と retry の引き継ぎ）**: 作成の共通経路と `retry.rs`。試験 `browser_policy_autoattach_`（6）。
  - 食い違い: `BrowserTaskPolicySet { source }` event と出自欄は未実装。web は `policy_id`（`auto`・`web-human`）で出自を表す（承認・能力の判定には使わない）。
  - 訂正（2026-10-08、葉 policy-missing）: D2 末尾の `browser_policy_missing` での停止は close-out の時点で未実装だった
    （`BrowserPolicyMissing` を出すコードが無く、保存 policy の無い browser task は worker の `browser policy rejected` で
    infra_requeue を上限まで繰り返した。部署 reviewer の差し戻し）。葉 policy-missing で実装した:
    `crates/task-dispatch/src/dispatcher/browser_prereq.rs` の `browser_task_code`（dispatch 前の gate と再開の両方）が、
    台帳より先に、保存 policy が無い・読めない（`browser_task_policy_get` の Err も止める側）・task の
    `requirements.browser.allowed_domains` と交わらない（`task_run_policy` が Err）ときに `BrowserPolicyMissing` を返す。
    `Ready → Blocked`（reason `browser_prerequisite`、attempts 不変、`BrowserPrerequisiteBlocked` 1 回）で worker・LLM を起こさない。
    `PUT /tasks/{id}/browser/policy`（`browser_prerequisite` で止まった task も受ける）の後、task ごとの見直し
    （`RESCAN_EVERY_TICKS` = 30 tick ごと）で `BrowserPrerequisiteResumed` を足して ready に戻る。worker が
    `browser policy rejected`（担当の grant と交わらない等）を返した場合も `worker_error_code` が `BrowserPolicyMissing` に
    分類し、infra_requeue でなく同じ trigger で止める。試験は `browser_policy_missing_`（task-dispatch 4、task-ops 1）。
    - 残る食い違い: 担当の grant と交わらないことは dispatcher の gate では見ない（worker の `admit` だけが見る）。この形で
      止まった task は policy を直さなくても 30 tick ごとの見直しで ready に戻り、worker の `admit` で再び止まる
      （LLM は呼ばず attempts も変えないが、遷移と event が周期で増える）。
- **§4 の web の event 種類**: `browser_prerequisite_blocked`・`browser_prerequisite_resumed` は `web/api/realtime/event-kinds.ts`・
  `invalidation-map.ts` にある。`browser_site_policy_changed`（D3 の食い違い）と `browser_task_policy_set`（D4 の食い違い）は
  Event が無いので足していない。
- **D5（点検コマンド）**: `celerisctl browser doctor` と `GET /api/v1/browser/readiness`（bearer 必須）、credentiald の Ping、`docs/ops/browser-prod-enablement.md`・`browser-prod.md`。試験 `browser_doctor_`（13）。
