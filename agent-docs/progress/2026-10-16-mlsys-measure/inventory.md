---
title: "MLSys 本測定: 既存 Benchfs／Pluvio 資産の棚卸しと代表 case 候補"
tasks: [01M51QRH7TD3FHER7HVF9W3TAH]
unit: inventory
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# inventory（WU inventory、読むだけ）

親 root 01M51QNB8EC4EG0D5GA4BDVWHQ の「2. 既存資産を使ったキャンペーン定義と独立 grader」のための棚卸し。
コードは変えていない。本番 host・本番 SQLite には書いていない（`celerisctl show/ls/projects` と
`knowledge search/get` の読み取り、ローカルにある過去 task の作業場所の読み取り、`$TMPDIR` への bundle の clone だけ）。
外部 network・LLM 呼び出し・クラスタ job は使っていない。

## 状態の区分

| 区分 | 内容 |
|---|---|
| 実装済み | なし（この unit はコードを変えない） |
| mock 検証済み | なし |
| 実経路検証済み | 資産の**存在と hash**だけ確かめた（下表の「確認」列）。case の受入 command は**一度も実行していない** |
| 未実施 | 受入 command の実行（pre-fix で落ち fix で通ることの確認を含む）、Sirius での再実験、case の固定（WU `cases`）、grader（WU `grader`）、分離（WU `isolation`） |

## 1. 資産の表（場所と状態）

`W=/local/celeris/data/workspaces`（daemon が同期した過去 task の作業場所。リモート worktree の写しで、`.git` は Sirius の
`/work/NBB/rmaeda/workspace/rust/benchfs/.git/worktrees/<task>` を指すため、ここでは git は使えない）。

