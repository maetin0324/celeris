# 測定環境の launcher: 単一の設定源・接続先 daemon の同一性照合・固定記録 env-lock

- 日付: 2026-10-18
- 状態: 決定（実装は後続 WU: daemon-identity-api / env-lock / env-check / launcher / runbook）
- tasks: [01M56HCNS9SYAS7TAEA7Z3E4CZ]
- 関連: ADR 2026-10-17-mlsys-pilot2-fixes、ADR-0040（release・instance・昇格の認可）、ADR-0139（SO_REUSEPORT 相乗り）、
  `crates/campaign/src/{measure_env,preflight,main,lock,lock_exec,curl_transport}.rs`

## 背景（cae27fc073a5 時点の実装を読んだ結果）
- `celeris-campaign preflight/run/watch` は `--api-url`・`--token-env` と `--measure-config`（測定 daemon の
  config.toml）を**別々に**受け取る。`measure_env::check_config` は config ファイルの中身だけを検査し、
  `--api-url` の先で答えている daemon がその config で起動したものかは誰も見ていない。別 daemon（本番を含む）
  の URL を渡しても、config さえ正しければ preflight は通る。
- task-api の既存の読取:
  - `GET /api/v1/health`（無認証）: `instance_id`・`release`（`--release` の自己申告）・`mode`・`role`・
    `celeris_version`・`schema_version`。path は出さない。
  - `GET /api/v1/config`（認証）: `ConfigView` = `config_path`・`db`・`workspace_root`・`api.bind`・clusters 等。
    **起動時に 1 回作る静的な値**（`crates/celeris/src/daemon/api.rs` `config_view`、reload で更新されない）。
    knowledge root・default_mounts・memory dir・langmem/gc・cron seed・selfdeploy・llm-proxy listen・
    CLI 上書きの有無・config ファイルの中身の hash・pid は無い。
  - `GET /api/v1/releases`: `running`（release・role・instance_id）と `daemon_instances`（pid を含む）。
  - `GET /api/v1/llm/models`（tier 付きカタログ）、`GET /api/v1/routing/evidence`（根拠表）、`GET /api/v1/llm/accounts`。
- `celeris` は `--config` の他に `--db/--listen/--workspace-root/--token-file` の CLI 上書きを持つ（`main.rs`）。
  上書きは config ファイルを読んでも見えない。
- API の listen は `bind_reuseport`（SO_REUSEPORT）。同じ port に 2 つ目の daemon が bind でき、要求が
  振り分けられる（ADR-0139 の事故と同じ型）。「URL が正しい」は「答えた daemon が正しい」を意味しない。
- 昇格の認可（ADR-0040 付記）: `--release <sha12>` が `[selfdeploy] releases_dir` の下にあり `current` で
  なければ exit 4。releases_dir に無い名前は「unmanaged」で判定外。`[selfdeploy]` の既定は本番の
  `~/.local/celeris/releases`・本番の作業 checkout・`push = true`。測定 daemon がこの既定のままだと
  `POST /releases/{sha12}/promote` が本番の `promote.sh` を起こせる。
- A: preflight は `ResolvedProgram {path, version}` を返し run はその path を起動するが、**実体の sha256 は
  記録していない**。同じ path の binary 差し替えは版文字列が同じなら検出できない。
- B/C: `lock.json`（`CampaignLock.execution` = manifest の configs・routing・budget）は「宣言」の固定で、
  daemon の release・実効 config・worker harness の実体・カタログの tier 対応は固定していない。

## 決定

### D1. 設定源は測定 env ファイル 1 つ（`measure-env.toml`）。接続先・token・state・sandbox 資産はそこから導出する
- launcher と `celeris-campaign` の API を使う全 subcommand（preflight・run・watch・usage・final-accounting、
  および account を引く経路）は `--env <measure-env.toml>` だけを受け取る。形:
  ```toml
  schema = "celeris-measure-env/1"
  measurement_id = "<id>"                  # lock.json の measurement_id と同じ
  manifest = "<campaign manifest path>"
  daemon_config = "<測定 daemon の celeris config.toml>"   # 下の導出の唯一の元
  normal_config = "~/.config/celeris/config.toml"          # 突き合わせ専用（読むだけ）
  release = "<sha12>"                      # B/C の Celeris release（manifest の B/C harness_version と一致必須）
  releases_root = "~/.local/celeris/releases"   # binary の取り出し元（読むだけ）
  state_dir = "<launcher 所有の dir>"      # launcher.json・daemon.log・release の写し
  out = "<campaign out dir>"               # lock.json・env-lock.json・台帳
  path = "<daemon と A に渡す PATH>"       # worker harness と A の解決に同じ値を使う
  worker_programs = ["claude", "codex"]    # B/C の worker が起こす CLI（解決して固定記録へ）
  task_template = "…"; canaries = "…"
  a_program = "claude"; a_account = "<claude_oauth account_id>"   # 作業 1 の形に合わせる（D4-3）
  [assets]  # 測定用 sandbox 資産
  materials = "…"; kb = "…"; repo = "…"; inputs = "…"
  cases_root = "…"; grader_root = "…"; grade_cases_root = "…"
  ```
  相対 path は env ファイルの dir 基準、`~` は `$HOME`。
