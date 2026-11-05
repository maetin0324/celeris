---
title: P4-F 実 daemon 全体の非本番 soak と RSS の原因の切り分け（準備と短時間の実施）
tasks: [01M6PAMC40X023V30F4T9QH2GW]
status: done
updated: 2026-11-05
completed: 2026-11-05
---
# P4-F 実 daemon 全体の非本番 soak と RSS の原因の切り分け

完了日: 2026-11-05（非本番の短時間の範囲。24/72 時間の実稼働・本番の読み取り・本番の off の再起動は未実施）。第 4 フェーズ親 01M6PAK8E8RH3ZMQC0FAD8ZNAA の子（人の依頼 seq 2343）。基準は `git merge-base HEAD main`。ADR [2026-11-05-phase4-p4f-daemon-soak](../../adr/2026-11-05-phase4-p4f-daemon-soak.md)。

## 結論
- **24/72 時間の実稼働（段 3）、本番の RSS の読み取りと off の再起動（段 4）は未実施である。** 人の承認が無い。実施の範囲・停止条件・承認の file の形は下の「人が決めること」に書いた。driver は 1 時間を超える run を承認の file なしでは起こさず、worker の run の中では承認があっても断る（試験で確かめた）。
- 本物の daemon binary（API・dispatcher・CoS chat・CoS の一次対応・cos_digest・cron・worker・reviewer）を 4 条件で 26 分ずつ同時に走らせた。本番の 26 分の増加（VmRSS +406,848 kB、そのうち RssAnon の増加は 338,880 kB 以上）は**再現しなかった**。試験の RssAnon の増加は 1,500 秒で 16,760〜19,440 kB で、本番の RssAnon の増加の下限の 4.9〜5.7% だった。
- 試験の RssAnon の増加は malloc の増加（使用中 +11.6〜13.6 MB、空き +7.8〜12.2 MB）で全部説明が付いた（残差は負）。V8 の used heap は最大 2.5 MB、SQLite の page cache は最大 5.5〜5.8 MB、非 main の malloc の arena の推定は 27→31（off）・33→37（shadow）。shadow と off の RssAnon の増加の差は +2.7 MB／25 分。
- **本番の増加の原因は未確認である。** この負荷と DB の大きさ（task 163 件・events 3,405 行。本番は events 783,007 行）では、V8・compare・CoS chat・cron・API・SQLite の page cache・malloc の arena のどれも本番の桁の増加を出さなかった。残る候補は DB の大きさ（page cache の上限と大きな行の読み）、本物の LLM の run の出力の大きさ、ssh・実 worker の process、本番の日数の長さで、どれも未検証である（「再現の条件と残差」）。
- 計測の道具を足した: daemon の 60 秒ごとの `process_memory` の log 行（RSS・RssAnon・threads・SQLite の page cache・malloc・V8 heap）、driver `scripts/dev/daemon-soak/daemon_soak.py`・`scripts/dev/daemon-soak.sh`（4 条件・自動停止・raw・集計・承認の門・before/after）。P4-C・P4-D の後の性能の回帰は `--celeris <旧い binary> --label before` で測れる（7 分の before/after で確かめた）。

## main と既存実装の照合
- 読んだもの: `agent-docs/progress/2026-11-05-phase3/p1i.md`（4 条件・26 分・72 時間の仮想時刻）、`p0f.md`、`p2j.md`、`docs/ops/project-controller.md`、兄弟の `p4a.md`〜`p4e.md`。
- 再利用したもの: P1-I の条件の名前と負荷の考え方、本番の 4 標本（`long_run_soak_summary.py` と同じ値）と 2 点の式、P1-I の `ScriptRuntimeHandle::heap_stats`、long-horizon pilot の driver の一時 daemon の起こし方。仮想時刻の 72 時間（`project_controller_soak_72h_*`）は作り直さず、test-affected の中で再び通した。
- 足りなかったもの: 本物の daemon の process の値（P1-I は dispatcher だけ）、daemon の中でしか読めない SQLite・malloc・V8 の値の本番での読み口、run を持ったまま止めた daemon の restart、自動停止と承認の門、before/after の口。

## 変更（commit 件名）
- `p4f: daemon の process_memory の log 行と、実 daemon 全体の soak driver・fake harness・試験を足す`
- `p4f: 26 分・4 条件の実 daemon soak と before/after の raw、集計の窓と RssAnon の傾き、ADR と進捗を足す`
- `p4f: 提出時の HEAD の検査の結果を進捗に書く`

