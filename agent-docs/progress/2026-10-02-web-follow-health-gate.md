---
title: web release の依存欠落と web-follow の起動確認（ADR-0135）
tasks: [01M3YT4PT3EP8A38111BXH1DCF]
status: done
updated: 2026-10-02
---
# web release の依存欠落と web-follow の起動確認（ADR-0135）

> 旧 `docs/PROGRESS.md`（現 `agent-docs/PROGRESS.md`）に main が足した節を ADR-0128 D6 に従い sync-main-2 migrate-docs（task 01M3Z8CXYG1J6BZCQ87YS3FC67）がここへ移した。本文は元のまま（リンクだけ新配置へ直した）。

## web release の依存欠落と web-follow の起動確認の修正（work unit `record`、HEAD `ab687209cb05`）

### 事故の要約

2026-10-02 17:19 UTC、release `ae780a918695` を live で昇格したところ、`promote.sh` の web-follow が `celeris-web@95ac16442f92` から `celeris-web@ae780a918695` に切り替えた。新 unit の `node server/index.js` が `ERR_MODULE_NOT_FOUND` で起動できず、Web UI（`127.0.0.1:7720` と LAN `192.168.1.103:7721`）が止まった。運用者が `celeris-web@95ac16442f92` に戻して復旧した。`ae780a918695` の `web/app` には `server/`・`dist/` はあるが `node_modules` が無く、tarball は `.pnpm-store` だけで `node_modules` を含んでいなかった。`gate.json` の `web` は `ok=true`・`blocking=false` のままで、web-follow の切替条件（`web.ok=true` かつ `server/index.js` の有無だけ）を満たしてしまっていた。加えて、web unit の再起動のたびに LAN 中継 `celeris-web-lan.service` が `start-limit-hit` で止まり、人が `reset-failed` と socket 再起動を手で行っていた。

方式の決定は [agent-docs/adr/0135-web-follow-health-gate.md](../adr/0135-web-follow-health-gate.md)。実装は本案件の先行 work unit（`release-deps`・`follow-health`・`lan-units`・`verify-web`）で完了済み（下記 D1〜D4 はいずれも `done`）:

- D1（`release.sh` の `bundle_web`）: offline install 後に `node_modules` の実在と `server/` import の解決を確かめ、どちらかが失敗したら `WEB_OK=false`・`WEB_FAILED_STEP=web-bundle` にして理由を `.gate-web-bundle.log` に残す（`scripts/selfdeploy/release.sh:614-645`）。
- D2（`web-follow.sh`）: 切替前に `node_modules` の存在と `sd_web_app_probe` による一時起動確認を行い、失敗したら旧 unit に触れず warning で exit 0。切替後も本番 bind の `/healthz` が `release=<NEW>` で 200 を返すか確かめ、失敗したら新を stop・disable、旧を start・enable し直す（`scripts/selfdeploy/web-follow.sh`）。
- D3（LAN 中継）: `deploy/systemd/celeris-web-lan.{socket,service}` をリポジトリに追加。socket に `TriggerLimitIntervalSec=0`、service に `StartLimitIntervalSec=0` を設定し、`web-follow.sh` の終わりに `reset-failed` と `start celeris-web-lan.socket` を実行する（unit を触った経路のみ、`trap … EXIT` 経由）。
- D4（`verify.sh`）: release の `gate.json` の `web.ok=true` のとき、staging の空き port で `sd_web_app_probe` を実行し、非 blocking で `record 4d web-app-start` に残す（`scripts/selfdeploy/verify.sh:876-886`）。

### 各葉の証拠コマンドと結果

全 12 本の selfdeploy 試験を順に実行した。

```
$ for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || { echo "FAILED: $t"; exit 1; }; done
```

→ exit 0（`real 1m21s`）。各試験の最後の行:

| 試験 | 結果 |
| --- | --- |
| `pid_resolution_test.sh` | `pid_resolution_test.sh: all ok` |
| `prepare_timeout_test.sh` | exit 0（出力なし） |
| `promote_authorization_marker.sh` | `promote_authorization_marker: all ok` |
| `promote_web_follows_release.sh` | `promote_web_follows_release: all ok`（内部で `web_follow_health_gate: all ok` も実行） |
| `release_gui_skip_and_shared_tree.sh` | `release_gui_skip_and_shared_tree: ok` |
| `release_parallel_test_gate.sh` | `release_parallel_test_gate: ok` |
| `release_uses_scratch_lease.sh` | `release_uses_scratch_lease: ok` |
| `release_web_bundle_requires_deps.sh` | `release_web_bundle_requires_deps: ok`（D1: node_modules が無い/import が解決できない offline install を偽 pnpm で再現し `web.ok=false` を確認） |
| `release_web_stage_nonblocking.sh` | `release_web_stage_nonblocking: ok` |
| `verify_durations_and_parallel.sh` | `verify_durations_and_parallel: ok` |
| `verify_web_app_start.sh` | `verify_web_app_start: ok`（D4: `verify.sh` の非 blocking 起動確認） |
| `web_follow_health_gate.sh` | `web_follow_health_gate: all ok`（D2: node_modules 無しでは切り替えない／probe 失敗で旧を残す／切替後確認失敗で旧へ戻す、の3条件を含む） |