- 導出規則（`daemon_config` を `measure_env` と同じ解決関数で読む）:
  - api_url = `http://<[api] listen>`。listen が `0.0.0.0`/`::` なら `127.0.0.1`/`[::1]`。
  - token = `[api] token_file` の中身を**要求の直前に file から読む**（`CurlTransport` の token 源を
    「環境変数名」から「file path」に置き換える。値は curl の stdin config にだけ載り、argv・ログ・出力に出ない）。
  - state = `db`・`workspace_root`・`[knowledge] root`・`[memory] dir`（無ければ「記憶なし」）。
  - sandbox 資産 = `[assets]`。A の実行ファイルは `path` で解決する。
- 削除する CLI 欄（破壊的変更の方針。互換分岐を残さない）: `--api-url`・`--token-env`・`--measure-config`・
  `--normal-config`・`--canaries`・`--task-template`・`--a-program`・`--a-account`・`--materials-src`・
  `--kb-src`・`--repo-src`・`--inputs-src`・`--cases-root`・`--grader-root`・`--grade-case-root`・
  `--measurement-id`（API を使う subcommand から）。渡されたら「superseded by --env」で exit 非 0
  （無視しない）。mock・dry-run・validate など API を使わない経路は従来どおり。
- 測定 daemon は launcher が `--config <daemon_config>` **だけ**で起動する（`--db/--listen/--workspace-root/
  --token-file` の上書きを使わない。D2 の `identity.overrides` が空であることを照合する）。
- `measure_env::check_config` に足す検査（本番への副作用を断つ。どれも config ファイルを読むだけ）:
  `[api] listen` の port が通常運用と違う、`[llm_proxy]` は `enabled = false` か `listen` を明示して通常運用と
  違う port（`listen` には既定値があり、省略すると通常運用と同じ port に SO_REUSEPORT で相乗りする。ADR-0139）、
  `[selfdeploy] releases_dir` を明示し通常運用の releases_dir と重ならない、`[selfdeploy] repo` が通常運用の
  作業 checkout でない、`push = false`、`[notify] discord_webhook_secret` が空（Discord へ送らない）。
  `[accounts]` の dir は A の account（作業 1）と共有し得るので重なりを拒否しない（D4-3）。
- 却下:
  - `--api-url` 等を残し、`--measure-config` との一致を検査するだけ: 同じ値を 2 か所に書かせる構造が残り、
    手順書と launcher の両方で食い違いの余地がある。導出すれば食い違いがそもそも起きない。
  - 測定 daemon の celeris config.toml に `[campaign]` 節を足して設定源にする: celeris の config は
    `deny_unknown_fields` で、daemon が使わない欄を本体の config に持ち込むことになる。
  - token を従来どおり環境変数名で渡す: launcher が token_file を読んで env に移す手間が増えるだけで、
    「接続先の daemon の token_file と同じ token」であることを保証しない。

### D2. 同一性照合: 既存 API ＋ プロセス／ポート所有確認 ＋ `GET /api/v1/config` に `identity` を 1 つ追加
- **task-api への追加は要る**（最小 1 項目）。既存の `/health`・`/config`・`/releases` だけでは
  (a) 起動後に config ファイルが書き換わった、(b) CLI 上書きで別 DB/listen を使っている、
  (c) knowledge/memory/selfdeploy/proxy の実効値、を daemon 自身の答えとして得られない。
