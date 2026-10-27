# 自己改善のデプロイ — 運用手順（ADR-0040）

`agent-platform` 自身を celeris の上で改善し、**動いている本番を壊さずに**新しい版へ移るための道具。
設計は [ADR-0040](../../agent-docs/adr/0040-self-improvement-deploy.md)（D1〜D5）と
[ADR-0041](../../agent-docs/adr/0041-self-improvement-loop-hardening.md)（D2〜D4）。ここはその**使い方**だけを書く。

外部 operator と CoS の役割・実行担当・重複防止は [外部 operator 手順書](external-operator.md) に従う。

## 0. 全体像

```
release.sh <ref>  →  verify.sh <sha12>  →  promote.sh <sha12>        （戻すとき: rollback.sh）
  本番に触れない      本番に触れない        人が決定する（§4c）       人だけが実行する
  誰が実行してもよい   誰が実行してもよい
```

置き場は**設定**と**状態**に分かれている（ADR-0045 D2。XDG 流）。どちらも環境変数で動かせる:
`CELERIS_CONFIG_DIR`（既定 `~/.config/celeris`）と `CELERIS_STATE_DIR`（既定 `~/.local/celeris`）。

```
~/.config/celeris/       （0700。設定と秘密）
  config.toml            本番の設定（**誰も書き換えない**。例外は人が実行する `relocate-db.sh` の `db =` の書き換えだけ。§5b）
  org.toml               組織の種
  api.token              本番 API のトークン
  web.password  web.session-secret  web の認証
  secrets/               ADR-0030 の秘密（1 秘密 = 1 ファイル）

~/.local/celeris/        （状態）
  celeris.sqlite3        既定の DB の場所。本番は `config.toml` の `[db].path` でローカルディスクの
                          `/var/lib/celeris/celeris.sqlite3` を指す（ADR-0064。§5b）。どちらも
                          `sqlite3 .backup` と `mode=ro` で読むだけ。状態ディレクトリ本体はここのまま。
  current -> releases/<sha12>     いま動いている版
  previous -> releases/<sha12>    直前の版（rollback 先）
  releases/<sha12>/     bin/{celeris,celerisctl}  web/app/  manifest.json  gate.json  verify.json
                        changes.json      この版で何が変わるか（ADR-0041 D4。§2）
                        notes.json / notes.md  この版に何が入ったか（task 単位。ADR 2026-10-04-release-notes。§4d）
                        promoted.json     昇格の記録 {promoted_at, mode, from}（ADR-0041 D3。§4。成功したときだけ）
                        promote_failed.json  直近の昇格が失敗した記録 {failed_at, error}（§4。失敗したときだけ。
                                          次の昇格の試みが始まると消える）
                        scripts/          selfdeploy 一式の写し（ADR-0040 D6。昇格に作業チェックアウトが要らない）
                        promote.log       この API 経由の昇格の出力（§4c）
                        promote.lock      昇格中の pid
  releases/.build/tree/  release.sh の detached の作業ツリー（Phase SD-1: 場所を固定して使い回す。毎回
                        `git checkout --force <sha>` + `git clean -ffdx` でその sha のきれいな checkout にする）
  releases/.build/<sha12>/  その sha の gate が落ちたときの gate.json と .gate-<step>.log（次に同じ sha を回すと消える）
  releases/.build/.lock-<sha12>  同じ sha の二重ビルドを止める flock（ADR-0041 D2）
  releases/.lock-release  異なる sha も含め、検査から成果物のコピーまでを直列化する flock（共有 target と
                        作業ツリーを 2 本が同時に触らないのもこれ）
  releases/.cargo-target/  CARGO_TARGET_DIR の退路（scratch が無効・celerisctl が無いとき。普段は scratch の
                        `release-build` の target。ADR-0075 追記「Phase SD-1 実装時の逸脱」）
  staging/              verify.sh の作業場所（毎回作り直す。`.lock` だけは残る）
  staging/.lock         verify.sh を 1 本に直列化する flock（ADR-0041 D2）
  backups/              昇格前の DB のコピーと promote-<ts>.log
  backups/pre-celeris/  改名の移行（ADR-0045。済み）で残した旧い設定と unit
  workspaces/  memory/  claude-accounts/  codex-accounts/  logs/
  tools/{ldr,paperqa,opencode}/   アダプタが使う venv と設定
```

`SD_DB`（selfdeploy が読む DB）は **`config.toml` の `db =`（または `[db].path`）から決まる**。
`CELERIS_DB` で DB を、`CELERIS_CONFIG` で設定ファイルそのものを上書きできる（試験・別の置き場での検証用）。

**同時に走らせない**（ADR-0041 D2）。`verify.sh` は staging のディレクトリとポートを固定で使うので、
2 本目は `staging/.lock` で待つ（上限 `SD_VERIFY_LOCK_WAIT`、既定 1800 秒。超えたら **exit 75**
= EX_TEMPFAIL「いまは無理。あとでもう一度」）。`release.sh` は同じ sha の 2 本目が**待たずに** exit 75
（同じ sha を 2 度ビルドしても結果は同じなので、待つ意味が無い）。**75 は「壊れた」ではなく「混んでいる」**。

ポート:

| | 本番 | staging（verify.sh） |
|---|---|---|
| celeris API | `127.0.0.1:7710` | `127.0.0.1:7711`（N-1 は `:7712`） |
| web | `127.0.0.1:7720` | probe `127.0.0.1:7728`（一時起動） |

## 1. systemd の unit を入れる・更新する

テンプレート unit はリポジトリの `deploy/systemd/` にある。**人が初回および雛形変更時に**入れる:

```bash
bash ~/workspace/agent-platform/scripts/selfdeploy/install-units.sh
# = deploy/systemd/ の celeris@ / celeris-web@ / celeris-credentiald@ / celeris-web-lan の
#   .service を ~/.config/systemd/user/ に置いて daemon-reload（置くだけ。enable しない。
#   既存との差分があれば、何も書き換えず diff を表示して停止する）
# 差分を確認し、運用に必要な設定が config または drop-in にあることを確かめてから:
# bash ~/workspace/agent-platform/scripts/selfdeploy/install-units.sh --replace-existing
# 置換時は <unit>.bak-<ts> に旧ファイルを残す。
systemctl --user cat celeris@.service        # 入ったことの確認
loginctl show-user "$USER" | grep Linger   # Linger=yes であること（ログアウトしても動き続ける）
```

`%i` はリリースの `sha12`。`celeris@<sha12>` は `~/.local/celeris/releases/<sha12>/bin/celeris --config ~/.config/celeris/config.toml
--release <sha12>`、`celeris-web@<sha12>` は release の `web/app/` で `node server/index.js` を動かす。
web の環境変数・認証・LAN 中継は [web 運用手順](web-operation.md) を参照する。