| 場所 | 内容 |
|---|---|
| `crates/celeris/src/daemon/process_memory.rs` | `process_memory` の log 行（60 秒ごと、別 task）。試験 `process_memory_*` 3 件 |
| `crates/celeris/src/daemon/run.rs` | 起動時に spawn、停止時に abort |
| `crates/script-runtime/src/generation.rs` | `GenerationRegistry::heap_stats`（生きた generation ごとの V8 heap。lock を離してから待つ） |
| `crates/celeris/Cargo.toml` | `libc`（`mallinfo2`） |
| `scripts/dev/daemon-soak/daemon_soak.py` | `run`（一時 home の daemon・負荷・標本・自動停止・SIGKILL の restart）、`summary`、`plan` |
| `scripts/dev/daemon-soak/soak_harness.py` | fake harness（LLM 0、`--sleep` 秒だけ run を持つ） |
| `scripts/dev/daemon-soak/test_daemon_soak.py` | 門・自動停止・読み取り・集計の試験 15 件 |
| `scripts/dev/daemon-soak.sh` | 4 条件を別 process で同時に走らせ、集計する |

## 結果 1: 実 daemon・26 分・4 条件（raw `p4f/soak26/`）
`bash scripts/dev/daemon-soak.sh --out agent-docs/progress/2026-11-05-phase4/p4f/soak26 --secs 1620 --label main --kill-at 1540 --sample-secs 60 --task-interval 10 --chat-interval 300 --wake-interval 600` → exit 0（2026-11-05 21:24〜21:51Z）。restart は 1,540 秒（本番の比較の区間 0〜1,529 秒の後）。窓の標本は最初の process の 27 点（0〜1,500 秒）。

| 指標（窓 0〜1,500 秒） | off | v8_load | shadow | enabled | 本番（2026-11-04） |
|---|---:|---:|---:|---:|---:|
| RssAnon 開始→終了（kB） | 14,208→30,968 | 14,592→32,072 | 20,928→40,368 | 20,544→38,652 | 終了 520,700 |
| RssAnon の増分（kB） | 16,760 | 17,480 | 19,440 | 18,108 | 338,880 以上 |
| RssAnon 傾き 0→1,529・600→1,529（MB/時） | 39.3・21.5 | 41.0・20.0 | 45.6・23.8 | 42.4・24.3 | - |
| VmRSS 傾き 0→1,529（MB/時） | -49.8 | -46.8 | -74.6 | -79.1 | 935.5 |
| malloc 使用中の増分・空きの増分（kB） | 11,774・7,849 | 11,577・8,970 | 13,648・12,151 | 12,935・11,992 | 未計測 |
| 残差（RssAnon − malloc − V8、kB） | -2,864 | -3,068 | -6,104 | -6,564 | - |
| SQLite page cache 最大（kB） | 5,638 | 5,373 | 5,496 | 5,381 | 未計測 |
| V8 used heap 最大（kB） | 0 | 0 | 2,539 | 2,538 | 未計測 |
| threads・arena の推定 | 27→29・27→31 | 27→29・27→31 | 33→35・33→37 | 33→35・33→37 | threads 29→38 |
| dispatch の遅延 p50・p95（ms） | 167・519 | 169・477 | 173・488 | 181・561 | - |
| task・CoS chat の run・受信箱の項目（終了） | 163・859・316 | 163・859・316 | 163・859・316 | 163・859・316 | - |
| wake・判断・比較の行（終了） | 0・0・0 | 0・0・0 | 332・46・173 | 332・46・173 | 比較 2 |
| 未消費の wake・event の取り込みの遅れ（終了） | 0・1 | 0・1 | 6・1 | 6・1 | - |
| log の slow tick（1 秒以上） | 32 | 34 | 52 | 46 | - |

- VmRSS の傾きが負なのは、1,100 秒前後に 4 条件とも RssFile が約 50 MB 減ったため（debug の binary の file の page。同じ時間に test-affected の build が host の memory を使った）。VmRSS は本番（release の binary）と比べる指標にならず、RssAnon で比べた。本番の RssAnon の開始は記録が無く、開始の VmRSS（181,820 kB）を上限として増加の下限 338,880 kB（= 520,700 − 181,820）を使った。
- RssAnon の増分は malloc の使用中と空きの増分の和を下回り（残差は負）、この負荷の増加は malloc の heap（生きた data と断片）で説明が付いた。
- CoS chat の run 859 本は CoS の一次対応（task の done ごとの受信箱の項目）で、4 条件で同じ数だった。fake harness なので LLM は 0。
- slow tick は test-affected と同じ時間に走らせたので host の CPU の取り合いを含む。比較には使わない。
- 合否の目安（ADR D7）: 4 条件とも pass（RssAnon の 600 秒以後の傾きの off との差 -1.5〜+2.8 MB/時、V8 の used heap は開始の 2 倍未満、generation 1、restart の後の running の run とリース 0、自動停止なし）。