| # | 資産 | 場所 | 状態・確認（この run） | 出典 |
|---|---|---|---|---|
| A1 | BenchFS の **pre-fix commit** `4115dbd8912d42f4509654e8b06d329760e07e77`（tree `5bda9947…`、313 file、`#[test]` 163 件） | `$W/01M3QKJZ8Y8TMSQ6HXQNFDFD9Z/artifacts/p0-fix.bundle`（sha256 `f8b67ddb…`、9.8 MB） | `git clone` で取り出せた。commit・tree object は bundle にある。親 `21232ed9…` 以前の object は無い（履歴は 1 段だけ） | task 01M3QKJZ8Y |
| A2 | BenchFS の **fix commit** `086c4618e2a4d94ea56da78ceeed0f8d6d568d67`「phase 0: fsync と chunk0 xattr 復元を修復」（13 file、+526/−104、`#[test]` 166 件） | 同 bundle の HEAD、差分は `p0-fix.patch` | 取り出せた。追加試験 3 件: `src/rpc/data_ops.rs:1545 fsync_reply_uses_registered_reply_stream`、`src/storage/chunk_store.rs:3116 test_posix_explicit_fsync_reads_back`、`:3129 test_posix_size_xattr_survives_store_reopen`（pre-fix tree には無い） | 同上 |
| A3 | 後続 fix: `535f72d2…`（by-ID chunk の xattr 復元、親 086c4618、試験追加なし）、`54f2ad2b…`（UCX lookup で chunk0 xattr から再起動後サイズ復元、試験 `lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks`）、`e4c8e77d…`（IOR client ID・backend build） | `p0-xattr-path.bundle`／`p0-final.bundle`／`p0-run6.bundle` と各 `.patch` | 535f72d は 086c4618 の上に適用できる（prerequisite が A2）。54f2ad2・e4c8e77 の bundle は prerequisite `60d19a1653f0…` を要求し、**60d19a1 はローカルに無い**（Sirius の git store のみ） | 同上、`fix-report.md` |
| A4 | 既知の欠陥 5 件（fsync 経路）と修復対応表 | KB `projects/benchfs/known-issues-inventory.md`、`$W/01M3QKJZ8Y…/artifacts/fix-report.md` | 読んだ。欠陥: FsyncChunk 返信 ID 不一致／POSIX fsync_chunk がメモリ常駐（SIGKILL 後 File not found）／Locusta feature 無し binary／IOR fsync 許可リストで黙って fsync=0／BENCHFS_EFFECTIVE 無出力と失敗でも集約 exit 0 | task 01M3QKJZ8Y、KB |
| A5 | P0 短い実クラスタ実験の手順と原票: 4 セル（UCX/Locusta × io_uring/POSIX）、job **42588–42591**、全セル PBS Exit_status=0 | 手順 `jobs/p0/sirius-p0-{build,submit}.sh`・`jobs/params/p0_restart.conf`（A2 の tree 内）、原票の写し `$W/01M3QKJZ8Y…/artifacts/p0-evidence/20260930T011640Z/`（ほか 2 時刻、`final-raw.tar.gz`）、独立照合 `verify-p0.py`／`verify-p0-final.py` | 存在を確認（`p0-evidence` 下 641 entry）。照合台本は**実行していない**。クラスタ側原票 root は `/work/NBB/rmaeda/scratch-bf-p0-retry-01M3QN5547/results/p0_restart/20260930T011640Z/` | `fix-report.md`、review.json crit 2 pass |
| A6 | Locusta 低並列対策 A/B の実測原票 job **43223**（2+2 ppn1、3 反復）と残差実験 **43322** | `$W/01M45ACZQWTW9BP8WVY8KAW9TC/artifacts/locusta-latency/raw/main-43223.tgz`（sha256 `bdb155d4c169…`、3070 entry: `ior_results/ior_result_0.json`・stats CSV・benchfsd logs・toml）、`raw/c-43322.tgz`、集計 `analysis-main-43223/{rates.csv(132 行),medians.json,summary.md}`、`analysis-c-43322/summary-c.json`、台本 `summarize-main.py`・`audit-main.py` | tgz の sha256 は KB 記載 `bdb155d4` と一致。集計は**再実行していない**。注意: 4 KiB write の B 効果は `medians.json` が中央値の比 **1.309**、`summary.md`・KB が反復ごとの比の平均 **1.333** — 統計量の定義が 2 つある（case で固定が要る） | task 01M45ACZQ、KB `locusta-user-mr-dispatch-idle-results.md` |
| A7 | Locusta/UCX 原因分解の原票 job **43209**（44 セル中 41 有効）・**43218**（27/27） | `$W/01M44TDAZFWSE8E997V87B3BT6/artifacts/transport-latency/raw/{v2-43209.tgz,v3-43218.tgz,*.qstat.txt}`、`analyze.py`・`summarize.py`、`source-snapshot.tgz` | 存在を確認。集計は再実行していない | task 01M44TDAZ、KB `locusta-low-parallelism-latency-cause.md` |
| A8 | 4 FS 比較（BenchFS/CHFS/GekkoFS/UnifyFS）原票 v3: job **43706/43708/43722**（N2/N4/N8）ほか、原票から再計算する `verify.py`・`aggregate.py`、`cells-primary.csv`・`summary.csv`・`failures.csv`・node-h 台帳 | `$W/01M49KWT59XM37ANGCTJ8A4DX5/artifacts/cmp4/`（raw 1.2 GB、`final-v2/`、`final-v3/`） | 存在を確認。`verify.py` は実行していない。BenchFS W2 は eager RPC timeout で欠測と報告に明記 | task 01M49KWT5 |
| A9 | Sirius 実験の運用事実（会計群 gen/NBBG、1 vnode/host 配置、walltime 目安、IOR JSON の欄名、CHFS fsync 許可リストの罠、`chfsctl stop` の pkill 誤爆、client lib は `target/release` から解決） | KB `projects/benchfs/{sirius-experiment-run-playbook,sirius-experiment-pitfalls,ior-json-result-format,chfs-experiment-notes,experiment-results}.md` | 読んだ | KB |
| A10 | 制御試験用 fixture（mock）: 5 problem・pilot manifest・誤り注入 review cases・mock 台本 | `crates/campaign/fixtures/`、`agent-docs/progress/2026-10-15-mlsys-min/campaign-fixtures.md` | 既存。**縮めた mock 題材で実 Benchfs の写しではない**（`fixtures/repo/README.md` 自身がそう書く）。本測定には使わない | 前 root 01M4Z1QJX5 |
| A11 | BenchFS の版を固定できない過去 task: 01M44TDAZ／01M45ACZQ／01M49KWT5 の task branch は head_sha 空のまま `IntegrationDiscarded`（遠隔 repo に branch が無い） | `celerisctl show <task>` の event | 確認した。これらの task の BenchFS commit は**記録が無い**。内容は `source-snapshot.tgz`・`source.sha256`・`benchfs.diff` で hash 固定されている | events |
| A12 | Locusta commit `31e8742726e7b4b1ac378f4ed8814e3f3b3fd2c9`（= gitlink `68f8b210…` + `patches/locusta-external-mr.patch`、branch `benchfs-external-mr`） | Sirius の git store のみ（GitHub 未 push） | ローカルには patch と commit message だけ。README の「base 0b5b83db」は誤りと task 01M45ACZQ の checkpoint に記録 | task 01M45ACZQ |

