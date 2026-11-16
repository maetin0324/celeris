---
title: 健全性v2 V0 candidate — release 候補の作り方・確かめ方・rollback・本番への移行と判定 script
tasks: [01M6TYVJT97XH88TEEAVRD7JK6]
status: done
updated: 2026-11-16
completed: 2026-11-16
---
# 健全性v2 V0 candidate

完了日: 2026-11-16（この WorkUnit の完了。task 全体の完了は段 close と review が決める）

## 結論
- 人（operator）がやること: 段 verify の統合の後に、host で §1 の手順 1〜4（`release.sh` → `verify.sh` →
  web の e2e → `release-check.py`）を実行する。**promote（§1 手順 5）は人の承認の後だけ**。この task も worker も promote しない。
- 判定は `release-check.py`（この dir）が決める。gate.json が ok、verify.json が ok、その sha と task branch の HEAD の差が
  `agent-docs/progress/` の下だけ、の 3 つを満たす release があれば sha12 を出して exit 0。段 close はこれが exit 0 のときだけ done にする。
- 提出時の事実（本番の releases dir と DB を読み取りだけで確認）:
  - 本番の `current` は `bae3149de834`（R6 を含む。2026-11-15T23:51Z に live で昇格。schema_version 126）、`previous` は `a1bed652bba1`。
  - 提出時の HEAD（件名「integrate wu/targets (phase measure)」）と `bae3149de834` の差は `agent-docs/progress/` の下だけ
    （それ以外の path は 0 件）。`release-check.py --releases /local/celeris/state/releases` は `bae3149de834` を出して exit 0。
    B0 の基準（slices.json の base_sha `5923a76c9aa0`）から HEAD までのコードの変更は、すでに本番で動いている。
  - V0 の各 leaf は crates・web・control-ts・scripts・tests を変えない計画なので、段 verify の統合の後も同じ判定になる見込み（未確認。
    統合の後に手順 4 で確かめる）。
- 未検証: release gate（`release.sh`）は web の Playwright e2e を走らせない（gate.json の `web.steps` は web-config・
  web-pnpm-install・web-pnpm-typecheck・web-pnpm-test・web-pnpm-release の 5 段。`web-pnpm-test` は vitest と `node --test`）。
  ADR 2026-10-20-full-tests-at-release-only の D1 は web の e2e を release gate に含めると書いている。task の受け入れ条件 0 の
  「web e2e」は、§1 手順 2 で人が別に実行するまで未検証。

## 1. 候補を作る・確かめる（人が host で実行する）

以下は `~/workspace/agent-platform`（`release.sh` の `SD_REPO` の既定）で実行する。`$B` は task の branch。

```sh
cd ~/workspace/agent-platform
B=celeris/01M6TYVJT97XH88TEEAVRD7JK6
```

0. **前提: 段 verify の統合が終わっている。** branch の最新の commit が段 verify の統合（件名「integrate wu/… (phase verify)」）か、
   その後の commit であることを確かめる。
   ```sh
   git log -3 --format='%h %s' "$B"
   ```
1. **release を作る（release gate）。** gate は fmt・control-ts・script bundle・workspace 全体の nextest（`CELERIS_USERNS_TESTS=1`）と
   doc test・clippy・source-size-report・build・web の 5 段を回す（前回 `bae3149de834` は nextest 713 秒・doc test 16 秒）。
   ```sh
   scripts/selfdeploy/release.sh "$B"
   S=$(git rev-parse --short=12 "$B")
   python3 -c 'import json,sys; g=json.load(open(sys.argv[1])); print(g["ok"], repr(g["failed_step"]), g["cargo_test"]["failed"], g["web"]["ok"])' \
     /local/celeris/state/releases/$S/gate.json
   ```
   期待: `True '' 0 True`。gate が落ちると release は公開されず、gate.json とログは
   `/local/celeris/state/releases/.build/$S/gate.json` に残る（`failed_step` を見る）。`release.sh` は最後に古い release を消す
   （current・previous と新しい 3 件を残す）。消したくない release があれば `SD_RELEASE_PRUNE=0` を付ける。