個別の実行時間（参考、`/usr/bin/time`）: `pid_resolution_test.sh` 0.06s、`prepare_timeout_test.sh` 0.03s、`promote_authorization_marker.sh` 4.47s、`promote_web_follows_release.sh` 5.13s、`release_gui_skip_and_shared_tree.sh` 13.00s、`release_parallel_test_gate.sh` 6.78s、`release_uses_scratch_lease.sh` 1.16s、`release_web_bundle_requires_deps.sh` 4.70s、`release_web_stage_nonblocking.sh` 9.66s、`verify_durations_and_parallel.sh` 10.39s、`verify_web_app_start.sh` 18.16s、`web_follow_health_gate.sh` 7.82s。合計約 81 秒で、2 分のタイムアウトに収まる（短い `timeout` で打ち切ると偽の失敗になるので注意）。

実装の変更はしていない（このWU の objective は試験の実行と記録のみ）。

### 人が実行する手順

#### 1. 新しい release の作成

```
$ scripts/selfdeploy/release.sh
```

`gate.json` の `web.ok` を確認する。`false` の場合は `.gate-web-bundle.log`（release の build 木、`$BUILD/.gate-web-bundle.log`）に理由が残る（D1 により、`node_modules` が無い・`server/` の import が解決できない release は自動的に `web.ok=false` になり web-follow の対象から外れる）。

#### 2. `deploy/systemd/celeris-web-lan.*` の install と有効化（初回のみ、D3）

```
$ mkdir -p ~/.config/systemd/user
$ cp deploy/systemd/celeris-web-lan.socket deploy/systemd/celeris-web-lan.service ~/.config/systemd/user/
```

`celeris-web-lan.socket` の `ListenStream=192.168.1.103:7721` が実際の LAN address と違う場合はコピー後に書き換える。

```
$ systemctl --user daemon-reload
$ systemctl --user enable --now celeris-web-lan.socket
```

人がすでに手で置いた既存の unit がある場合は、上記の `StartLimitIntervalSec=0`／`TriggerLimitIntervalSec=0` が入っているか diff で確認し、入っていなければ置き換えて `daemon-reload` する。

#### 3. 昇格後の確認

```
$ systemctl --user status celeris-web@<new_sha12> --no-pager
$ journalctl --user -u celeris-web@<new_sha12> -n 50 --no-pager
$ journalctl --user -u celeris-web-lan.service -n 20 --no-pager
```

`web-follow.sh` のログ（`sd_log` の出力。`promote.sh` の標準出力、または `~/.local/celeris/releases/<sha12>/promote.log` 相当）に `web follows the release: celeris-web@<new_sha12>` が出ていれば成功。`warning: new web is not healthy; restoring celeris-web@<old_sha12>` や `warning: web app probe failed; keeping celeris-web@<old_sha12>` が出ていれば、旧 web のまま維持されている（昇格自体は exit 0 のまま失敗しない）。

`/healthz` の release を直接確認する:

```
$ curl -s http://127.0.0.1:7720/healthz
$ curl -s http://192.168.1.103:7721/healthz   # LAN 側。celeris-web-lan.socket 経由
```

応答 JSON の `release` が期待する sha12 と一致するか確認する。

#### 4. 失敗時に旧 web へ戻す手順（web-follow の自動復旧で足りない場合）

通常は D2 により web-follow 自身が失敗を検知して旧 unit を start・enable し直す。それでも旧に戻っていない場合は人が以下を実行する:

```
$ systemctl --user stop celeris-web@<new_sha12>
$ systemctl --user disable celeris-web@<new_sha12>
$ systemctl --user start celeris-web@<old_sha12>
$ systemctl --user enable celeris-web@<old_sha12>
$ systemctl --user reset-failed celeris-web-lan.service celeris-web-lan.socket
$ systemctl --user start celeris-web-lan.socket
$ curl -s http://127.0.0.1:7720/healthz   # release が <old_sha12> に戻ったことを確認
```

### 未解決事項