## 結果 2: run を持ったまま止めた restart（raw `p4f/soak26/*/ops.jsonl` の `sigkill`・`restart`）
| 指標 | off | v8_load | shadow | enabled |
|---|---:|---:|---:|---:|
| SIGKILL の前の running の run・task のリース | 1・1 | 1・1 | 1・1 | 1・1 |
| 起動から API が答えるまで（秒） | 0.21 | 0.40 | 0.20 | 0.20 |
| SIGKILL から running の run とリースが 0 になるまで（秒） | 4.23 | 4.42 | 4.22 | 4.22 |
| restart の後の running の run・リース | 0・0 | 0・0 | 0・0 | 0・0 |

- 回収は標本の 1 秒刻みで測った（driver の待ちの刻み）。blocked の task は 0 件（fake harness は done を返す）で、P1-I の「blocked 約 650 件で最初の tick 約 3 秒」とは条件が違う。

## 結果 3: before/after（raw `p4f/before-after/`）
`daemon-soak.sh --label before --celeris <基準の binary> --conds off,shadow --secs 420 --kill-at 400` と同じ引数の `--label after` を同時に走らせた → 両方 exit 0。基準の binary は `git merge-base HEAD main` を一時 worktree で build した（`process_memory` の行が無いので、その欄は None）。

| label/cond | RssAnon の増分（kB） | dispatch p50・p95（ms） | 回収（秒） | 比較の行 |
|---|---:|---|---:|---:|
| before/off | 12,724 | 69・188 | 4.42 | 0 |
| before/shadow | 6,408 | 82・149 | 4.43 | 41 |
| after/off | 9,904 | 54・148 | 4.43 | 0 |
| after/shadow | 9,152 | 89・174 | 4.43 | 41 |

- 7 分では RssAnon の増分の差（-2.8〜+2.7 MB）は誤差の幅で、log 行の追加の影響は見えない。目的は before/after の口が版の違う binary で動くことの確認である。

## 再現の条件と残差
| 条件 | 本番（2026-11-04） | この計測 | 揃ったか |
|---|---|---|---|
| process | daemon 全体（release） | daemon 全体（debug） | 中身は揃えた。build は違う |
| compare の領域 | `lane_rules`・`cos_digest` | `shadow` は同じ | 揃えた |
| DB の大きさ | events 783,007 行 | 開始 0 行、終了 3,405 行 | 揃わない（次の候補） |
| worker | 実 LLM（claude・codex 等）・ssh | fake harness | 揃わない |
| 長さ | 1,529 秒 | 1,500 秒 | 揃えた |
| threads | 29→38 | 27〜33→29〜35 | 本番の方が多く増えた |

- 残差: 本番の RssAnon の増加の下限 338,880 kB のうち、この計測で再現したのは 16,760〜19,440 kB。残り（約 320 MB 以上）の出どころは未確認。
- 次に切り分ける順（提案）: (1) 本番の DB の写しを seed にした同じ 4 条件（events 78 万行。page cache と大きな行の読み）、(2) 段 4 の本番の `process_memory` の行の読み取り（この task の binary が配備された後。読むだけ）、(3) 本番の off の再起動との比較。

## 安全の閾値（24/72 時間の実施で使う。ADR D6・D7）
- 自動停止: RSS 4,096 MB、disk の空き 5 GB、DB 4 GB、daemon の終了、API の 3 回連続の失敗、承認の `max_secs`。
- 合否の目安: RssAnon の 600 秒以後の傾きが off との差で +10 MB/時以内、V8 の used heap が開始の 2 倍未満、generation 2 以下、restart の後の running の run とリース 0。
- 26 分の実測の RSS は最大 146,872 kB で閾値から遠い。DB（WAL を含む）は 26 分で 74〜108 MB になった。比例すると 24 時間で 1 daemon 4〜6 GB で、既定の DB の上限 4 GB で止まり得る。24 時間以上では `--max-db-mb` を上げる（`plan` の `recommended_args`）。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `python3 -I -m unittest discover -s scripts/dev/daemon-soak -p 'test_*.py'` → OK（15 tests）。
- 上の 26 分の command → exit 0、4 条件とも stop `wall`。7 分の before/after → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（7,757 tests run: 7,757 passed、22 skipped、script bundle SKIP 0 件、test-ts-verify 20 passed。`process_memory_*` 3 件と `project_controller_soak_72h_*` 3 件を含む）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `test -f agent-docs/progress/2026-11-05-phase4/p4f.md` → exit 0。`sh scripts/dev/progress-index.sh --check` → exit 0。`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-doc-links.sh`・`check-adr-numbers.sh` → exit 0。
- test-affected・clippy・fmt・unittest は 2 つ目の commit の後の HEAD で再実行し、同じ結果だった（test-affected 7,757 passed、22 skipped）。