2. **web の e2e（gate に無い段）。** 同じ sha の detached の作業ツリーで流す。
   ```sh
   git worktree add --detach "$TMPDIR/v0-e2e" "$S"
   pnpm -C "$TMPDIR/v0-e2e/web" install --frozen-lockfile --offline
   pnpm -C "$TMPDIR/v0-e2e/web" exec playwright install chromium   # browser が無いときだけ
   pnpm -C "$TMPDIR/v0-e2e/web" e2e; echo "exit=$?"
   git worktree remove --force "$TMPDIR/v0-e2e"
   ```
   期待: exit 0。`e2e` は `vite preview` で dist を配るだけで、本番の API には繋がない（`web/playwright.config.ts`）。
   offline の install・browser の有無は host の状態しだい（未確認）。
3. **検証する（staging）。** 本番 DB の読み取りの snapshot に新しい celeris を 127.0.0.1:7711 で起こし、件数一致・主要 GET・
   N-1 互換・煙試験・web app の起動を確かめる。本番の API とプロセスには触れない。
   ```sh
   scripts/selfdeploy/verify.sh --dry-run "$S"   # 前提だけ確かめる（任意）
   scripts/selfdeploy/verify.sh "$S"; echo "exit=$?"
   python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print(v["ok"], v["live_ok"], v["schema_version"], [c["name"] for c in v["checks"] if not c["ok"]])' \
     /local/celeris/state/releases/$S/verify.json
   ```
   期待: exit 0、`True True 126 []`（`live_ok` が `False` なら promote は停止→起動になる）。exit 75 は他の `verify.sh` が走っている
   ので、終わってからもう 1 回実行する。
4. **判定する。** script は branch にあるので、`git show` で取り出して読み取りだけで実行する。
   ```sh
   git show "$B:agent-docs/progress/2026-11-15-health-v2-v0/release-check.py" > "$TMPDIR/release-check.py"
   python3 "$TMPDIR/release-check.py" --releases /local/celeris/state/releases --repo . --head "$B"; echo "exit=$?"
   ```
   期待: stdout に sha12 が 1 行、exit 0。exit 1 なら stderr に release ごとの理由（gate・verify・`agent-docs/progress/` の外の
   差分）が出る。段 close の worker は自分の作業ツリー（HEAD = branch）で
   `python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --releases /local/celeris/state/releases` を実行する。
5. **promote（人の承認の後だけ。この task では実行しない）。** 承認が出たら、§3 の表の「昇格前」の確認をしてから:
   ```sh
   scripts/selfdeploy/status.sh | less     # verify.json.ok と live_ok を見る
   SD_ACTOR=human:<名前> SD_REASON='<理由>' SD_POLICY_REF='<承認の参照>' scripts/selfdeploy/promote.sh "$S"
   readlink /local/celeris/state/current                       # → releases/$S
   curl -s http://127.0.0.1:7710/health | python3 -c 'import json,sys; h=json.load(sys.stdin); print(h["schema_version"])'   # → 126
   ```
   手順 4 が既存の release（例 `bae3149de834` = いまの current）を出したときは、その release はすでに本番で動いているので、
   昇格の操作は要らない。新しい sha12 を昇格しても、コードは current と同じ（差は `agent-docs/progress/` の下だけ）。

## 2. 戻す（rollback。人だけ。docs/ops/selfdeploy.md §5）

```sh
SD_ACTOR=human:<名前> SD_REASON='<理由>' SD_POLICY_REF='<参照>' scripts/selfdeploy/rollback.sh
```

- `rollback.sh` は `previous` を昇格し直す。V0 の候補を昇格した後の `previous` は `bae3149de834`（schema 126）で、DB の
  schema_version も 126 のままなので、`--restore-db` は要らない（旧の `SCHEMA_VERSION` ≥ DB の schema_version の場合）。
- `--restore-db` は旧の方が schema が低いときだけ使う。直近の `*-pre-*.sqlite3` に書き戻すので、**昇格してから今までの仕事は消える**。
  この候補では schema が変わらないので使わない。
- 確認:
  ```sh
  readlink /local/celeris/state/current /local/celeris/state/previous     # current が rollback 先の sha12 に戻る
  curl -s http://127.0.0.1:7710/health | python3 -c 'import json,sys; h=json.load(sys.stdin); print(h["schema_version"])'
  scripts/selfdeploy/status.sh | python3 -c 'import json,sys; s=json.load(sys.stdin); print(s["current"], s["previous"])'
  ls -t ~/.local/celeris/backups/ | head -3                                 # rollback の log と DB の退避
  ```