## 2. 代表 case 候補（5 件）

外側の 1 problem = 実装→検証→実験→分析・報告→レビューの研究キャンペーン 1 本（工程間の受け渡し・job 待機は Celeris 内部）。
資源は A/B/C で同一。token 上限は親の承認済み予算（tokens 合計 1,000,000、試行 12）を試行数で割った目安で、**最終値は人の承認**（§3 R1）。
node-h は過去 job の実績からの見積もり。

| case | split | 種別（主工程） | 初期 commit／入力 | 受入テスト command（grader が隠して持つ） | 実験条件 | 期待する生データの形 | 既知欠陥（レビュー工程で検出させるもの） | 資源上限 |
|---|---|---|---|---|---|---|---|---|
| **K1 bf-fsync-campaign** | dev | 実装＋短い実験＋報告＋レビュー | BenchFS `4115dbd8`（A1、bundle から）＋ submodule `lib/pluvio@9ecd51ee`・`lib/locusta@68f8b210`・`ior_integration/ior@7054224d`（**submodule の中身は bundle に無い**、§3 R2） | `cargo test --release --lib -- fsync_reply_uses_registered_reply_stream test_posix_explicit_fsync_reads_back test_posix_size_xattr_survives_store_reopen`（A2 の試験 3 件を grader 側から差し込む。既存 163 件も全通過）。UCX が要るので Sirius／pegasus の login node で build | P0 の 4 セル: transport {UCX, Locusta} × storage {io_uring, POSIX}、3 host（server・writer・別 host reader）、IOR `-e`（fsync）・`-R`（checkRead）、write→別ノード read→server SIGKILL＋再起動→read（`jobs/params/p0_restart.conf`） | 各セル `ior_results/{write,cross_node_read,restart_read}.{json,log}`、`p0_{before_kill,after_restart}_xattr.log`、`p0_effective_config.txt`、PBS job ID と `qstat -x` の Exit_status。合格値: IOR JSON fsync=1・checkRead=1・rc=0、chunk0 xattr 4194304→4194304（参照値 42588–42591） | 欠陥 5 件のうち dev 用 3 件: FsyncChunk 返信 ID 不一致、POSIX fsync_chunk 非永続、IOR fsync 許可リストで黙って fsync=0（報告側に「fsync 有効」と書かれていても実効 JSON が 0 なら誤り） | 3 host × 0.5 h 程度（1 セル約 1 分、build 込みで 1 job）＝ **2 physical-node-h 以下**、試行あたり tokens **80k**、壁時計 2 h |
| **K2 bf-xattr-restart-campaign** | final | 実装＋短い実験＋報告＋レビュー | BenchFS `086c4618`（A2、K1 の解答を含むので final は**別の入力**）＋ 同じ submodule | `cargo test --release --lib -- lookup_recovers_xattr_after_restart_for_direct_and_by_id_chunks`（54f2ad2 の試験。**60d19a1 上の試験なので 086c4618 へ移植できるかは未確認**、§3 R3） | P0 の再起動 read（by-ID chunk と direct chunk）、UCX × {io_uring, POSIX} の 2 セル（または 4 セル） | K1 と同形。合格値: restart_read の checkRead=1・rc=0、再起動後サイズが書いた値と一致 | final 用（K1 と別）: by-ID chunk の xattr 復元経路の欠落（535f72d）、UCX lookup が再起動後サイズを復元しない（54f2ad2）、Locusta 後続 client が同じ `ior_client_0` ID で接続停止（e4c8e77）、BENCHFS_EFFECTIVE 無出力・失敗でも集約 exit 0 | K1 と同じ（2 node-h・80k tokens・2 h） |
| **K3 locusta-ab-analysis** | dev | 分析・報告＋レビュー（クラスタ不要） | 原票 `main-43223.tgz`（sha256 `bdb155d4…`、A6）を入力 snapshot として固定。集計済み `analysis-main-43223/` と KB ページは**渡さない**（解答の漏れ） | grader が `ior_result_0.json` から独立に再計算した L_A/L_base・L_B/L_base・L_AB/U（4 KiB・1 MiB・4 MiB × write/read）と、成果物の表を照合（統計量は case で「反復ごとの比の平均」か「中央値の比」に固定、A6 注意） | 再実験なし（43223: 2+2 ppn1、xfer {4k,1m,4m}、3 反復、条件 L_base/L_A/L_B/L_AB/U） | 入力: IOR JSON（`summary` は `operation/bwMeanMIB`、`tests[0].Results` は `access/bwMiB`）。出力: 条件×xfer×op の比の表＋分母（反復数 3、有効セル数） | 誤り注入（実在の誤りから）: 1 MiB read の B 効果 0.94 を「改善」と書く、`base 0b5b83db` を Locusta の基点と書く（正は 68f8b210＋patch）、MIB/MiB 欄の取り違え | node-h 0、tokens **60k**、壁時計 1 h |
| **K4 cmp4-validity-analysis** | final | 分析・報告＋レビュー（クラスタ不要） | 原票 cmp4 v3 `main-v3-n2-43706.tgz`（必要なら n4-43708 を追加。n8 は 547 MB で重い）を入力 snapshot として固定（sha256 は WU `cases` で記録）。`final-v*/` の集計・報告は渡さない | `cells-primary.csv` 相当（fs×workload×nhosts の帯域・IOPS・有効/無効と理由）を grader が `verify.py` と同じ規則で原票から再計算し照合 | 再実験なし（v3: 4 FS × workload w1a/w1b/…、N=2、ppn 4/64） | 入力: IOR/mdtest JSON・stdout・実効設定・CPU。出力: 有効セル数／全セル数の分母つき表、無効理由 | final 用（K3 と別）: 実効 fsync=0（CHFS の許可リスト）を有効に数える、`qstat -x` Unknown Job Id や F/Exit≠0 の job を有効に数える、BenchFS W2 の欠測を分母から落とす | node-h 0、tokens **60k**、壁時計 1 h |
| **K0 fixture-control** | control（dev のみ） | 全工程の制御試験 | `crates/campaign/fixtures/`（A10） | 既存の `crates/campaign/tests/pilot_fixtures.rs` と mock 台本 | mock（実行しない） | 既存 fixture の形 | 既存 `review/cases.json` の 6 件 | 資源 0。**最終合格・本測定の数値に使わない** |

