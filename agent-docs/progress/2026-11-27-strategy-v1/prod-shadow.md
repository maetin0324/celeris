---
tasks: [01M7XQ9PS0AFKKMRJ7P5ZMKQQE]
wu: prod-count
status: done
updated: 2026-11-28
completed: 2026-11-28
---
# 戦略層 v1 親 prod-count: 本番の R1 shadow の記録を事前の規則で数える

## 結論

- 判定は `inconclusive`（理由 `no project with controller mode shadow`）。`pc/r1-stall` の発火 0、誤検知 0、先行 0、安全違反 0。
- 決定 count-now の答えは b（本番の設定をしなかった。今の本番の状態を数える）。本番には R1 を含む版も controller の設定も無く、R1 は本番で一度も評価されていない。判定 `inconclusive` は、R1 が働かなかったことを示すのではなく、本番に評価の対象が無かったことを示す。
- 数え方は count-rule が記録を見る前に固定した [prod-shadow-rule.md](prod-shadow-rule.md) のまま。`prod-shadow-rule.md`・`test_prod_shadow.py`・`prod_shadow.py` は変えていない（export は本番の schema で落ちなかったので、読み取りも直していない）。
- 本番 DB は `mode=ro` で読んだだけで、本番 DB・config に書いていない。controller の mode・script の領域の mode・Roadmap は変えていない。本番昇格・publish はしていない。
- 記録: [prod-shadow/prod-snapshot.json](prod-shadow/prod-snapshot.json)（snapshot）と [prod-shadow/prod.json](prod-shadow/prod.json)（判定）。

## 読んだ時刻と読み方

- export の時刻（snapshot の `exported_at`）: 2026-11-28T07:43:17.942840Z。shell の時計で前後 07:43:17.918Z と 07:43:18.097Z。
- 流した command（repo の root で）:
  - `python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py export --out agent-docs/progress/2026-11-27-strategy-v1/prod-shadow/prod-snapshot.json` → exit 0（`n_projects 9`・`n_decisions 0`・`n_r1_waiting 0`・`n_strategy_creates 0`）
  - `python3 -I agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py run --snapshot agent-docs/progress/2026-11-27-strategy-v1/prod-shadow/prod-snapshot.json --out agent-docs/progress/2026-11-27-strategy-v1/prod-shadow/prod.json` → exit 0（`verdict=inconclusive`）
- 台本の外の読み取り（2026-11-28T07:44:21Z、同じ DB を URI `file:/local/celeris/data/db/celeris.sqlite3?mode=ro` で開いた。Roadmap の version は snapshot に無いため）: `project_roadmap_versions` 0 行、`project_roadmap_outcome_events` 0 行、`project_controller_state` 0 行、`project_controller_decisions` 0 行、strategy 付きの `task.create`（`state` を問わない）0 行、`script_domain_modes` の `project_controller` は mode `rust`・version 0。
- count-rule の preflight（2026-11-28T07:00:59Z）との差: `exported_at` の行を除くと snapshot も判定も一致する（`diff` exit 0）。42 分の間に本番の数は変わっていない。

## 本番の版と R1 の commit

- `readlink /local/celeris/state/current` → `releases/4446c97339c5`。その `manifest.json` の `sha` は main の先頭の「integrate wu/close-verify (phase close)」で、`promoted.json` の `promoted_at` は 2026-11-27T15:17:35Z（`from` 8c44c114c155）。動いている daemon の process は `/local/celeris/state/releases/4446c97339c5/bin/celeris --config /home/rmaeda/.config/celeris/config.toml --release 4446c97339c5`（`ps` の開始時刻 2026-11-27 15:17:16 UTC）。
- WU r1-core・r1-ts の commit は、どれも本番の版の祖先ではない。親の branch（この WU の HEAD）の祖先ではある。