## 3. 本番への移行（B0 の基準 `5923a76c9aa0` から HEAD まで）

集めた範囲: 健全性 v2 の G1〜R6 と health 関連 task（01M76BYGKY7R8SBZ22AZGJ72SP・01M79CHBRY3CBHKPNECPPM6G32、
2026-11-10-health-phase-base）の進捗 file と unit record の「本番への移行」節。migration は B0〜HEAD の
`crates/task-core/migrations/` の差分（health 以外の task の分も含む）から数えた。`$DB` は本番 DB
（`/local/celeris/data/db/celeris.sqlite3`）で、確認はすべて読み取り（`sqlite3 -readonly`）。

「current」列は、本番の current（`bae3149de834`）にその変更が入っているか。HEAD と current の差は `agent-docs/progress/` の
下だけなので、表のすべての行が current に入っている。

| # | 出所 | 種類 | 変化 | current | 人の作業 | 確認の command（期待） |
|---|---|---|---|---|---|---|
| 1 | G3 inventory | migration | 0121 `health_inventory_runs`（表を足すだけ） | 入っている | 無し | `sqlite3 -readonly "$DB" "select version from schema_migrations where version in (121,122)"`（121 と 122） |
| 2 | G3 limits | migration | 0122 events に `dependents_reopened` の部分 index | 入っている | 無し | 1 と同じ |
| 3 | 範囲内の他 task | migration | 0123 authority_global_routine・0124 cos_operator_route・0125 chat_run_knowledge_use・0126 repo_auto_integrate。schema_version は 120 → 126 | 入っている | 無し | `sqlite3 -readonly "$DB" "select max(version) from schema_migrations"`（126）。候補では verify.json の `schema_version`（126） |
| 4 | R6 scratch | config 欄の削除（残ると読み込みエラー） | `[scratch] l1_max_gb` | 入っている | 残っていればその行を消す（読まれていなかった欄） | `grep -rn 'l1_max_gb' ~/.config/celeris/`（0 件。運用者が 2026-11-15 に確認済み） |
| 5 | R6 routing | config 欄の削除（残ると読み込みエラー） | `[execution] cheap_local_first` | 入っている | 残っていればその行を消す | `grep -rn 'cheap_local_first' ~/.config/celeris/`（0 件。include 先も含む） |
| 6 | R6 console-api | API の欄の撤去 | `GET /metrics/scratch` の `cache.stats`（常に null）、`GET /console`・`/console/stream` の block の kind `milestone` | 入っている | repo の外の client があれば直す（repo の中には無い） | `grep -rnE 'metrics/scratch\|console/stream' ~/.config/celeris ~/bin 2>/dev/null`（0 件か、呼び手が `cache.stats`・`milestone` を読まない） |
| 7 | R6 legacy | CLI の撤去 | `celerisctl plan` を消した。新しい仕事は root task と `celerisctl execution plan set` | 入っている | 運用手順の `celerisctl plan` を `celerisctl execution plan set` に書き換える | `/local/celeris/state/current/bin/celerisctl plan --help; echo $?`（非 0。2026-11-16 に 2）、`grep -rnE 'celerisctl plan( \|$)' ~/.config/celeris ~/.config/systemd/user`（0 件） |
| 8 | R6 cos-cli | CLI の接続先 | `celerisctl` の cron・integration・models・script も `CELERIS_CONFIG`・`CELERIS_API_URL` を読む | 入っている | 無し（設定した shell ではその URL に繋ぐ） | `env \| grep -E '^CELERIS_(API_URL\|CONFIG)='`（運用の shell の値を確かめる） |
| 9 | R6 research | 挙動 | langmem・local-deep-research・paperqa の wall clock 超過は `budget_exhausted`、result.json の無い成功は retryable な失敗 | 入っている | 無し | `readlink /local/celeris/state/current`（`releases/bae3149de834` 以降）。挙動の観察は次の研究 run の events（未確認） |
| 10 | G3 evidence・loop-points | 証跡の定義指紋 | 必須検査の指紋 v3（`celeris.required-checks/3`）。昇格の時点の `MergeQueued` の配送は 1 回だけ再 review | 入っている | 無し | `grep -a -c 'celeris.required-checks/3' /local/celeris/state/current/bin/celeris`（1 以上。2026-11-16 の読み取りで 1） |
| 11 | G3 bundle-path | 同梱の検査器 | `<release>/repo-health/` を daemon が起動時に読む | 入っている | 無し | `test -f /local/celeris/state/current/repo-health/CHECKER_TREE && echo ok`、`journalctl --user -u "celeris@$(basename "$(readlink /local/celeris/state/current)")" \| grep -m1 'repo health: bundled checker'` |
| 12 | G3 limits | event・受信箱の種類 | event `health_findings_repeated`・`dependents_reopened`、受信箱 `health_repeat`・`dependents_reopened`、遷移の reason `review_blocked`。health の往復と上限の使い切りで task が Blocked になる | 入っている | 受信箱の項目には源の task への comment で答える | `sqlite3 -readonly "$DB" "select json_extract(json,'$.type') t, count(*) from events where t in ('health_findings_repeated','dependents_reopened') group by t"`（2026-11-16 の読み取りで 3 件・8 件） |
| 13 | G3 inventory | 運用手順（任意） | 全体棚卸しの cron job。既定では job が無い | job は無い（`health_inventory_runs` 0 行） | 有効化するなら docs/ops/cron-jobs.md §9.3（`--enabled false`・`--overlap skip`・週 1 回、1 案件 1 件）、止めるときは §9.4 | `sqlite3 -readonly "$DB" "select count(*) from health_inventory_runs"`（有効化前は 0） |
| 14 | G3 CI | 運用手順（任意、repo の管理者） | CI に `scripts/dev/repo-health/ci.sh` を足し、branch 保護で必須の check にする（docs/guides/repo-health-ci.md） | — | 任意 | `sh scripts/dev/repo-health/ci.sh --help`（引数の説明が出る。ci.sh は `sh` で呼ぶ） |
| 15 | 2026-11-10-health-phase-base | event の値 | `IntegrationCheckFinished.health.base_sha` が段の基点でなく main との merge-base になる | 入っている | 無し | `sqlite3 -readonly "$DB" "select task_id, json_extract(json,'$.health.base_sha') from events where json_extract(json,'$.type')='integration_check_finished' order by id desc limit 3"` |
| 16 | R4 fix-integ | 配備前の確認 | 更新前の daemon が残した、`PhaseIntegrated` の無い done の統合 WU を記録し直さない | 入っている | 昇格の前に下の SQL で 0 件を確かめる。1 件以上なら次の段が人の確認かを見る | `sqlite3 -readonly "$DB" "SELECT w.task_id, w.key FROM work_units w JOIN tasks t ON t.id = w.task_id WHERE w.kind = 'integrate' AND w.status = 'done' AND w.phase IS NOT NULL AND t.status NOT IN ('done','failed','cancelled') AND NOT EXISTS (SELECT 1 FROM events e WHERE e.task_id = w.task_id AND json_extract(e.json,'$.type') = 'phase_integrated' AND json_extract(e.json,'$.work_unit_id') = w.id);"`（0 件） |
| 17 | G4 fix-review-gap | 挙動 | `[review]` の無い profile・generic の repo の final review が `review.json` の `health` を求める。旧い鍵の `HealthReviewInputBuilt` は 1 回作り直す | 入っている | 無し | `sqlite3 -readonly "$DB" "select count(*) from events where json_extract(json,'$.type')='health_review_input_built'"`（2026-11-16 の読み取りで 117） |
| 18 | 01M79CHBRY3CBHKPNECPPM6G32 | 挙動 | WU・段の統合の health の base を、review target を取り込んだ merge にする | 入っている | 無し（task の手順 3 の「再検査」は R4 の完了までに済んでいる。R4 は done） | `sqlite3 -readonly "$DB" "select id, status from tasks where id in ('01M79CHBRY3CBHKPNECPPM6G32','01M6TYVJT4S29ZMP6P0D4831JG')"`（どちらも done） |
| 19 | 01M76BYGKY7R8SBZ22AZGJ72SP | 検査器（policy だけ） | 検査器の欠け G1（自分の module を import できないと exit 1）と G2（包んだ検査の script が無いと fail）を直した | 入っている | 無し | `sqlite3 -readonly "$DB" "select status from tasks where id='01M76BYGKY7R8SBZ22AZGJ72SP'"`（done） |
| 20 | R2 | 挙動 | credentiald の受付の変更（config・API・DB は変えていない） | 入っている | 無し | `readlink /local/celeris/state/current`（`releases/bae3149de834` 以降） |
| 21 | G1・G2・R1・R3・R5・G4 本体・hotfix-health-arch-map-side | — | 「本番への移行」節が無いか「無し」。config 欄・API の欄・CLI・migration の撤去や追加の記述は無い | — | 無し | `grep -n '本番への移行' <各進捗 file>`（節が無い・「無し」） |

