---
title: G7 1 回目の cos_usage_null 3 件の修正の結果と main 反映後の再測定手順
tasks: [01M6RGSDEKJECE8QNFS1JC5P0B]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# G7 1 回目（n1-g7-20261106-1）の修正の結果と再測定手順

完了日: 2026-11-07。基準は `git merge-base HEAD main`。

## 結論
- 起動前に終わった CoS run を費用不明に数えないこと、枠の埋まりだけで CoS run を失敗にしないこと、driver が CoS run の終了と usage の書き込みを待つことの 3 点を、ADR と実装と決定的な試験で入れた。
- **実 LLM の G7 再判定は未実施**。この task は実 LLM を起動していない。G7・cost・wall の合否は未判定のまま。
- release・promote・systemd・本番 DB・本番 config の操作もしていない。
- operator がやること: main 反映後に承認を確かめ、G7 を再測定する（下の「本番への移行」）。

## 元 raw の結果表
raw は [raw/](raw/)、詳細と根拠は [contract.md](contract.md)。数値は contract.md から転記した。

| 項目 | 値 |
|---|---|
| 終了コード | 3 |
| judge | verdict unknown、g7 fail、cos_usage_null 3（CoS run 5 本中 3 本の usage が null） |
| cost_cap / wall_cap | unknown / pass（234 秒） |
| USD | usage のある 12 本の和 0.9802（C06 0.5794、C08 0.4008）。合計は null |
| C06（試行 id は raw/arm-map の file 名） | 135 秒、採点 pass。null の CoS run は at=134（試行の終わりの 1 秒前） |
| C08（同上） | 99 秒、採点 fail。null の CoS run は at=72 と at=91。`cos_run_failures` に「no declared route candidate for harness claude_code is eligible」1 件。案件は `budget:usd_unknown:2026-11-06` で blocked |
| credential の検査 | 3 通りの grep で秘密の値 0 件、除去箇所なし（manifest.json の `secret_scan`）。judge の `leak_hits` は空 |

## 確認した原因と未確認事項
確認した原因（コードと raw の突き合わせ。根拠は contract.md）:
1. 起動前に route を選べず失敗した chat run は、`started_at` と `finished_at` を持ち usage が無い。案件の USD 集計（`project_run_usage_between`）が、これを費用不明に数えた。C08 の案件はこれで止まった。
2. driver は最後の操作の後、CoS run の終了と usage の書き込みを待たずに daemon へ SIGTERM を送っていた。C06 の at=134 の run の usage はこの間に失われたと判断した。

未確認:
- C08 の null 2 本（at=72・at=91）のうち、どちらが起動前の失敗かは分からない。一時 DB が残っていない。もう 1 本は試行の終わりで切れたと推定するが、未確認。
- 起動前の失敗で、候補がどの理由で除かれたか（実 account の枠、暗黙 account の枠、cooldown、5 時間枠の予約）は不明。
- operator が渡した `--llm-config` の `[accounts]` の中身は raw に無い。
- C06 の at=134 の run が SIGTERM 時に running か interrupted だったかは不明。

## 3 点の ADR・実装・試験の対応
正本 ADR は [2026-11-06-cos-run-launch-accounting-and-drain](../../../adr/2026-11-06-cos-run-launch-accounting-and-drain.md)。ADR を追加した commit は、試験を追加した最初の commit の祖先（`git merge-base --is-ancestor` が 0）で、実装より先に入っている。付記は [2026-11-07-pilot-drain-shared-deadline](../../../adr/2026-11-07-pilot-drain-shared-deadline.md)。

| 決定 | 実装の要点 | 決定的な試験と修正前の不合格 |
|---|---|---|
| (a) D1: 起動前に終わった chat run は費用不明にしない。起動後の usage 欠測は fail closed のまま | 起動直前の永続通知（`launched_at`）、`prelaunch_reason`、案件予算の `not_launched_runs` | `cos_launch_wire`（12 件）、`cos_launch_cross`（8 件）。修正前は 12 件中 12 件・8 件中 8 件が失敗。起動後 usage null は `UsdUnknown` のままという試験を含む。詳細は [backend/launch-and-wait.md](backend/launch-and-wait.md) |
| (b) D2: 枠の埋まりだけでは CoS run を失敗にしない | 枠だけの除外は Waiting で保持し、期限で `slot_wait_expired` | `route_select::slot_wait`、`cos_slot_wait`（5 件）。修正前は枠待ちの試験が落ちた（同ファイルの表） |
| (c) D3: driver が CoS run の終了と usage の書き込みを待つ | drain と承認の wall の総量から決める共有の期限、期限超過は unknown | `test_pilot_drain.py` ほか 84 件、実 daemon の `test-pilot-drain.sh`。修正前の driver は遅い CoS で `cos_usage_null:1`（[pilot-drain.md](pilot-drain.md)） |

旧い経路（usage 無しで閉じる経路、文字列による枠判定）は同じ変更で消している。互換分岐は残していない（各 unit の記録を参照）。

## 受け入れ条件ごとの証拠（提出時の HEAD で実行）
最後の同期の後の HEAD で実行した。この task の差分はこのファイルだけ。