dev／final の分け方: K1・K3 が dev、K2・K4 が final。final は初期 commit・入力原票・注入する欠陥がすべて dev と別
（K2 の初期 commit は K1 の解答を含むが、K2 の欠陥と隠し試験は K1 と重ならない）。

## 3. 未発見・要人の確認

### 未発見（捏造しない）
- **Pluvio 単体の実資産**: Pluvio 調査案件（01M2RW2WG… 系、survey task 01M2SMJ3QRN1・01M2SMJ3QRYP）の作業場所・原票はローカルに無い。KB にも Pluvio survey の原データは無い（`celerisctl knowledge search pluvio` は user/goals と benchfs ページのみ）。`fixtures/materials/pluvio-survey-raw.json` は縮小 mock。→ Pluvio を主題にした実 case は作れない。Pluvio は K1/K2 の submodule `lib/pluvio@9ecd51ee`（BenchFS の runtime）としてのみ使う。
- **`60d19a1653f0b15559ec6462dd1c9fa1332bb35c`** の object（54f2ad2・e4c8e77 の親）: ローカルに無い。
- **01M44TDAZ／01M45ACZQ／01M49KWT5 の BenchFS commit sha**: 記録が無い（A11）。内容 hash（`source.sha256`・`source-snapshot.tgz`）だけがある。
- **mdtest-hard heap corruption**（rank ≥16）の再現手順・原票: KB に症状だけで、固定 commit・再現 job は見つからない。case にしない。
- **A0（IO500）の有効原票**: 8 job で完走 0（KB）。case にしない。