表の外のこと:
- config の欄を足した変更は無い。消した欄は 4・5 の 2 つだけ（健全性 v2 の範囲。他の task の config の変更は数えていない）。
- events の形は追記だけ（任意欄と種類の追加）。旧い events はそのまま読める（G3 の各 unit の記述）。

## 判定 script（`release-check.py`）
- 呼び方: `python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --releases /local/celeris/state/releases`
  （`--repo` 既定 `.`、`--head` 既定 `HEAD`）。
- 読む記録: `<releases>/<sha12>/gate.json`（release.sh の write_gate_json。欄 `sha`・`ok`・`failed_step`）と
  `<releases>/<sha12>/verify.json`（verify.sh が最後に書く。欄 `sha12`・`at`・`ok`・`live_ok`・`checks[]`）。名前が `.` で始まる
  dir（`.build`・`.pnpm-prod-cache` など）は release として数えない。gate の落ちた sha は `.build/<sha12>/gate.json` にあり、
  `--head` の sha12 のものがあれば理由に `failed_step` を添える。
- 条件を満たす release が複数あれば `verify.json` の `at` が新しいものを出す。exit は 0（候補あり）・1（無し、理由は stderr）・
  2（releases dir が無い・`--head` を解決できない）。
- 何も書かない。`--self-test` だけが `$TMPDIR` の下に一時の releases dir と git repo を作り、合格・gate 失敗・verify 失敗・候補の後の
  コード差分の 4 例を subprocess で確かめ、releases dir の中身が実行の前後で変わらないことも見て、最後に消す。