- 追加: `GET /api/v1/config`（認証済み・既に秘密を除いた要約）の応答に `identity` を足す。値は起動時に
  `config_view` と同じ所で 1 回作る（静的。reload は file を読み直すだけで file の hash は変わらないので、
  file を書き換えて reload した場合は D2 の照合で `config_sha256` が食い違って拒否される）。
  ```jsonc
  "identity": {
    "instance_id": "…", "release": "<sha12|dev>", "pid": 12345, "started_at": "RFC3339",
    "config_sha256": "<起動時に読んだ config ファイルの bytes の sha256>",
    "overrides": [],                       // 適用した CLI 上書きの名前（db/listen/workspace_root/token_file）
    "effective": {                         // 秘密を含まない実効値（path は絶対・展開済み）
      "db": "…", "workspace_root": "…",
      "knowledge_root": "…", "knowledge_default_mounts": ["kb:…"],
      "memory_dir": "…|null", "langmem_enabled": false, "knowledge_gc_enabled": false, "cron_seed_count": 0,
      "selfdeploy_releases_dir": "…", "selfdeploy_repo": "…", "selfdeploy_push": false,
      "api_listen": "127.0.0.1:7811", "api_token_file": "<path のみ。中身は出さない>",
      "llm_proxy_listen": "…|null"
    }
  }
  ```
  hash（effective_sha256・state_path_sha256）は API に出さず、campaign 側が `effective` の正規化 JSON
  （key 昇順）から計算する（API と campaign で同じ hash 関数を二重に持たない）。
  `/health` には足さない（無認証なので path を出さない、ADR-0064 D1 の方針どおり）。
- campaign の照合（`env-check`。preflight の 1 項目として毎回走り、run/watch の開始・再開でも走る）:
  1. `GET /api/v1/config` を env ファイル由来の url・token で 1 回だけ読む（instance/release/pid/effective を
     同じ応答から取り、2 回の GET の間の引き継ぎ（handoff）で値が混ざらないようにする）。
  2. `identity.config_sha256` == env-check 時に読んだ `daemon_config` の sha256。
  3. `identity.overrides` が空。`identity.effective` の各値 == `daemon_config` から D1 の規則で導出した値
     （項目ごとに比べ、違う項目名を `missing` に出す）。`config_path` == `daemon_config` の絶対 path。
  4. `identity.release` == env の `release` == manifest の B/C `harness_version`。`identity.release != "dev"`。
  5. プロセス: `identity.pid` が launcher の `launcher.json` の pid と一致し、`/proc/<pid>/stat` の starttime が
     記録値と一致、`/proc/<pid>/exe` の sha256 == 起動した release 写しの `bin/celeris` の sha256、
     `/proc/<pid>/cmdline` が `--config <daemon_config 絶対 path>` を含む。
  6. ポート所有: `/proc/net/tcp`・`tcp6` で `api_listen`（と `llm_proxy_listen`）の port の LISTEN 行を**全部**
     集め、各 socket inode を持つ fd が `identity.pid` の中にしか無いこと。所有者の見えない inode・別 pid の
     inode が 1 つでもあれば拒否（SO_REUSEPORT の相乗りと、他 uid の daemon を拒否する）。
  - どれか 1 つでも不一致なら A を含め外部実行・task 作成・台帳作成 0 件で非 0 終了（preflight の既存の型）。
- 却下:
  - 新 endpoint `GET /api/v1/daemon/identity`: `/config` が既に「秘密を除いた起動時の実効構成」の置き場で、
    同じ内容の 2 つ目の窓を作るだけ。
  - `/health` に足す: 無認証で path が出る。
  - API を足さずに `/config` の既存欄 ＋ `/proc/<pid>/cmdline` だけで照合: config ファイルの起動後の書き換えと
    knowledge/memory/selfdeploy/proxy の実効値が取れない。cmdline の解析で daemon の config 解釈を二重実装する。
  - `/health.release` だけで版を判断: `--release` は自己申告。exe の sha256 と組で見る（5）。
  - URL の疎通だけで接続先を判断: SO_REUSEPORT で別 daemon が同じ port に居られる（6）。

### D3. 固定記録 `env-lock.json`（`<out>/env-lock.json`。`lock.json` の隣）と再開照合
- 作る時: run/watch の初回、preflight（env-check を含む）合格の直後・最初の試行の投入より前。原子的に書く
  （一時 file → rename）。既にあれば作らず照合する（再開）。`lock.json` と同じく「保存値と作り直した期待値の
  突き合わせ」で、保存 file の丸ごと差し替えも検出する。