`celeris-credentiald@<sha12>` も home/hot root のどちらでも設置される。daemon と同じ
`~/.config/celeris/config.toml` を `serve --config` で読み、`[browser].launcher_uid` を使う。
UID を unit に直接書かない。config の path を drop-in で変更する場合は daemon と broker の
両方を同じファイルへ向ける。設定変更後は両方の再起動が必要。config を読めない・TOML/UID が不正なら
broker は起動失敗し、UID 未設定なら credential 注入は拒否される。

`--replace-existing` は表示済み差分を運用者が確認した後に使う。削除される設定を見落とさず、
新しい config または必要な drop-in に反映してから実行する。mask や symlink は
`--replace-existing` でも拒否するので、運用者が目的を確認して個別に整理する。drop-in 自体は置換対象外なので
`systemctl --user cat` で最終的な ExecStart も確認する。UID 引数付き旧 unit からの初回移行は
[本番への移行](../../agent-docs/progress/2026-10-11-credentiald-launcher-config.md#本番への移行)の順序に従う。
旧 release は `serve --config` に対応しないので、新 unit の設置直後に旧 broker を再起動しない。


## 2. リリースを作る（`release.sh`）

```bash
scripts/selfdeploy/release.sh HEAD          # または ブランチ名 / タグ / sha
scripts/selfdeploy/release.sh celeris/01M2XXX # 自己改善の案件の実装ブランチ（celeris が切る。ADR-0041 D1）
```

- `~/.local/celeris/releases/.build/tree` の **detached worktree** でだけビルドする（Phase SD-1: 場所は固定で、毎回
  `git checkout --detach --force <sha>` + `git clean -ffdx` してから HEAD と「追跡外・変更が 0」を確かめる。無い・壊れていれば
  `git worktree add` で作り直す）。作業チェックアウト（`~/workspace/agent-platform`）が汚れていても、その中身は使われない。
- `CARGO_TARGET_DIR` は scratch pool の lease（**全リリースで 1 つの owner `release-build`**、TTL `SD_RELEASE_TARGET_TTL` 既定 48h）。
  作業ツリーの場所と target が変わらないので、cargo は変わった crate とその下流だけを作り直す（テストは毎回全部走る。gate.json の
  `cargo_test` にバイナリ数・合格数）。終了時は lease を release せず touch する（release すると GC が即回収するため）。
  `CARGO_INCREMENTAL=0` 固定。詳細は ADR-0075 追記「Phase SD-1 実装時の逸脱」。
- gate（この順。1 つでも非 0 ならリリースを作らない。Rust と web の必須段）:
  `cargo-workspace-clean`（自前の workspace パッケージの `cargo clean -p …`。外部依存は保持。**共有 target がこの作業ツリーから
  作られていれば飛ばす**）→ `cargo fmt --check` → `cargo-test`※※（`scripts/dev/test-parallel.sh`）→ `cargo clippy --workspace -- -D warnings`
  → `cargo build --release -p celeris -p celerisctl`
  → web `install --frozen-lockfile` → `typecheck` → `test` → `release`（build を含む）。
  pnpm は web/package.json の packageManager に固定する。web の段を飛ばす切替は無い。
- **※※ `cargo-test` はテストバイナリを並列に回す（Phase SD-2、ADR-0041 §8、人の判断 2026-09-28）**: `scripts/dev/test-parallel.sh`
  = `cargo nextest run --workspace --no-fail-fast --test-threads $CELERIS_TEST_JOBS`（既定 `min(8, max(2, nproc/3))`）の後に
  `cargo test --doc --workspace`（nextest は doc-test を回さない）。範囲は `cargo test --workspace` と同じ。直列が要るテストは
  **ビルドする sha の** `.config/nextest.toml` の test-group で縛る。gate.json の `cargo_test` は
  `{runner: "nextest", nextest_version, jobs, binaries（nextest のバイナリ数 + doc-test の crate 数）, nextest_binaries, doc_binaries,
  passed, failed, ignored, nextest_exit, doctest_exit, nextest_secs, doctest_secs, summary_parsed, userns}`。どちらかが落ちる・nextest の
  `Starting … across N binaries` / `Summary …` の行が読めない（全部走った証拠が無い）ときは段が落ちる。
  - `cargo-nextest` が要る（版は `tools/nextest/VERSION` で固定。入れ方は `docs/ops/nextest.md`）。無ければ作業ツリーを作る前に
    入れ方を示して落ちる。非常用に `SD_GATE_TEST_RUNNER=cargo-test` で従来の直列の `cargo test --workspace`（`runner: "cargo-test"`）。
  - 開発・WU・phase 完了では全体の試験を回さない（`scripts/dev/test-affected.sh`。ADR 2026-10-20-full-tests-at-release-only）。並列で絞って回すなら
    `scripts/dev/test-parallel.sh [-p <crate> …]`。
- 成功時は `bin/` と `web/app/`、web tarball、manifest.json、gate.json を梱包する。
  `web/app` では `pnpm install --prod --offline --frozen-lockfile` を実行し、node_modules の実在と
  server/app.js の import 解決を確認する。梱包が失敗しても release は公開せず、失敗の gate とログを残す。
- `scripts/` は **その sha の `scripts/selfdeploy/*.sh` をそのまま写したもの**（実行ビットごと。ADR-0040 D6、
  Phase 48）。`POST /releases/{sha12}/promote` はこの `<release>/scripts/promote.sh` を起こすので、
  **作業チェックアウトが別のブランチにいても、無くても昇格できる**。`lib.sh` は
  `dirname "${BASH_SOURCE[0]}"` で自分の隣を読むだけで、場所はすべて `CELERIS_STATE_DIR` 基準なので
  リリースの中から source しても動く（`SD_REPO` を要るのは `release.sh` の worktree 操作だけ）。
- 失敗したら**リリースディレクトリは作らず**、`~/.local/celeris/releases/.build/<sha12>/gate.json` と
  `.gate-<step>.log` を残す（作業ツリーから写したもの。次に同じ sha で `release.sh` を回すと消える）。
- 掃除: `current` / `previous` / いま作った版 / 検証済み（`verify.json.ok`）の新しい 3 件だけを残す。
- gate.json の追加の欄（Phase SD-1。読む側は無視してよい）: `build`（`tree` / `tree_reused` / `cargo_target` / `scratch_owner` /
  `workspace_clean`）、`cargo_test`（`binaries` / `passed` / `failed` / `ignored`）、`bundle`（`secs` /
  `web_bundle_secs` / `release_total_secs`）、`steps[].skipped` / `steps[].reason`。

`manifest.json`:

```json
{"sha": "...", "sha12": "...", "ref": "HEAD", "built_at": "...", "profile": "release",
 "schema_version": 10, "celeris_version": "0.1.0", "gate_ok": true}
```

`schema_version` は その sha の `crates/task-core/src/store.rs` の `pub const SCHEMA_VERSION` を読んだもの。

### 昇格したら何が変わるか（`changes.json`。ADR-0041 D4）

`release.sh` は**ビルド時の `current`** を起点に、そこからこのリリースまでの差分を書き留める:

```json
{"base": "70e3175eeb20",
 "at": "2026-09-19T13:00:00Z",
 "commits": [{"sha": "…40 桁…", "subject": "phase 50 / G15: …"}],
 "files": ["scripts/selfdeploy/verify.sh", "agent-docs/PROGRESS.md"],
 "sensitive": ["scripts/selfdeploy/verify.sh"]}
```

- `base` は `~/.local/celeris/current/manifest.json` の sha12。`current` が無ければ `null` で、
  `commits` / `files` は空（比べる相手が無いので「何が変わるか」を言えない）。
- `commits` は `base..<sha>` を**新しい順に最大 50 件**。`files` は `git diff --name-only base <sha>`。
- **`sensitive`** は `files` のうち、`scripts/selfdeploy/lib.sh` の **`SD_SENSITIVE_PATTERNS`** に
  **前方一致**したもの。一覧はそこ 1 か所にしかない（API も GUI もこの結果を運ぶだけ）:

  `scripts/selfdeploy/` / `deploy/` / `crates/celeris/src/instance.rs` / `crates/celeris/src/releases.rs` /
  `crates/task-api/src/releases.rs` / `crates/task-core/migrations/` / `CLAUDE.md` /
  `.claude/` / `config/` / `agent-docs/adr/0040-` / `agent-docs/adr/0041-`

  これらを変えたリリースは「昇格の仕組みそのもの」「本番の設定」「エージェントへの指示文」を変える。
  GUI は赤いバッジ「安全に関わる変更 N 件」とパス一覧を**最初から開いて**出し、「昇格」は
  `window.confirm` ではなく **sha12 を打たせる確認**にする（§4c）。
- `GET /releases` の `items[].changes` は、これに `stale`（`base` がいまの `current` と違う）と
  `commit_count` / `file_count` を足したもの。`stale` が真なら、その差分はもう「いま昇格したら」の話では
  ないので、`release.sh` を回し直すのがよい。

## 3. 検証する（`verify.sh`）

```bash
scripts/selfdeploy/verify.sh <sha12>
scripts/selfdeploy/verify.sh --dry-run <sha12>   # 前提だけ確かめる（何も起こさない）
SD_VERIFY_LOCK_WAIT=60 scripts/selfdeploy/verify.sh <sha12>   # 他の検証を 60 秒だけ待つ
```

やること（ADR-0040 D3 / ADR-0041 D2。本番には触れない）:

0. `~/.local/celeris/staging/.lock` を `flock` で取る（**1 本ずつしか走らない**）。`SD_VERIFY_LOCK_WAIT` 秒
   （既定 1800）待って取れなければ **exit 75** で「他の検証が走っている」と言って終わる
   （何も起こさず、何も消さない）。
1. `~/.local/celeris/staging/` を作り直し（`.lock` だけ残す）、
   `sqlite3 "file:~/.local/celeris/celeris.sqlite3?mode=ro" ".backup staging.sqlite3"`。
   **その場（マイグレーション前）で件数を数える**（検査 2 の基準）。
2. 新リリースの celeris を **verify モード**で `127.0.0.1:7711` に起こす
   （`--mode verify --db <snapshot> --listen … --workspace-root … --token-file … --release <sha12>`。
   設定は**本番の `config.toml` をそのまま読む**。上書きは CLI だけ）。
3. 検査:
   1. 起動し、`health.schema_version` が新バイナリの `SCHEMA_VERSION` と等しい（＝本番のデータで
      マイグレーションが通った）
   2. **件数一致（ADR-0041 D2 で意味が変わった）**: **同じスナップショットのマイグレーション前後**を比べる。
      前は `.backup` 直後に `sqlite3` で数えた**生の行数**、後は staging API（＝新バイナリが
      マイグレーションした同じファイル）。対象は `tasks` / `projects` / `milestones` / `org_nodes` /
      `approvals`（全件と決定済み） / `reports` / `messages` の件数と、`tasks` の `{id,status}` の
      ダイジェスト（sha256 の先頭 16 桁。両側**同じ式**で計算する）。
      **本番 API はもう一切叩かない**（本番は検証の間も動いているので、本番と比べると当たり前にずれて
      偽陰性になっていた。ADR-0041 §1-2）
   3. 主要 GET が 200 かつ JSON（`inbox` / `org/<最初のノード>/memory` / `notify` / `clusters` / `providers` / `config`）
   4c. web の読み取り e2e（任意）: `SD_VERIFY_WEB_HOOK` があれば `<sha12> <release> <api> <token>` で呼ぶ。
       指定した場合は成功が必須。例は [web 運用手順](web-operation.md)。
   5. **N-1 互換**: `~/.local/celeris/current/bin/celeris`（旧）を、**新バイナリがマイグレーションした後の**同じ
      スナップショットに対して `:7712` で起こし、1〜3 と同じ検査（件数は**スナップショット**と比べる。
      ここでも本番 API は読まない）。落ちたら `live_ok = false`（`current` が無い初回も `live_ok = false`）。
      **煙試験（6）はここではやらない**（旧バイナリは `smoke` を知らない）
   6. **煙試験（ADR-0041 D5）**: staging に 1 件だけタスクを流し、**dispatch → ワーカー起動 →
      結果の取り込み → レビュー → 終端 → 報告の生成**までを通す。詳しくは下の節
   4d. web/app を一時起動し `/healthz` と release の一致を確認する。失敗は verify 全体を落とす。
   任意 hook と N-1 は読み取りだけで並行する。煙試験は両方が終わってから行う。
4. `~/.local/celeris/releases/<sha12>/verify.json` を書く。`ok` は **1〜3・4d・（指定したときの）4c・6 が全部真**のとき。`live_ok` は 5。
   Phase SD-1 から、検査ごとの壁時計の秒数が `checks[].secs`、全体が `durations`
   （`lock_wait_s` / `prepare_s`〈staging の作り直し・スナップショット・数え〉/ `n1_s` / `total_s`）に入る。
   `checks[].elapsed_s` は従来どおり煙試験のタスクが `done` になるまでの秒数（検査 6 だけ）。
5. 起こしたプロセスは `trap` で必ず止める（自分が起こした pid だけ）。
6. staging のポートは env（`SD_STAGING_API_PORT` / `SD_STAGING_OLD_API_PORT`）で変えられる
   （`scripts/selfdeploy/tests/` 用。7710 / 7720 / 7721 は拒む）。

### 検査 6: 煙試験（ADR-0041 D5）

検査 1〜5 は「起動する・データが残っている・画面が出る」しか見ない。アダプタ・前置き・委譲・レビューの
回帰は素通りして本番に届いていた。検査 6 はそこを塞ぐ。

**verify モードの celeris は `genre = "smoke"` のタスクだけを dispatch する**（それ以外の ready は
従来どおり 1 件も動かさない。リースも奪わない、レビューも拾わない、`daemon_instances` にも書かない）。
`smoke` の**役割・分野・source・harness は verify モードが組み込みで足す**（`Config::apply_verify_smoke`）:

| | 中身 |
|---|---|
| `[llm_sources.smoke]` | `kind = "fake"`、`accounts = ["smoke"]`、`models = ["fake"]` |
| `[harnesses.smoke]` | `kind = "fake"`（起動コマンドは `FakeAdapter::default_command()`） |
| `[route_defaults.smoke]` | `candidates = [{ source = "smoke", model = "fake" }]` だけ |
| `[harness_selection]` | `genre = "smoke"` → harness `smoke` の規則 1 つだけ（worker run も reviewer run も偽の harness） |
| `[[roles]] id = "smoke"` | `harness = "smoke"`、`tier = "standard"`、`max_turns = 1`、`max_wall_secs = 60` |
| `[[genres]] id = "smoke"` | `description = "検証の煙試験"`、`default_role = "smoke"`、`roles = ["smoke"]` |
| `[adapters.fake].command` | `FakeAdapter::default_command()` に固定 |

verify の dispatcher が拾うのは `genre = "smoke"` かつ task の harness が `smoke` のタスクだけ
（`smoke` harness は上書きで必ず kind `fake` になる）。

設定ファイル（`~/.config/celeris/config.toml`）に同じ id があっても**上書きする**。本番の設定に何を書いても、
煙試験が本物の LLM を呼ぶ経路は無い（ADR-0041 §3「煙試験で本物の LLM を呼ばない。`fake` だけ」）。
これらが出るのは verify モードの `GET /config` だけで、本番の設定ファイルは 1 バイトも変わらない。

`verify.sh` がすること（**検査 5 の後**。ここで 1 件足すので、先に回すと検査 2 / 5 の件数がずれる）:

1. `POST /tasks`（staging のトークン）
   `{"title":"smoke","genre":"smoke","role":"smoke","acceptance":[{"type":"command","cmd":"true","expect_exit":0}],"assignee":"<最初の組織ノード>"}`
   — 受け入れ条件がコマンドなのでレビューにも LLM が要らない。`assignee` を付けるのは
   **報告の生成（ADR-0034）まで**回帰に入れるため（報告は `assignee` のあるタスクにだけ作られる）。
   そのノードがスナップショットに無ければ `assignee` 無しでもう一度作り、報告の段だけ飛ばす
2. `POST /tasks/{id}/approve`（draft → ready）
3. `GET /tasks/{id}` を 1 秒ごとに見て、**60 秒以内**に `status == "done"`
4. `GET /tasks/{id}/events` に `worker_started` と、`outcome` が `done…` の `worker_finished` がある
5. `GET /reports?node=<assignee>` にそのタスク（`task_id`）の報告が出る

結果は `verify.json.checks` の `id = 6` に `{ok, task_id, elapsed_s, detail}` として載り、
`verify.json.ok` の条件に入る。生の JSON は `~/.local/celeris/staging/logs/smoke.json`。

落ちたときの見方:

- `the smoke task is 'ready' after 60.0s` — dispatch されていない。`logs/celeris-new.log` に
  `no provider in the config matches this worker_hint` が出ていないか（`smoke` の組み込みが
  効いていない）
- `the smoke task is 'failed' after …` — ワーカーかレビューが落ちた。`GET /tasks/{id}` の
  `runs` と `~/.local/celeris/staging/workspaces/<task_id>/runs/` を見る
- `no report for the smoke task under node …` — 終端での報告の生成（ADR-0034）が壊れている

### 件数一致が落ちたとき

`verify.json` の `counts` は `{snapshot, staging}` の 2 つ（`prod` / `prod_after` は ADR-0041 D2 で
**消えた**）。両方とも**同じスナップショット**を見ているので、ずれたなら理由は 1 つ:
**マイグレーションが行を消したか変えた**。`~/.local/celeris/staging/staging.sqlite3` は残っているので、
その場で `sqlite3` で突き合わせられる。

1 つだけ外因がある。`reports` と `messages` は「全件を返す入口」が API に無いので、
**組織のノードごと**（`messages` はさらに案件ごと）に数えて足している。組織のノードを消した後に
残っている行（`node_id` が `org_nodes` に無い行）はそこから漏れるので、`snapshot` の方が多く出る。
そのときは失敗の一行に `orphan reports/messages ...` の数が出る。

`verify.sh` が **exit 75** で終わったときは検査ではなく**順番待ち**（他の `verify.sh` が走っている）。
`SD_VERIFY_LOCK_WAIT` を伸ばすか、しばらく待ってから回し直す。

### staging の見方

```
~/.local/celeris/staging/
  .lock                  直列化の flock（作り直しでも消さない。ADR-0041 D2）
  staging.sqlite3        本番 DB のコピー（新バイナリがマイグレーション済み）
  api.token              staging だけのトークン（毎回作り直す）
  workspaces/            verify モードの作業場所（煙試験の 1 件だけがここに出る。ADR-0041 D5）
  logs/celeris-new.log     新リリースの celeris の出力 ← 起動に失敗したらまずここ
  logs/celeris-old.log     N-1（旧バイナリ）の出力 ← `SchemaTooNew` はここに出る
  logs/counts-*.json     件数の生データ（snapshot = マイグレーション前 / staging / n1）
  logs/health-*.json     health の生データ
  logs/smoke.json        煙試験の生データ（`{ok, task_id, elapsed_s, detail, report}`。ADR-0041 D5）
```

`verify.sh` は次に走ったときに `staging/` を**丸ごと作り直す**。残したいログは先にコピーすること。

## 4. 昇格する（`promote.sh`。人だけ）

```bash
scripts/selfdeploy/status.sh | less        # verify.json.ok と live_ok を見る
scripts/selfdeploy/promote.sh <sha12>
```

- `verify.json.ok` が真でなければ**拒否する**（`--force` は無い）。
- `--pre-start <script>`: 停止→起動で、旧が止まり DB をバックアップした後・新を起こす前に 1 度だけ実行する
  （デーモンが止まっている間に DB を直接触る移行のため。引数は `<sha12> <release dir> <db path> <config path>`）。
  指定すると必ず停止→起動になる。失敗したら DB をバックアップから戻し、旧 unit を起こし直して止まる。
- ログは `~/.local/celeris/backups/promote-<ts>.log`。DB のコピーは `~/.local/celeris/backups/<ts>-pre-<sha12>.sqlite3`。
- 成功したら `~/.local/celeris/releases/<sha12>/promoted.json` に `{promoted_at, mode, from}` を書く
  （ADR-0041 D3。`GET /releases` の `promoted_at` と `status.sh` はこれを読む）。
- **失敗したら**（`sd_die` を含め、`set -e` でどこで止まっても）`~/.local/celeris/releases/<sha12>/promote_failed.json`
  に `{failed_at, error}` を書く（`error` は `promote-<ts>.log` の末尾 20 行）。バグ報告（2026-09-21）:
  `promote.sh` は API から detached で起こされる（§4c）ので、失敗しても呼び出し元の celeris は
  「起動できた」ことしか知らない。この印が無いと、GUI は「昇格が終わった（`promoting` が偽に戻った）」
  ようにしか見えず、成功したのか失敗したのか区別できなかった。次の昇格の試みが始まると消える
  （`POST /releases/{sha12}/promote` が spawn の直前に消す）ので、古い失敗が残り続けることはない。
  `GET /releases` の `items[].promote_failed` と `status.sh` の `promote_failed` はこれを読む。
- **git リポジトリには触れない。** `main` への反映は人がやる（次節）。
- 昇格の前に、`current`/`new` 以外に **active な `celeris@*` unit**（drain したまま終了しなかった
  前回昇格の残骸）があれば一覧を出す。既定では出すだけ。`promote.sh <sha12> --stop-stale` を付けると
  SIGKILL する（人が判断する操作なので既定はしない）。`status.sh` の `stale_instances` でも同じ一覧が
  見える（`current`/`previous` 以外に active な `celeris@*` の sha12 と pid）。

### 4a. 停止 → 起動（`live_ok` が偽・`/health` に `role` が無い・`--pre-start` を指定したとき）

1. 旧 celeris の pid は `current` の release の unit（`celeris@<sha12>`）の `MainPID`（Phase 119 D3）。
   systemd がその release を知らないときだけ、**設定パスまで含めた完全一致**で `/proc` を探す（`argv[0]` の
   basename が `celeris` で、`--config ~/.config/celeris/config.toml` を持ち、`--mode` / `--db` / `--listen` 等を
   持たないもの。staging のプロセスは当たらない）→ `SIGTERM` → `kill_grace_secs + 10` 秒待つ
2. 旧が終わってから DB をバックアップ（引き継ぎ中の仕事もコピーに入る）。`--pre-start` があればここで実行
3. `systemctl --user start celeris@<sha12>` → `/health` が 200 かつ `schema_version` が期待どおりになるまで 60 秒待つ
4. 駄目なら新を止め、旧 unit があれば起こし直し、**DB は戻さずに**失敗を報告する
   （スキーマが進んでいるかもしれないので、戻すかどうかは人が `rollback.sh --restore-db` で選ぶ）
5. `web-follow.sh` で web を切り替え、health と release を確認する（§4e）
6. `enable` 新 / `disable` 旧、`current` / `previous` の symlink を更新

**この間だけ API と GUI が止まる**（数十秒）。走っていたワーカーの run はリースが切れて新しい daemon が拾う。

### 4b. ライブ引き継ぎ（`live_ok` が真で、動いている celeris の `/health` が `role` を持つとき）

`verify.json.live_ok` が真で、動いている celeris の `/health` が `role` を持っているとき:

1. DB をバックアップ
2. `systemctl --user start celeris@<新>` → 新は `standby` で起き、同じポートに `SO_REUSEPORT` で bind する
3. `/health` を**毎秒 5 回ずつ**見て、5 回とも `release == <新>` かつ `role == active` になるまで 60 秒待つ
   （新旧が同じポートを共有するので 1 回の応答では判らない。5 回とも新なら旧はもう listener を閉じている）
   - ならなければ `systemctl --user stop celeris@<新>` して**失敗**。旧はそのまま動き続ける
4. `enable` 新 / `disable` 旧
5. `web-follow.sh` で web を切り替え、health と release を確認する（§4e）
6. `current` / `previous` を更新

**旧 celeris の drain 完了は待たない**。旧は `draining` になって自分が起こした run だけ面倒を見続け、
手元の run が 0 になったら exit 0 する（最長 `[handoff] drain_timeout_secs`）。進み具合は `status.sh` の
`daemon_instances` で見える。API が一瞬 `503 {"detail":"standby"}` を返す管理系（`reload` / `check` /
クラスタ接続 / アカウントのログイン / `notify/test`）があるが、窓は 1〜2 tick。

### 4c. web から昇格する（ADR-0040 D6）

web「リリース」画面（`/releases`）の「昇格」ボタンは `POST /releases/{sha12}/promote`（管理系）を叩き、
celeris が **`~/.local/celeris/current/scripts/promote.sh <sha12>`** を detached（`setsid`、stdin は
`/dev/null`、stdout/err は `<release>/promote.log`）で起こす。やることは shell から `promote.sh` を
叩くのと**同じ**。人の Web 操作に加え、人が対象版の昇格を依頼した場合は CoS の
`POST /api/v1/cos/operations` → `release.promote` も利用できる
（[CoS 全変更操作 ADR](../../agent-docs/adr/2026-10-09-cos-operations-all-mutations.md)、
[production skill](../../config/skills/cos-operator/production.md)）。CoS は `status.sh` の
`verify.json.ok` と `live_ok` を確認し、監査付きの経路を使う。通知だけの一次対応では昇格しない。
人の Web と CoS のどちらが実行するか一つに決め、operator は同じ版を重ねて起動しない。
CoS に直接の `promote.sh` 実行を許可したものではない。202 は受付であり、稼働版は別途確認する。

**どちらの `promote.sh` か**（ADR-0041 D4。Phase 50 で変わった）: **いま動いている版（`current`）**の
もの。昇格は「動いている本番を止めて／引き継いで替える」作業なので、その手順を知っているべきなのは
いまの本番の方。実装者が `scripts/selfdeploy/` を壊したリリースを作っても、その壊れた昇格スクリプトは
走らない（新しい昇格スクリプトは、それ自身が一度昇格されてから次の昇格で使われる）。
`current` に `scripts/` が無い（Phase 48 以前・初回）ときだけ昇格先のものを使い、
応答の `script_from` が `"current"` / `"target"` のどちらかを言う。

**安全に関わる変更があるリリース**（`changes.sensitive` が空でない。§2）は、画面が赤いバッジ
「安全に関わる変更 N 件」とパス一覧を**最初から開いて**出し、「昇格」ボタンは**押す前に sha12 を
打たせる**（打つまでボタンは灰色。`window.confirm` は出ない）。普通のリリースは従来どおり
`window.confirm` の二重確認。

```bash
# shell からと同じことを API で
curl -sS -X POST -H "Authorization: Bearer $(cat ~/.config/celeris/api.token)" \
     http://127.0.0.1:7710/api/v1/releases/<sha12>/promote
# → 202 {"sha12":"…","log":"~/.local/celeris/releases/<sha12>/promote.log","started_at":"…"}
```

- 断られるとき（409 `release_not_promotable`）: `verify.json` が無い／`ok` でない、既に `current`、
  既に昇格中（`promote.lock` の pid が生きている）、`current` にも昇格先にも `scripts/promote.sh` が無い
  （どちらも Phase 48 より前のリリース — その場合は shell から `scripts/selfdeploy/promote.sh` を使う）。
- ログは `<release>/promote.log`（**API では中身を出さない**）と、`promote.sh` 自身が書く
  `~/.local/celeris/backups/promote-<ts>.log`。進行は `GET /releases` の `instances`（旧 `draining` / 新 `active`）
  と `status.sh` で見る。
- **202 を返した celeris 自身がその昇格で `draining` になって終わる**（ライブ引き継ぎ）。API が一瞬
  切り替わるのは正常（新旧が `SO_REUSEPORT` で同じポートを共有する）。

`~/.config/celeris/config.toml` に足す設定は無い（`[selfdeploy] releases_dir` の既定が `releases` なので、
`~/.local/celeris/releases` をそのまま見る）。

### 4d. 昇格したら `main` に戻す（ADR-0041 D3）

昇格は `~/.local/celeris/` の中だけで完結し、あなたのチェックアウト（`~/workspace/agent-platform`）は
一切動かない。放っておくと「本番で動いているコード」が `main` に無い状態が続き、次のタスクが
古い `main` から分岐する。だから**昇格したら人が `main` に反映する**:

```bash
scripts/selfdeploy/status.sh | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["current"], [r["on_main"] for r in d["releases"] if r["is_current"]])'
# current が on_main: false なら
cd ~/workspace/agent-platform
git switch main
git merge --ff-only <sha12>     # 実装者のブランチ（celeris/<task-id>）の先端がその sha
```

- GUI の「リリース」画面は、**現行の行が `on_main: false` のとき**だけ
  「本番は main に未反映: `git merge --ff-only <sha12>`」と出す。
- `on_main` は `git -C <[selfdeploy] repo> merge-base --is-ancestor <sha> main` の結果。
  `repo` の既定は `~/workspace/agent-platform`（`~/.config/celeris/config.toml` に書かなくてよい）。
  リポジトリが無い・その sha を知らないときは `null` になり、GUI は何も言わない。
- `--ff-only` なので、**`main` が先に進んでいたら止まる**。そのときは実装者のブランチを
  `main` に rebase してから `release.sh` をやり直すのが早い（本番より古いコードを `main` に混ぜない）。

### 4e. web/ の追従（`web-follow.sh`。ADR-0081 / web ADR-W3 付記 2026-10-02）

`promote.sh` は daemon の切り替え後、current の更新前に `web-follow.sh <new_sha12> <old_sha12>` を呼ぶ。
新しい web gate と配布物・依存を確かめ、一時起動に成功してから旧 unit を止めて新 unit を起動する。
旧 web が無い初回も新 web を起動する。health の release が一致したら current を更新する。
失敗時は旧 web が active だった場合だけ復旧し、promote は非0で終了する。daemon は新しい版の場合がある。
ログを確認し、修復後に同じ promote を再実行する。current が更新されていなければ旧 web の情報を再利用できる。

`celeris-web@` は daemon の `Wants=` を持たない。web の再起動だけで daemon の handoff は発生しない。
web は昇格した release の `web/app` から動かし、一時的な作業場所や staging を参照させない。
設定と確認方法は [web 運用手順](web-operation.md)。

## 4d. リリースの説明と昇格の要約（ADR 2026-10-04-release-notes）

`release.sh` は `<release>/notes.json`（機械可読）と `notes.md`（人が読む版）を `celerisctl release notes` で書く。
`base..sha` の first-parent を、配送記録（`GET /deliveries`）と `celeris/<ULID>` の branch 名で Celeris の task
ごとにまとめ、task に属さない直接の commit・DB migration と schema_version の変化・ADR・
`config/celeris.example.toml` の変更（本番 config に足すものがないか人が見る）・gate で飛ばした段を載せる。
決定的で、LLM は使わない。**生成が失敗してもリリースは作られる**（警告だけ。`notes.json` が無いリリースは説明なし）。

昇格の前には「`current` から対象までに入る全リリース」の要約を見る: web「リリース」画面の各行、
`GET /api/v1/releases/{sha12}/promotion-preview`、`celerisctl release preview <sha12>`（`promote.sh` がログにも出す）。
同じ task は 1 回、schema は `current` と対象の値、方式（`live` / `stop-start`）は対象の `verify.json` から出る。
`complete: false` のときは `current` まで notes で辿れていない（理由が `problem` に出る）。
注意: `POST /promote` は `current` に同梱の `promote.sh` を走らせるので、ログ出力と `promoted.json` への
「含まれるリリース・task」の記録は、この機能を含む版が `current` になった後の昇格から効く。

## 5. 戻す（`rollback.sh`。人だけ）

```bash
scripts/selfdeploy/rollback.sh              # previous を昇格し直す
scripts/selfdeploy/rollback.sh --restore-db # DB も昇格前に書き戻す（昇格後の仕事は失われる）
```

- 旧バイナリの `SCHEMA_VERSION`（`previous/manifest.json`）が DB の `schema_version`（`/health`、
  取れなければ `sqlite3 "select max(version) from schema_migrations"`）**以上**なら、そのまま
  `promote.sh previous` と同じことをする（live か停止→起動かは `previous` の `verify.json.live_ok` 次第）。
- 旧の方が低い（`SchemaTooNew` で起動できない）ときは**拒否する**。`--restore-db` を付けたときだけ:
  1. `~/.local/celeris/backups/` の直近の `*-pre-*.sqlite3` を選ぶ（`*-pre-rollback.sqlite3` は選ばない）
  2. いまの DB を `<ts>-pre-rollback.sqlite3` に退避
  3. celeris（unit か、完全一致で見つけた pid）を止める
  4. 選んだコピーを `celeris.sqlite3` に上書きし、`-wal` / `-shm` を消す
  5. `celeris@<previous>` / `celeris-web@<previous>` を起こす → symlink を更新

**書き戻すと、昇格してから今までに進んだ仕事は消える。** どちらが損かを人が決める。

## 5b. DB の置き場を移す（`relocate-db.sh`。人だけ）

本番の DB は既にローカルディスクの `/var/lib/celeris/celeris.sqlite3` にある（`/home` は NFS で fsync が遅く、
`database is locked` が起きたため。ADR-0064）。状態ディレクトリ本体（workspaces / releases / backups / tools）は
`~/.local/celeris` のまま。もう一度別の場所へ移すときだけ使う（`<新パス>` のディレクトリは人が先に作る）:

```bash
scripts/selfdeploy/relocate-db.sh <新パス> --dry-run   # 計画と in-flight の確認だけ
scripts/selfdeploy/relocate-db.sh <新パス>             # 実行
```

手順（[ADR-0064](../../agent-docs/adr/0064-db-local-disk-and-store-resilience.md) D2）:

1. `GET /api/v1/tasks` の `counts_by_status`（`running` + `reviewing`）が 0 であることを確認する。
   0 でなければ何もせず止まる（待つか、`--dry-run` で様子を見てから改めて実行する）。
2. `current` の sha の `celeris@<sha>` / `celeris-web@<sha>` を止める。
3. `sqlite3` があれば `VACUUM INTO`、無ければ現在のリリースの `celerisctl db backup`（読み取り専用接続の
   `VACUUM INTO`。ADR-0064 D2/D3、方式は ADR 2026-10-16-mlsys-pilot-wiring-fixes D8）で新しい場所へコピーする。
4. 新しいファイルに `PRAGMA integrity_check`。`ok` でなければコピーを消して止まる
   （**旧 DB とサービスには触れない**）。
5. `config.toml` の `db =`（または `[db].path`）を書き換える。`config.toml.bak-<ts>` を残す。
6. 旧ファイルを `<旧パス>.moved-<ts>` に**リネーム**する（削除しない）。
7. unit を起こす。
8. `GET /api/v1/config`（認証つき）の `db` が新パスになっていることを確認する。**`GET /health` は
   無認証なので DB の絶対パスは出さない**（`db.filesystem` / `db.device` だけ出る。ADR-0064 D1）。

各段階の失敗は、その時点までに済んだことと**手で戻す手順**を stderr に出して exit 1 で止まる
（自動では戻さない）。冪等（`db` が既に指定パスなら何もせず exit 0）。ディレクトリは作らない
（無ければ「先に `sudo install -d` してください」と言って止まる）。

移した後は、`[db]` テーブルで `busy_timeout_ms`（本番では 15000 を勧める）・
`checkpoint_interval_secs`（既定 30）・`backup_dir`・`backup_interval_secs`（既定 3600）・
`backup_keep`（既定 48）も書ける（§8）。`backup_dir` を書くと、celeris がその間隔で
`celeris-<unix_ts>.sqlite3` を自分の背景タスクで書き、世代を`backup_keep`件だけ残す
（DB がローカルディスクにあると NFS 側のスナップショットに乗らなくなるため）。

## 6. いまを見る（`status.sh`）

```bash
scripts/selfdeploy/status.sh
```

JSON 1 つ。`current` / `previous`、`releases[]`（`gate`（各段の exit と秒数）、`verify`（`ok` / `live_ok` /
落ちた検査の名前）、`promoted_at` / `promoted`（`promoted.json`）、`promote_failed`（`promote_failed.json`。
直近の失敗が残っていれば `{failed_at, error}`、無ければ `null`）、`on_main`、`changes`（`base` / `stale` /
`commit_count` / `file_count` / `sensitive`））、本番 `:7710` の `/health`、`:7720` の `/healthz`、
`daemon_instances`、`backups` の新しい 10 件。**読むだけ**なので誰が実行してもよい
（`on_main` の git も `merge-base --is-ancestor` だけ。`$SD_REPO` に `main` が無ければ `null`）。

## 7. 禁止（ADR-0040 D5）

celeris の上の「人」（ワーカー）が自己改善の案件でやってよいのは **`release.sh` と `verify.sh` まで**。

やってはいけないこと:

- `promote.sh` / `rollback.sh` / `install-units.sh` を実行する（リリースの中の `<release>/scripts/*.sh` も同じ）
- 通常 worker が `POST /releases/{sha12}/promote` を叩く（人の Web 操作、および §4c の人の指示に基づく CoS の監査経路だけを認める）
- `systemctl` を叩く（本番の unit を start / stop / restart / enable / disable する）
- `~/.config/celeris/config.toml` を編集する
- `~/.local/celeris/*.sqlite3` に書き込む（読むのは `sqlite3 "file:…?mode=ro"` と `.backup` だけ）
- 本番のプロセスに `kill` などのシグナルを送る
- `127.0.0.1:7710` / `127.0.0.1:7720` に bind する
- `main` に直接コミットする（実装者は **celeris が用意した worktree とブランチ `celeris/<task-id>`** にコミットする。ADR-0041 D1）
- `git checkout` で作業ツリーのブランチを変える・自分でブランチを切る（作業ツリーは celeris がタスクごとに用意する）

実装者は `gate.json` / `verify.json` を `artifacts/` に写し、報告に「検証済み sha」を書く。
昇格は人が版を決定し、`status.sh` で `verify.json.ok` と `live_ok` を確認してから §4c の一つの経路で行う。

## 8. config.toml について

selfdeploy のために `config.toml` へ足す設定は無い。省略したときの既定（ADR-0045 D2）:

| 設定 | 省略時の既定 |
|---|---|
| `db` | `~/.local/celeris/celeris.sqlite3` |
| `workspace_root` | `~/.local/celeris/workspaces` |
| `[selfdeploy] releases_dir` | `~/.local/celeris/releases` |
| `[memory] dir` | `~/.local/celeris/memory` |
| `[containers] build_dir` | `~/.local/celeris/containers` |
| `[secrets] dir` | `~/.config/celeris/secrets` |

ADR-0064 D1: `db` は文字列でも、`[db]` テーブルでもよい
（`db = "<path>"` と `[db] path = "<path>"` は同じ意味）。テーブルにすると追加のキーが書ける:

| `[db]` のキー | 省略時の既定 |
|---|---|
| `path` | `~/.local/celeris/celeris.sqlite3`（`db = "..."` と同じ既定） |
| `busy_timeout_ms` | 5000（本番では 15000 を勧める。`relocate-db.sh` の後に書き足すとよい） |
| `checkpoint_interval_secs` | 30（背景チェックポイントの間隔。ADR-0064 D5） |
| `backup_dir` | 無し（無ければ定期バックアップをしない） |
| `backup_interval_secs` | 3600 |
| `backup_keep` | 48 |

**書いてあれば従来どおり**（相対パスは設定ファイルのディレクトリ基準）。`[api] token_file` と
`[accounts] claude_dir` / `codex_dir` には**暗黙の既定を入れない**: 「書いていない」こと自体が
「認証を使わない（ADR-0013 D3）」「そのプールを設定していない（ADR-0024 D2 / ADR-0025 D1）」という
意味を持っているため。推奨の置き場（`~/.config/celeris/api.token`、
`~/.local/celeris/{claude,codex}-accounts`）は `config/celeris.example.toml` に書いてある。

`[selfdeploy] repo` も既定が
`~/workspace/agent-platform`（`~` は celeris の `$HOME` で展開）なので、書かなければそのまま当たる
（読むのは `on_main` のためだけで、**書き換えることは無い**。無ければ `on_main` が `null` になるだけ）。
どちらも別の場所に置きたいときだけ書く。`[handoff] drain_timeout_secs`（既定 3600）は旧 celeris が
draining のまま run の面倒を見る上限（§4b）。
`promote.sh` が読むのは既存の `kill_grace_secs` だけ（無ければ 10 秒）。
`verify.sh` は `config.toml` を**そのまま**新リリースに読ませる（本番の設定が新しいバイナリで通るかを
見るのが目的なので、上書きは CLI フラグだけ）。

## 9. 部署レビューから自動で候補を準備する（ADR-0051）

自動取り込みは**案件の属性**（`auto_integrate`。既定は無効＝手動取り込み。ADR 2026-10-16-project-auto-integrate）。
`PATCH /api/v1/projects/{id}` に `{"auto_integrate": true}` を送るか、Web の案件画面の「自動取り込み」で切り替える
（reload・再起動は要らない）。有効な案件の root task は、次のReviewer runでマージ可否も判定する。
部署長の別runが既存レビューを担当するため、CoSに技術的な再承認は求めない。
合格した固定SHAを task の repo（案件のローカルリポジトリ）の既定ブランチへfast-forwardする。
取り込み先が `[selfdeploy] repo`（自己リポジトリ）なら、そのあとrelease/verifyを準備し、成功後にCoSからデプロイ候補を通知する
（本番昇格は人が決定し、リリース画面または §4c の CoS 経路で行う）。**それ以外のリポジトリは取り込みで止まり**、release・promoteには進まない
（CoSには「取り込み完了」を通知する）。push は `[selfdeploy] push` が有効で、取り込み先に `push_remote` が実在するときだけ行う。
旧 `[selfdeploy] delivery_projects` は廃止（書くと config の読み込みエラー）。

完了済みの仕事を引き渡すには、管理API `POST /tasks/{id}/rereview` に
`{"expected_status":"done"}` を送る。実装をやり直さず、現在のコミットを再判定する。
競合・承認後の変更は部署へ差し戻し、既存の再開・修正経路を使う。マージ・ビルドの技術的失敗は一度だけ自動で実装担当へ戻し、同じタスクで無限に修正を繰り返さない。
準備のログは `<releases_dir>/.deliveries/<task-id>/<sha>/prepare.log` に残る。
`prepare.sh` は release 全体に 7200 秒、verify に 900 秒の上限を掛ける。
Cargo の clean と全 workspace テストに時間がかかる場合も、gate 成功後の梱包と
作業ツリーの掃除まで release の期限に含める。


### 9a. push_error の確認と復旧（non-fast-forward）

`GET /api/v1/tasks/<id>/changes` の `delivery` で `push_error`・`pushed_at`・対象 commit・state を確認する。
`GET /deliveries` は task/commit 対応の一覧であり、診断には task の changes を使う。
[ADR-0051](../../agent-docs/adr/0051-supervised-delivery.md) と `crates/celeris/src/delivery.rs` のとおり、
push 失敗は release 準備を止めない。自動再試行は一度だけで、なお失敗すると内部の `[retried]` 印が付き、
以後は繰り返さない。`ready` や本番稼働を remote 同期済みの証拠にしない。

non-fast-forward は、まず人が指定した repo/remote/default branch と対象 delivery を照合し、
CoS・operator のどちらか一人が診断を担当する。以下は**人が実行する手順**。値は delivery と登録 repo から指定する。
fetch は remote に書かないがローカル追跡 ref を更新するため、進行中の取り込みと重ねない。

```bash
repo=/path/to/registered/repo
remote=origin
branch=main
# reviewed は delivery の対象 commit（実際の値を指定）
reviewed='<delivery-commit>'
git -C "$repo" status --short
# 認証失敗なら停止。対話認証を自動反復しない
GIT_TERMINAL_PROMPT=0 git -C "$repo" fetch "$remote" "$branch"
git -C "$repo" rev-list --left-right --count "refs/heads/$branch...FETCH_HEAD"
git -C "$repo" merge-base --is-ancestor "$reviewed" FETCH_HEAD
```

- 最後が exit 0: remote は対象 commit を含む。push は不要。fetch 時刻と包含確認を復旧証拠にする。
- 対象を含まず、`git -C "$repo" merge-base --is-ancestor FETCH_HEAD "refs/heads/$branch"` が exit 0:
  remote がローカルの祖先。未承認 commit が混じっていないことと既存 push 認可を確認した人が
  `git -C "$repo" push "$remote" "refs/heads/$branch:refs/heads/$branch"` を一度実行し、再 fetch と対象包含で成功を確かめる。
- 分岐・ローカルが後退・対象 commit 不明・認証失敗: 自動再試行を止め、履歴とエラーを元 task に添えて人へ戻す。
  force push、共有 main の reset/rebase、無関係な再起票はしない。統合修復が必要なら既存のレビュー経路で扱う。

手動 push 後の古い `push_error` を解除する専用の管理 API は現状ない。復旧済みでも履歴に残り得る。
本番 DB の delivery JSON を直接書き換えず、元 task に観測時刻・対象・remote の包含・コマンド結果を追記する。
「remote は復旧済み／過去の push_error は残存」を分けて報告する。表示を消すための rereview・release・promote はしない。

## 配備操作の監査情報

`promote.sh` / `rollback.sh` は `SD_ACTOR`・`SD_REASON`・`SD_POLICY_REF` を必須とする。
実行者は `human`、`operator:<name>`、`cos`、`system:<name>`。OS の USER は実行者の代用にしない。
`SD_TASK_REFS` と `SD_DECISION_REFS` は JSON 文字列配列（関係するものが無ければ省略して `[]`）。
人が実行する例:

```sh
SD_ACTOR=human SD_REASON='検証済み修正の配備' SD_POLICY_REF='利用者の配備承認への参照' \
  SD_TASK_REFS='["task-id"]' SD_DECISION_REFS='["decision-id"]' \
  bash scripts/selfdeploy/promote.sh <sha12>
```

rollback も同じ環境変数を指定する。`--restore-db` のデータ復元に関する注意は従来どおり。
対象 release の `audit/*.json` に各操作の開始時の verify 全体・実行者・理由・根拠・参照・結果を保存する。
成功時は `promoted.json` / `rolled_back.json` にも写す。`status=started` のままなら正常終了の証拠は無い。
`GET /api/v1/releases` と `status.sh --json` の `items[].deployment_audit` / `releases[].deployment_audit`
で履歴を読める。過去の release の欠けている実行者は推測で補わない。