- D1 の根因（事故時の release 木で offline install が exit 0 のまま `node_modules` を作らなかった経路の確定）は ADR-0135 に記載のとおり未確定。D1 の検査（node_modules と import 解決の必須化）はどの根因でも壊れた release を通さないため、根因の確定は release の修正を妨げない。
- `deploy/systemd/celeris-web-lan.*` の実機への install・`daemon-reload`・既存 unit との置き換えは本番 host の操作であり、このWUでは実行していない（上記「人が実行する手順」参照）。

### main 取り込みと SD_GATE_SKIP_WEB の扱い（work unit `merge-main`、final review 差し戻し対応）

final review で、このタスクの branch が main の `f1904ecd`（release.sh の web 段を既定で skip にする変更、`SD_GATE_SKIP_WEB` の既定を 1 に）を含んでおらず、merge すると新設の `release_web_bundle_requires_deps.sh` だけが「web steps: skipped — SD_GATE_SKIP_WEB=1」で落ちる、という技術的欠陥を指摘された。対応は以下のとおり。

- `git merge main`（`f1904ecd` と `5d6df9f3` を含む main）を実行。`git merge-tree --write-tree HEAD main` で事前に衝突なしを確認済みで、実merge も `docs/PROGRESS.md` と `scripts/selfdeploy/release.sh` を auto-merge し、衝突なしで完了した（merge commit 本文に経緯を記載）。
- `scripts/selfdeploy/tests/release_web_bundle_requires_deps.sh` の `run_release()` に `SD_GATE_SKIP_WEB=0` を明示して追加した（`release_web_stage_nonblocking.sh` の既存の書き方と同じ）。他の selfdeploy 試験で `release.sh` を呼び web 段の実行を前提にしているのはこの 2 本だけで、他は変更不要だった。
- `release.sh` の `SD_GATE_SKIP_WEB` 既定を 1 にしたコメント（f1904ecd 由来）を書き直した。この task（release-deps / follow-health / lan-units / verify-web）で node_modules の有無・`server/` の import 解決の検査（`bundle_web`、web.ok=false 化）と web-follow の起動確認（ADR-0135）が実装済みであることを明記した。ただし **NFS 上での web/app 展開（offline の prod install 含む）に 40〜60 分かかる問題は未対応のまま残っている**ため、既定を 0（web 段を走らせる）に戻すかどうかは人の判断とし、**既定値 `${SD_GATE_SKIP_WEB:-1}` はこの task では変更していない**。
- `scripts/selfdeploy/release.sh` の `${SD_GATE_SKIP_WEB:-1}` という既定値自体のコード（条件式）は変更していない。変わったのはテストの呼び出し側の明示指定とコメントの文面のみ。

#### 証拠コマンドと結果

```
$ git merge-base --is-ancestor f1904ecd HEAD && echo ok
ok
$ git merge-base --is-ancestor 5d6df9f3 HEAD && echo ok
ok
$ for t in scripts/selfdeploy/tests/*.sh; do bash "$t" || { echo "FAILED: $t"; exit 1; }; done
...
pid_resolution_test.sh: all ok
promote_authorization_marker: all ok
web_follow_health_gate: all ok
promote_web_follows_release: all ok
release_gui_skip_and_shared_tree: ok
release_parallel_test_gate: ok
release_uses_scratch_lease: ok
release_web_bundle_requires_deps: ok
release_web_stage_nonblocking: ok
verify_durations_and_parallel: ok
verify_web_app_start: ok
web_follow_health_gate: all ok
```

exit 0（real 約4分28秒。各試験は個別実行では数秒〜20秒程度で、合算の遅さは同一 run 内での繰り返し実行による負荷。詳細な単体実行時間は前節「record」参照）。全 12 本すべて ok。

### 未解決事項（追加）

- NFS 上の web/app 展開が 40〜60 分かかる問題は本 task のスコープ外のまま。`SD_GATE_SKIP_WEB` の既定を 0 に戻すのは、この問題が解決してから人が判断する。

main 33aca5a35969 取り込み・selfdeploy 試験 exit 0（work unit `sync-latest`）。

main aed80844 取り込み、selfdeploy 試験全 pass（work unit `merge-latest`）。main はこの task の work unit `sync-latest` を既に `30e4a37d` で取り込み済みで、HEAD がその祖先だったため `git merge main` は fast-forward（新規 merge commit なし、`docs/PROGRESS.md` に衝突マーカーなし）。`git merge-base --is-ancestor 41366893 HEAD` は exit 0。

main ea2d9d32 取り込み、selfdeploy 試験全 pass（work unit `sync-ea2d`）。