- `bash scripts/dev/test-affected.sh` → exit 0。基準以降のコード差分が無く「試験対象の変更なし（docs のみ等）」。文書検査の自己試験は OK（check-doc-links、progress_index_commit_hashes）。コード側の通し結果は backend の各記録に残る（例: nextest 7809 passed）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-long-horizon-pilot.sh` → exit 0。実 daemon・LLM 0。`test-pilot-drain` の 4 場面:
  - (1) 遅い CoS: drain が 86.2 秒待ち、全 CoS run に usage がある。g7 は pending。
  - (2) `max_runs=1`: 役割は worker 4・reviewer 4・cos 6 で、CoS の枠待ちは [0, 4, 5, 4, 0, 0] 秒。
  - (3) 期限超過: `cos_drain_timeout:2`、verdict unknown。
  - (4) 2 試行が承認の総量を共有: wall [73, 31]、2 本目が上限に達した。
- `python3 -m unittest discover -s scripts/dev/long-horizon-pilot -p "test_*.py"` → 84 件 OK。
- 文書検査（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`・`check-doc-layout.sh`）の結果は下の「文書検査」に記す。
- 実 LLM の G7 再判定: 未実施。

## commit（件名。hash は書かない）
基準 `git merge-base HEAD main` から HEAD までの主なもの。

- ADR と raw: `phase 5: G7 1 回目の raw を取り込み、CoS run の起動境界・枠待ち・終了待機を ADR で決める`、`phase 5: G7 1 回目の contract に検査の結果を書く`
- 起動境界の永続化と予算: `phase 1: 起動境界の回帰試験（修正前に失敗）と store 側 ADR`、`phase 2: chat run の起動境界と attempt 欠測を永続化する`、`phase 2: 案件の費用対象行と起動前除外数を同じ条件で取得する`、`phase 3: 案件予算に起動前除外数と fail closed を反映する`、`phase 4: chat/controller の公開契約（not_launched_runs）と HTTP 回帰試験・API 文書`
- 起動通知と配線: `phase 1: 起動前通知の契約試験(修正前に失敗)と EventSink::before_spawn`、`phase 2: spawn 直前に起動前通知を送る(全 adapter)`、`phase 1: 起動境界・attempt 会計・回収の dispatcher 試験(修正前に失敗)`、`phase 2: launch・sink・control・回収を起動境界の永続契約へ接続する`、`phase 4: triage claim で作られた run の launched_at 試験を足す`
- 枠待ち: `phase 5: 候補別の構造化除外理由で枠のみの待ちを判定する`、`phase 5: CoS の枠のみの待ちを Waiting で保持し期限で閉じる`、`phase 1: CosReservation を route_select に配線し、実経路の試験を足す`
- 横断試験と記録: `phase 1: 起動通知・枠待ち・回収・予算の横断試験を足す`、`phase 1: 各回帰の修正前の不合格と修正後の合格を launch-and-wait.md に集約する`、`phase 1: backend 統合結果の検証と親への契約を記録する`
- 修復: `repair: chat run 一覧が壊れた usage_json で全体失敗しないようにする`、`repair: cos_digest の trace 試験を壊れた usage_json に頼らず再現する`、`repair: task-core の clippy 指摘 4 件を直す`、`repair: usage-query の統合検査を絞った検査で通し進捗に証拠を残す`
- pilot: `phase 1: pilot が CoS の終了と usage 永続化を待ち、期限超過を unknown にする`
- この記録: `phase 1: G7 1 回目の修正の結果と再測定手順を記録する`

## 本番への移行
この task は本番を操作していない。次は人（operator）が実行する手順。

1. main 反映の確認: 上の commit が main に入ったことを確かめる（`git merge-base --is-ancestor <commit> main` が 0）。release・promote は通常の手順で行う。この task では行っていない。
2. 承認の確認: G7 の承認 `n1-g7-20261106-1` の `expires_at`・`max_usd`・`max_wall_secs`・scope を読む。有効期限内で範囲が足りれば再利用できる。切れているか足りなければ、新しい承認を発行する。承認の wall は試行ごとでなく**総量**として使われる（付記 2026-11-07-pilot-drain-shared-deadline）。見積りの根拠は 1 回目の USD 0.9802（12 本の分）と wall 234 秒。drain の待ちが加わる。
3. 再測定: 1 回目と同じ手順で G7 を実行する（`--harness config`、account claude_max_personal、model claude-sonnet-5-5、harness claude_code、`--cos-route` を付ける。供給元の `max_runs = 1` も同じ）。承認の file を driver に渡し、共有の期限が働くようにする。
4. 判定: 次を見て G7・cost・wall を決める。
   - `cos_usage_null` が 0、`cos_not_launched` と `cos_drain_timeout` が 0 か、数があっても理由が facts に残っている。
   - cost_cap が unknown でなく数値になる。wall_cap は総量の期限内か。
   - C08 のような案件が `budget:usd_unknown` で止まっていない。
   - drain の期限超過があれば verdict は unknown のままになる。これは仕様どおり。その場合は期限か承認の wall を見直す。
5. 結果を新しい raw として取り込む（credential の grep 0 件を先に確かめる）。

## 未解決
- 実 LLM の G7 再判定（上の手順 3〜5）。
- contract.md の未確認事項 4 件（一時 DB が無く確かめられない）。再測定で同じ症状が出なければ、(1)(2) の修正が効いたと判断してよいが、原因の取り違えが無いことは再測定の facts で確かめる。
- CoS 同士が同じ account の枠を取り合う場合、launch tick の間は互いの予約が見えない（slot-wait の既知の制約）。

## 提案
- 起動前の reason に `launch_unavailable`（設定・credential・workspace・attachment）を足すかを判断する。今は `route_unavailable` に寄せている。
- ADR 2026-11-06-cos-run-launch-accounting-and-drain D3-1 の式を付記で改めたので、本体の文面を更新するか判断する。
- CoS 同士の枠の取り合いを効かせる別 task（launch tick の間も `route_reservations` を `route_state` に渡す）。

## 文書検査
下の結果は、このファイルを含む HEAD で実行した（コミット前の作業ツリー）。

- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（397 file）
- `sh scripts/dev/progress-index.sh --check` → exit 0（試行 id が hex として誤検出されたため、書き方を改めてから再実行）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → exit 0