| commit（件名） | 本番の版の祖先か（`git merge-base --is-ancestor <commit> 4446c97339c5`） | HEAD の祖先か |
|---|---|---|
| strategy-v1 r1-core: R1（K=3）の純関数 parent_checkpoint::stall と snapshot の欄 SnapshotCheckpoint.stall を足し、ADR 2026-11-27-r1-stall-shadow を書く | いいえ（exit 1） | はい（exit 0） |
| strategy-v1 r1-core: snapshot の stall の試験を project_snapshot/stall_tests.rs に移し、inline の試験 module を source-size の上限の中に戻す | いいえ（exit 1） | はい（exit 0） |
| strategy-v1 r1-core: WU の進捗 r1-core.md に実装前 brief・試験・検査・LoC を書く | いいえ（exit 1） | はい（exit 0） |
| strategy-v1 r1-core: 計画 v3 の check を提出時の HEAD で流し直し、r1-core.md の経緯と検査の節を書き直す | いいえ（exit 1） | はい（exit 0） |
| strategy-v1 r1-ts: control-ts の Project Controller に規則 pc/r1-stall と試験 project_controller_r1_stall_ を足す | いいえ（exit 1） | はい（exit 0） |
| strategy-v1 r1-ts: WU の進捗 r1-ts.md に実装前 brief・試験・検査・LoC を書く | いいえ（exit 1） | はい（exit 0） |
| integrate wu/r1-ts (phase shadow) | いいえ（exit 1） | はい（exit 0） |

- 版の中身でも確かめた: `git grep -l 'pc/r1-stall' 4446c97339c5 -- control-ts crates` は 0 file、HEAD では `control-ts/src/domains/project_controller.ts` と同じ dir の試験の 2 file。`git grep -c STALL_K 4446c97339c5 -- crates/task-core/src/parent_checkpoint.rs` は 0 件。`grep -rl r1-stall /local/celeris/state/current/` も 0 件。
- 逆向き: 本番の版は HEAD の祖先（`git merge-base --is-ancestor 4446c97339c5 HEAD` → exit 0）。親の branch は main を落としていない。

## script の領域の mode

snapshot の `script_domains`（`script_domain_modes` の全行）:

| 領域 | mode | 世代 |
|---|---|---|
| `cos_digest` | `compare` | 01M6J6X3HE4Z3MXM11GHJ1B6M4 |
| `execution_gate` | `rust` | なし |
| `lane_rules` | `compare` | 01M6J6X3HE4Z3MXM11GHJ1B6M4 |
| `project_controller` | `rust` | なし（version 0、`updated_at` 1970-01-01T00:00:00Z。一度も変えていない） |
| `route_priority` | `rust` | なし |

`project_controller` が `rust` なので、TS の規則（`pc/r1-stall` を含む）は本番の判定に使われない。手順 4（領域 `compare`）は行われていない。

## shadow の案件・controller の mode・Roadmap の version

- controller の mode が `shadow` の案件: 0 件。9 案件とも `project_controller_state` の行が無く、mode は `null`（既定の off）。
- Roadmap: `project_roadmap_versions` が 0 行で、どの案件にも Roadmap の version が無い（承認済みの版も無い）。手順 5（Roadmap の承認）・手順 6（mode shadow）は行われていない。

## strategy root と直下の子の終端の数

| 案件 id | controller の mode | strategy root | 直下の子の終端が 3 件以上の strategy root |
|---|---|---|---|
| 01M2RBJBPQS3ZKG04YHGD9D5WS | 未設定（off） | 0 | 0 |
| 01M2RCYVZH6RGX8RX0JP572BAT | 未設定（off） | 0 | 0 |
| 01M2WTS3DKNZBSZ2JMVB4CZMBW | 未設定（off） | 0 | 0 |
| 01M35WRV77A2JPYGERQGXF6V7K | 未設定（off） | 0 | 0 |
| 01M4ZXVMKZKKZS1HHANZ9B0NPX | 未設定（off） | 0 | 0 |
| 01M51VQBN3QACQ9M2RCGZDKG9K | 未設定（off） | 0 | 0 |
| 01M54X4V836T3GSMWT7EGQRJB4 | 未設定（off） | 0 | 0 |
| 01M5G00CDDW323Q67C45VPVWBJ | 未設定（off） | 0 | 0 |
| 01M856SYM7GB659365TS8RJ5NY | 未設定（off） | 0 | 0 |

strategy root が 0 件なので、直下の子の終端も全案件で 0 件。strategy 付きの `task.create` が本番に 1 行も無い（`n_strategy_creates` 0）。

## `pc/r1-stall` の数（prod.json の `counts`）