- 中身（秘密値なし。path・版・sha256・id だけ）:
  ```jsonc
  {
    "schema": "celeris-env-lock/1", "measurement_id": "…",
    "campaign_sha256": "…", "lock_sha256": "<lock.json の sha256 欄>",
    "env_file_sha256": "…",
    "a": { "program": {"path": "/abs/claude", "sha256": "…", "version": "…"},
           "account": {"account_id": "…", "dir": "/abs", "residue_ok": true} },
    "bc": {
      "release": "<sha12>", "release_bin_sha256": "<写した bin/ の hash_tree>",
      "daemon": { "exe_sha256": "…", "config_path": "…", "config_sha256": "…",
                  "effective": {…}, "effective_sha256": "…",
                  "state_path_sha256": "<db・workspace_root・knowledge_root・memory_dir の正規化 JSON の sha256>" },
      "worker_programs": [{"name": "claude", "path": "…", "sha256": "…", "version": "…"}],
      "arms": [{ "id": "B", "harness_version": "<sha12>", "lower_policy": "…", "evidence_table_id": null,
                 "online_update": false, "models": ["…"],
                 "catalog": [{"model_id": "…", "tier": "…", "source_id": "…"}],   // GET /llm/models の該当行
                 "evidence_sha256": null }]                                   // GET /routing/evidence の該当版
    },
    "instances_seen": [{"instance_id": "…", "pid": 1, "started_at": "…", "first_seen": "…"}]
  }
  ```
- 再開時の照合キー（1 つでも違えば外部実行 0 件で拒否。違う key 名を出す）: `campaign_sha256`・
  `lock_sha256`・`env_file_sha256`・`a.program.{path,sha256,version}`・`a.account.account_id`・
  `bc.release`・`bc.release_bin_sha256`・`bc.daemon.{exe_sha256,config_sha256,effective_sha256,
  state_path_sha256}`・`bc.worker_programs`・`bc.arms`（catalog・evidence を含む）。
  `instance_id`・`pid` は照合キーにしない（daemon を止めて同じ state で起こし直すのは正当な再開。
  新しい instance は `instances_seen` に追記し、D2 の 1〜6 は毎回その時の daemon に対して行う）。
- A の実体照合は試行ごと: harness_exec が A を起動する直前に `a.program.path` の sha256 を取り直し、
  env-lock と違えばその試行を投入せず run を止める（同じ path の差し替え。版文字列が同じでも検出）。
- worker harness の版: daemon には `path` を PATH として渡し、launcher は同じ PATH で `worker_programs` を
  解決・hash する。env-check は `/proc/<pid>/environ` から **PATH の 1 項目だけ**を取り出して env の `path`
  と比べる（他の変数は読まず、ログにも出さない）。
- 却下: `lock.json` に混ぜる（lock.json は case・grader・manifest の宣言の固定で `measurement_id` 単位で
  不変。daemon の実体は再起動で変わり得る事実で、更新規則（instances_seen の追記）が違う）。
  credential file の hash を記録する（秘密由来の値を成果物に出さない）。

### D4. launcher の入口・停止範囲・作業 1 との整合
- 入口は 1 本: `scripts/campaign/measure.sh <subcommand> --env <measure-env.toml>`（薄い shell。判定は
  全部 `celeris-campaign` の Rust 側で行い、試験はそちらに置く）。各 subcommand は再実行できる（冪等）:
  - `prepare`: state_dir・測定 KB root（空か `[assets] kb` と同一）・空の memory dir を作る。`[api] token_file`
    が無ければ 0600 で乱数 token を作る（測定専用。本番の token に触れない）。`<releases_root>/<release>/bin/`
    を `<state_dir>/release/<release>/bin/` へ写し、sha256 が元と一致することを確かめる（本番 release の
    prune で測定中に消えないようにするため。写しは測定 releases_dir の外なので昇格の認可は「unmanaged」で判定外）。
  - `start`: `launcher.json` があり D2-5 が成り立てばその daemon に接続（再起動しない）。無い・死んでいれば
    `setsid <写し>/bin/celeris --config <daemon_config> --release <release> --mode normal` を
    `PATH=<path>` で起動し、`launcher.json`（pid・starttime・exe_sha256・daemon_config・started_at）を書き、
    `GET /api/v1/health` を出来事待ち（上限あり）で待つ。`systemctl`・`systemd-run` は使わない。
  - `preflight`: `celeris-campaign preflight --env`（env-check を含む全件。有料実行 0）。
  - `run`: `celeris-campaign watch --executor celeris --env`（env-lock の作成または照合 → 台帳から再開）。
  - `grade` / `finalize`: 独立採点（check・grade-out）と final-accounting・export。
  - `status`: env-check の結果と台帳の要約（読むだけ）。
  - `stop`: 下記。