## 証拠（提出時の HEAD で実行）
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --self-test` → exit 0。4 例とも `ok`（合格は exit 0 で
  sha12 を出す。他の 3 例は exit 1 で stdout は空、stderr に `gate: ok=False`・`verify: ok=False（落ちた検査: smoke）`・`src/lib.rs`）。
  0.7 秒。`$TMPDIR` に一時物は残らない。
- `python3 agent-docs/progress/2026-11-15-health-v2-v0/release-check.py --releases /local/celeris/state/releases` → exit 0、
  stdout `bae3149de834`（gate ok・verify ok・live_ok=True、HEAD との差は `agent-docs/progress/` の下だけ）。実行の前後で
  releases dir と `bae3149de834/` の mtime は同じ。
- 同じ script を `--head 5923a76c9aa0`（B0 の基準）で実行 → exit 1。4 つの release それぞれに「`agent-docs/progress/` の外の
  870〜1074 path が変わった」と出る。`--releases /nonexistent` → exit 2。
- 本番 DB は `file:…/celeris.sqlite3?mode=ro` で読んだだけ（schema_version 126、表 12・13・17 の件数、task の状態）。
- `bash scripts/dev/test-affected.sh`・`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` の結果は
  この WorkUnit の報告に書く。

## 未解決事項
- web の Playwright e2e は release gate に無い（結論の「未検証」）。§1 手順 2 で人が流すまで、task の受け入れ条件 0 の
  「web e2e」は満たされない。
- 表 9・20 は current に入っていることしか確かめられない（挙動を直接見る読み取りの command が無い）。

## 提案
- `release.sh` の gate に `pnpm -C web e2e`（ADR 2026-10-20-full-tests-at-release-only の D1）を足すか、ADR の D1 を
  release gate の実際の段に合わせて直す。どちらにするかは人が決める。この WorkUnit は scripts を変えない計画なので扱わない。