| 数 | 値 |
|---|---|
| 判断（全規則） | 0 |
| R1 の発火（`pc/r1-stall`） | 0 |
| R1 の待ち（`pc/r1-stall/waiting`。発火に数えない） | 0 |
| 誤検知 | 0 |
| 先行 | 0 |
| guard の escalate | 0 |
| Outcome の reached | 0 |
| 安全違反 | 0（R1 の suppressed でない判断 0・controller の起票 0・CoS 入力 0・受信箱の項目 0） |

## verdict

`inconclusive`。規則の判定の順で、1 の `unsafe` は安全違反 0 件なので当たらず、2 の「controller の mode が `shadow` の案件が無い」に当たった（理由 `no project with controller mode shadow`）。3 以降（`falsified`・`useful`・`no_signal`）は評価していない。

## 決定 count-now の答え

- 答え: b（本番の設定をしなかった、または途中でやめた。今の本番の状態を数える）。人 rmaeda が 2026-11-28 07:42Z ごろに operator（external_operator:claude-code-ops-session）の対話 session の AskUserQuestion で「進めずに数える」を選び、operator が代わりに回答を送った（DB の actor は operator）。理由は、preflight で 9 案件とも strategy root 0 件で、手順を行っても `inconclusive` の見込みだったこと。
- 本番の設定が無かったこと（親の進捗の「本番で R1 の shadow の記録を取る手順」の手順 1〜6 と照らした）:
  - 手順 1・2（R1 を含む版の作成と昇格）: 本番の版は 4446c97339c5 のままで、R1 の commit を 1 つも含まない（上の表）。
  - 手順 3（config の `[project_controller]`）: `grep -nE '^\s*\[project_controller' ~/.config/celeris/*.toml` → 0 行（exit 1）。`config.toml` の更新時刻は 2026-11-26T03:10:57Z で、daemon の起動（2026-11-27 15:17:16 UTC）より前。
  - 手順 4（領域 compare）: `project_controller` は `rust`・version 0。
  - 手順 5（Roadmap の承認）: `project_roadmap_versions` 0 行。
  - 手順 6（mode shadow）: `project_controller_state` 0 行。

## 確認済み・推定・未確認

- 確認済み:
  - 判定 `inconclusive`、R1 の発火・誤検知・先行・安全違反はすべて 0（台本の結果。`prod_shadow.py verify` が数え直して一致）。
  - 本番の版 4446c97339c5 は R1 の code を含まず、script の領域 `project_controller` は `rust`、config に `[project_controller]` が無く、controller の状態・判断・Roadmap の version はすべて 0 行。
  - 9 案件とも strategy root 0 件。preflight（07:00:59Z）から 07:43:17Z まで数は変わっていない。
- 推定:
  - 本番で R1 が評価されたことは一度も無い（版に code が無く、領域が `rust` で、controller も無効のため）。R1 の先行・誤検知の性質は、本番の記録からは何も分からない。分かっているのは bench（shadow-bench）と replay の結果だけ。
  - 本番の手順 1〜6 を行っても、CoS が `strategy` を付けて root を起票しない限り、判定は `inconclusive` のまま（preflight と同じ見込み）。
- 未確認:
  - 動いている daemon が読み込んだ config の中身。file の内容と更新時刻からの推定で、daemon の API（`GET /api/v1/daemon`）では確かめていない（token を使っていない）。`systemctl --user` はこの run の sandbox から user bus に繋がらず、unit 名も確かめていない。
  - 本番で controller が R1 を出したときの `effect_json` の `question_key` の形（本番の判断が 0 件のため。count-rule の未確認と同じ）。
  - strategy root が本番で作られる経路（CoS が strategy を付けて `task.create` する）が動くか（子 2 の未確認と同じ）。

## 規則の file を変えていないこと

- `git diff --quiet "$CELERIS_WU_BASE" HEAD -- agent-docs/progress/2026-11-27-strategy-v1/prod-shadow-rule.md agent-docs/progress/2026-11-27-strategy-v1/test_prod_shadow.py agent-docs/progress/2026-11-27-strategy-v1/prod_shadow.py` → exit 0（作業ツリーも `git diff --quiet` で差分 0）。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

（下の「検査の結果」に書く）

## 本番への移行

無し。本番 DB・config・release・controller の mode・script の領域を変えていない。