### 要人の確認
- **R1 資源の承認**: K1/K2 は Sirius の実 job（1 試行 ≤2 node-h 見込み）を使う。親の承認済み予算（measure-budget = pilot-estimate）は tokens と使用率の上限で、**node-h の上限が書かれていない**。A/B/C×反復で K1/K2 を回すなら node-h 上限（例: 3 arm × 1 反復 × 2 case × 2 node-h = 12 node-h）の承認が要る。なお Sirius の計算ノードが落ちる問題（task 01M51PTJV4、Blocked）が未解決で、短い job でも欠測が出得る。
- **R2 submodule の固定**: K1/K2 の build には `lib/pluvio@9ecd51ee`・`lib/locusta@68f8b210`・`ior_integration/ior@7054224d` が要るが、過去の worker 起動で `not our ref 68f8b210… / 7054224d…` と GitHub から取れない記録がある。Sirius の git store（`/work/NBB/rmaeda/workspace/rust/benchfs`）から snapshot を作るか、ローカル写し（`$W/<task>/lib/…`、版不明）を使うかは人の判断。
- **R3 K2 の隠し試験**: `lookup_recovers_xattr_after_restart…` は 60d19a1 上で書かれた試験。086c4618 に移植して pre-fix で落ち fix で通るかは Sirius 側で確かめる必要がある（未実施）。移植できなければ K2 の受入は 535f72d の by-ID 経路を実験（restart_read）だけで判定する形に縮める。
- **R4 build 環境**: このホストに `libucp`（UCX）が無い（`ldconfig -p` で未検出）。BenchFS は `pluvio_ucx`（async-ucx）が必須依存なので、受入 command はクラスタの login node で走らせる前提。A arm（既存ハーネス）にも同じ実行場所を与える必要がある。
- **R5 K3 の統計量**: 「反復ごとの比の平均」（summary.md・KB の 1.333）と「中央値の比」（medians.json の 1.309）のどちらを正にするか。
- **R6 K4 の入力量**: n8 原票（547 MB）を入れるか。入れないなら N2（＋N4）に限る。

## 4. 次の unit への引き継ぎ（提案）
- WU `cases`: K1〜K4 を case 定義にし、入力（bundle・tgz）の sha256 と取り出し手順を固定する。隠し試験・参照値・欠陥一覧は grader 側にだけ置き、試行の作業場所・KB snapshot に入れない（KB の `locusta-user-mr-dispatch-idle-results.md`・`experiment-results.md`・`known-issues-inventory.md` は K1〜K4 の解答を含むので、試行の KB snapshot から除く）。
- WU `grader`: K3/K4 は既存台本（`summarize-main.py`・`cmp4/final-v2/verify.py`）を参照実装として読むだけにし、grader は自前で原票から再計算する（台本の出力を正解として写さない）。
- 本測定前に R1〜R4 の人の判断が要る。