- 停止範囲: `launcher.json` の pid **1 つだけ**。止める前に D2-5（starttime・exe sha256・cmdline の
  `--config`）を照合し、合わなければ何もせず非 0（pid 再利用・別 process を殺さない）。SIGTERM → daemon の
  drain（worker の後始末は daemon の既存の kill_grace に任せる）→ 上限経過でその pid にだけ SIGKILL。
  `pkill`/名前一致・process group 全体への signal・`systemctl --user` は使わない。本番 daemon・本番 service・
  本番 DB・本番 releases に触れる経路は無い（D1 の検査で測定 config 自体が本番の port・releases・checkout を
  指せない）。
- 作業 1（試行 identity 別 account 状態）との整合: launcher は account を作らない・消さない・書き換えない。
  account の指定は env ファイルだけ（今は `a_account`。作業 1 が試行 identity ごとの対応を導入したら、その
  対応表を env ファイルの同じ節に置き、env-lock の `a.account` を配列にする）。env-lock は account_id・dir・
  残留検査（`account_dir_residue`）の結果だけを持ち、credential の中身・hash を持たない。`stop` は
  account に触れない。再開時は account_id の一致と、dir の存在・ログイン済み（`GET /llm/accounts`）を照合する。
- 却下: launcher を Rust の新 daemon・汎用管理基盤にする（範囲外）。本番 daemon に接続して測定する
  （汚染検査の前提が崩れる。pilot までの運用で、本測定では廃止）。launcher が systemd unit を作る（本番 host の
  変更になる）。

## 各 WU への入力
- daemon-identity-api: D2 の `identity` を `ConfigView` に追加（`crates/task-api/src/types.rs`、
  `crates/celeris/src/daemon/api.rs` `config_view` に config bytes の sha256・適用した overrides・pid・
  instance_id・release・started_at を渡す）。秘密が出ない試験（token_file の中身が応答に無い）。
  schema 再生成（`UPDATE_SCHEMA=1`、gui `pnpm gen:types`、web へ写し）。`/health` は変えない。
- env-lock: D3 の型・作成・照合・`instances_seen` の追記・試行ごとの A 実体照合（`ResolvedProgram` に
  `sha256` を足し、harness_exec の起動直前に取り直す）。
- env-check: D1 の `--env` と導出、旧 CLI 欄の削除、`CurlTransport` の token 源を file に、D1 末尾の
  `check_config` 追加検査、D2 の照合 1〜6 を preflight に結合（`Host` trait に `/proc` 読みを足し、試験は差し替え）。
- launcher: D4 の `scripts/campaign/measure.sh` と、実 `celeris` 測定 daemon（一時 dir・一時 port・stub
  harness）を使う無 LLM e2e: 別 daemon の URL（同 port 相乗りを含む）・実効 config 相違・同一 path の
  binary 差し替え・版不一致・必須入力欠落がいずれも外部実行 0 件で拒否、一致時は API → watch → 採点 → export、
  再開で env-lock 一致。daemon 試験は userns 不要（ADR-0126）だが、試験は worker sandbox の外で走る前提を確認する。
- runbook: `run-final.md` を `measure.sh` の実在 subcommand と `measure-env.toml` に書き換え、旧
  `--api-url/--token-env` の手順を消す。

## 付記（daemon-identity-api の実装突き合わせ）
- D2 の `identity.effective` から `api_token_file` を**除いた**。既存の不変条件（`tests/e2e/tests/api_scenarios.rs`
  `api_enforces_token_host_and_workspace_boundaries_without_leaking_env_values`: 「token_file の場所は応答に出ない」、
  `docs/api/v1/api.md`）を保つため。token_file の同一性は `config_path` と `config_sha256`（config ファイルの bytes）
  で縛られるので、env-check は token_file の path を比べなくてよい（`daemon_config` の `[api] token_file` を campaign が
  読んで token を得る。D1）。`overrides` に `token_file` が出るので CLI で差し替えた daemon は拒否される。
- `identity` は `ConfigView.identity`（`DaemonIdentityView`/`EffectiveConfigView`、`crates/task-api/src/types.rs`）。
  `llm_proxy_listen` は `effective_enabled()` のときだけ値、memory は `[memory]` が無ければ `null`。
