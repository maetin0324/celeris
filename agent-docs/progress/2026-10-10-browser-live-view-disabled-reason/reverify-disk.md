---
tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]
wu: reverify-disk
status: running
---
# integrate-close 2 回目の全体試験 32 件失敗: /local の disk watch 由来（環境）の記録

## 結論
integrate-close 2 回目の `test-parallel.sh` で落ちた 32 件は、コードの欠陥ではなく
試験中に `/local` の使用率が 95.5〜96.9% になり、`task_dispatch::disk_watch` が critical を出して
新しい coding run を保留したことに伴う環境由来の失敗と判断する（確からしさ: 高。根拠は下記）。
コードは変更していない（crates/・web/ の差分ゼロ）。

## 今の使用率（0 項の確認）
- 確認日時: 2026-10-10（run 中）
- コマンド: `df -h /local`
- 結果: `/dev/mapper/pve-celeris--local 300G 225G 72G 76% /local`
- 判定: 76% < 93%。空きを作る依頼は不要。

## 失敗試験の一覧（32 件、一次情報）
- 元の log: `integration-checks/integrate-close/1791600686497-7.log`
- 集計: `Summary [386.349s] 5016 tests run: 4984 passed, 32 failed, 13 skipped`（`nextest_exit: 100`）
- 一覧は `artifacts/failed-unique.txt`（重複を除いた 32 行）。binary 別の件数:

| binary | 件数 |
|---|---|
| celeris（lib） | 1 |
| celeris::instance_handoff | 1 |
| e2e::account_pool_scenarios | 2 |
| e2e::api_scenarios | 4 |
| e2e::cluster_scenarios | 2 |
| e2e::codex_account_pool_scenarios | 1 |
| e2e::delegation_scenarios | 5 |
| e2e::multi_account_scenarios | 3 |
| e2e::phase7_scenarios | 5 |
| e2e::plan_scenarios | 2 |
| e2e::provider_admin_scenarios | 2 |
| e2e::scenarios | 4 |

- 失敗の型は 2 種:
  - e2e 系は ほぼ全部 `60.1s` か `120.x s` で時間切れ。`slow run never appeared in /daemon` の出力では
    daemon の `in_flight` が空のまま。coding run が保留され、idle に達しなかった。
  - `celeris tests::a_publickey_cluster_with_a_ready_task_does_not_panic_the_first_tick_phase_81`（0.2 秒で終了）は
    `crates/celeris/src/lib/tests.rs:1203` で `left: 0 / right: 1`（publickey cluster の connector 到達 0 回）。
    **これは disk 由来だと log から確認できていない**（後述）。

## 根拠（log の disk 行）
同じ log の daemon 出力（test 用 daemon が `/local` を見ている）:
- 初回: `2026-10-10T02:56:01.608192Z ERROR task_dispatch::disk_watch: disk usage above critical threshold path=/local pct=Some(96.85...)`
- 直後: `WARN task_dispatch::disk_watch: disk watch: critical; new coding runs are held (ADR 2026-10-07-build-tmp-hygiene A4)`
- 以後 log 全体で `disk usage above critical threshold path=/local` が 44 回。pct は 95.525〜96.850 の範囲。
- 時刻: daemon の起動は 02:55:58、最初の critical は 02:56:01。e2e の時間切れ（60〜121 秒）と重なる。
- 試験側の daemon 出力の `scratch` は `fs_free_bytes: 57154670592`（試験用 TMPDIR の fs）で、
  試験用 TMPDIR の空きは足りていた。見ているのは `/local` の disk watch だけ。

前回（integrate-close 1 回目, `1791599756338-4.log`）は失敗が 1 件（post-login flake）で、
disk 行は無い。2 回目で 32 件に増えたのは、disk watch の critical が試験中に出たことと整合する。

## 未確認（この記録の限界）
- `publickey` の lib 試験（`tests.rs:1203`）が disk 由来かは、log から確認できていない。
  試験は 0.2 秒で落ち、log に disk 行は近くに無い。connector 到達 0 回の原因は別に調べる必要がある。
  「環境由来」と書くのは e2e 群に限る。
- 再実行で同じ 32 件が再現するかは未確認（この WU の範囲は記録と提案まで）。
  統合の取り直し（integrate-close の再実行）で確かめる。

## 提案
- daemon e2e と celeris の lib 試験は、host の `/local` 使用率（disk_watch の既定 path）に依存する。
  試験用 daemon では disk_watch の path か閾値を一時 dir に差し替えるべき（既定の critical_pct は 95%）。
  例: 試験設定で `[[maintenance.disk_watch]]` の path を試験用 TMPDIR に向ける、または critical_pct を試験専用の値にする。
  これで、試験の合否が host の空き状況で変わらなくなる。
- 別案（小さい）: 試験 daemon の起動時に disk_watch を無効にする試験専用の設定を入れる。
  どちらにするかは人の判断（本番の保護を試験で外す範囲の選択）。
- publickey 試験（`tests.rs:1203`）は別件として切り分ける。connector 到達 0 回の経路を見る。
- 本番の `/local` は 76%。96% まで伸びた原因（大きい target・scratch 等）は運用側で確認が要る。
  本番操作はこの WU の範囲外。