## 失敗ケース（この run の中で起きて直したもの）
1. 最初の smoke で chat の POST が 422（`mode`・`resume_queue` の欄が要る）。失敗のたびに予定を進めなかったため 120 秒で 1,423 回送った。予定を先に進めてから送るように直し、欄を足した。
2. `process_memory` の行が読めなかった。text の log に ANSI の色の符号が入る。`NO_COLOR=1` で起こし、読む側でも符号を消す。
3. SQLite の `MEMORY_USED` は 0 だった（bundled の SQLite は memstatus が無効）。page cache の bytes を使い、Rust の試験もそれに合わせた（ADR D2）。
4. restart の時に run が 0 本で、回収を測れなかった。fake harness に `--sleep`（既定 3 秒）を足し、running の run が出るまで待ってから SIGKILL する。
5. 集計の最初の版は 1,529 秒に近い標本として restart の後の標本（別 process）を選び、傾きが負になった。窓を最初の process に限った。VmRSS の file の page の揺れは残るので、RssAnon の傾きを足し、合否は RssAnon で見る。
6. dispatch の遅延の照会が task の `created` を seq 1 として探していた（実際は seq 0）。

## 未実施（人の承認が要る）
1. 段 3: 非本番の 24 時間（延長で 72 時間）の 4 条件。
2. 段 4: 本番 daemon の `process_memory` の行と `/proc/<pid>/status` の読み取り（読むだけ。この task の binary が本番に入った後）、本番の `[script]` を外した off の再起動との 26 分の比較（本番の再起動が要る）。
3. 実 LLM を使う計測（この task の範囲外）。

## 人が決めること
- 段 3 を行うか。行うなら host、24 時間か 72 時間か、承認の file の中身。費用は fake harness なので LLM 0。資源の見積りは 4 daemon で disk 約 16〜24 GB／24 時間（26 分の DB 74〜108 MB の比例の推定。WAL の checkpoint で小さくなり得る。未確認）、CPU は tick 100 ms と task 10 秒に 1 件。停止条件は上の閾値。
  - 承認の file の形（`python3 -I scripts/dev/daemon-soak/daemon_soak.py plan --hours 24` が雛形を出す）: `{"approved_by": "human:<名前>", "scope": "non-production", "max_secs": 86400, "conditions": ["off","v8_load","shadow","enabled"], "expires_at": "<ISO 8601>"}`
  - 実行（人が worker の外で）: `bash scripts/dev/daemon-soak.sh --out <dir> --secs 86400 --approval <file> --kill-at 43200 --max-db-mb 12000`。確認は `<dir>/summary.md` の 4 行が pass で、停止の理由が `wall`。
- 段 4 を行うか（本番の読み取りは、この task の binary を promote した後に `journalctl --user -u celeris@<sha> | grep process_memory` で読める）。

## 未解決
1. 本番の RssAnon の増加（下限 338,880 kB／1,529 秒）の原因は未確認。この計測で再現したのは 5〜6%。
2. 本番の RssAnon の開始の値は記録が無い。段 4 では `process_memory` の行で開始から読める。
3. SQLite の `MEMORY_USED` は memstatus が無効で読めない。page cache 以外の SQLite の memory（statement・schema）は malloc の使用中に含まれ、分けて読めない。
4. arena の推定は `/proc/<pid>/maps` の形からの推定で、glibc の内部の数と照合していない。
5. 26 分の計測は test-affected の build と同じ時間に走らせた。slow tick と dispatch の遅延の p95 は host の負荷を含む。

## 提案
1. 本番の DB の写し（読み取り専用の写しを一時 home に置く）を seed にした 4 条件の 26 分を、次の切り分けにする（driver に `--seed-db` を足す。別 task）。
2. `GET /api/v1/daemon` に `process_memory` の値を載せるかは、段 4 の log の読み取りの後に決める（API の schema と web の型を変える）。

## 次の gate
- final review（受け入れ条件 0〜2）。
- 人の判断: 段 3（24/72 時間）と段 4（本番の読み取り・off の再起動）を行うか。
