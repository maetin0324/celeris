---
title: K4 公開入力の旧判定情報の監査と採用版 v2 の固定
tasks: [01M5SRJMA9CFNG2C9KCCS5VS4B]
status: done
updated: 2026-10-25
completed: 2026-10-25
---
# K4 公開入力の旧判定情報の監査と採用版 v2 の固定

完了日: 2026-10-25

判断: [ADR 2026-10-25-k4-public-input-v2](../adr/2026-10-25-k4-public-input-v2.md)（採点規則は [ADR 2026-10-23-k4-cells-validity-grading](../adr/2026-10-23-k4-cells-validity-grading.md) のまま）。
各段の詳細: [監査](2026-10-25-k4-public-input-audit/audit.md)・[archive](2026-10-25-k4-public-input-audit/archive.md)・[case](2026-10-25-k4-public-input-audit/case.md)・[無 LLM 通し](2026-10-25-k4-public-input-audit/nollm.md)。

## 要約
- 旧 K4 tgz（v1）は A0・B0 の worker に旧判定を見せていた。`review28/review-43706.txt` と `runs/*/fs/*/logs-audit.json` が正解（32 セル中 30 有効、FS 別 6/8/8/8）と一致する（verdict: leak）。
- 派生物 85 件だけを除いた v2 archive `main-v3-n2-43706-public2.tgz` を作り、case・grade-case・private・generator・試験を v2 に揃えた。scheduler 原票と原票の生データは残した。review28/ も全削除していない。
- **採用版: `k4-public-input-v2`。** 機械向けの採用記録は
  `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-inputs-v2/adoption.json`。P0-2 はこれを読む。
- 評価規則・分母・期待値・許容差は変えていない（32 セル・30 有効、BenchFS W2 の 2 セルは `missing`・`job_failed(exit=-20)`、相対 0.001）。
- 旧 raw・bundle・採点・main-4 revisions には書いていない。

## 公開可能な入力の一覧（v2。A0/B0 で同じ）
trial workspace は `Seeder::make`（problem id だけを引数に取り、arm で分岐しない）が組み立てる。A0 と B0 で可視範囲は同じ。
| 入力 | 中身 | 判定 |
|---|---|---|
| `inputs/main-v3-n2-43706-public2.tgz` と展開した `inputs/`（5676 entry） | `review28/qstat-43706.txt`・`qstat-43722.txt`・`v3-submissions.json`（scheduler 原票）と `runs/43706-20261008T045834Z/` の原票（`planned-cells.json`、`cells/*` の `cell.json`・`exit.json`・`ior.json`・stdout/stderr・`mdtest-ranks.csv`、`fs/*` の FS log、`runner/` の投入・実行 script、job・host・topology の記録） | 公開可。旧判定・集計を含まない |
| `materials/k4-cmp4-validity-analysis/task.md` | 課題文。v2 の展開後構成を説明する | 公開可（正解値を含まない） |
| `materials/k4-cmp4-validity-analysis/case.json` の公開欄 | `Case::public_json` が `private_ref`・`grade_case_ref` を除く | 公開可 |
| `kb/snapshot.md` | 変更なし | 公開可 |
| `materials/k2-bf-xattr-restart-campaign/task.md` | 共有 materials の木から入る別 case の資料 | 判断要（正解を含まない。人の決定待ち） |

grader が読む 147 件（`pinned_raw` 32 件を含む）はすべて v2 に残り、bytes も v1 と同じ。grader-private・ほかの試行・運用者の file はどちらの arm からも見えない。

## 変更理由
- v1 の `review28/review-43706.txt`（log 審査の fs 別 pass 数）と `fs/*/logs-audit.json` 31 件は採点の期待値に一致する（leak）。
- `review28/cells-43706.csv` はセルごとの valid/invalid と数値列を持つ旧判定 CSV（partial。valid 28 は罠 `k4-claim-unifyfs-w3-invalid` と同じ値）。
- 集計・判定 script（`collect.py`・`review-v3.py`・`runner/summarize.py`・`audit-nic.py` 等）は実行すると旧判定を再現できる。
- 公開入力から答えを写せる状態では A/B の差を測れないので、派生物 85 件だけを除いた（理由別件数は ADR D2、全件は manifest の `removed`）。
  内訳: review28/ の集計物・script 18 件、`logs-audit.json` 31 件、`nic-audit.json` 30 件、`runner/` の集計 script と `__pycache__` 6 件。

## 新旧 hash の対応表
| 対象 | v1（旧） | v2（新・採用） |
|---|---|---|
| tgz | `main-v3-n2-43706.tgz` `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`（5761 entry） | `main-v3-n2-43706-public2.tgz` `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`（5676 entry、53234387 bytes） |
| `cases/final/k4-cmp4-validity-analysis/case.json`（file） | `dca408e9e8414c0f6ee81ff7e82118aabf1fbe5b685294cce19f85ef7c94921e` | `9ae6231f00c0ad25ebd9a91b5c846157e5ef219eb5295c55deca22eff9ac9995` |
| case の `snapshot.materials_sha256` | `77aa6847bd45ea487d575a95dd6ad2a1efa36401d94b816bcc09921e3bb29dd7` | `8b4e5c6463baab47df43abf2f9a8cc4ccf1d47714435329aac38d6c66463e89f` |
| `materials/task.md` | `676991c842892bf40c92962931824dadfc0359b1c7b04b96c65e34fbdb7badda` | `c1abc6fcfdc2ff4c27b98ca30c859089bda94ff3161936fec95d3bbbaada2e9c` |
| `kb/snapshot.md` | `4a758f43cb093c45da9297784cf220e8bb26160782b3af30e560e292a59d8122` | 同じ（変更なし） |
| grade-case（`fixtures/grade-cases/final/k4-…/k4-cmp4-validity-analysis.json`） | `34de0c8823c7f03cf227620e73fcbe3c630b18070307d79ca8f9a74db338b64d` | `7751a8577dd09d3f8cd5729179bfbc819aaf87249bb46f16e3ecb751ce48ca54` |
| private（`fixtures/grader-private/final/k4-…/private.json`） | `2aeea5675525b8dd7959c87e5739be9608cccb6a3695c27a503549f821a08638` | `46ea074be15202c4b3d2fca143563ce9c06822607f51e656469da6ad5f6871da` |
| lock の expected_lock | `abdf0b6147ad830551200218206545a113591d585f4e59bb8498359815d2ae7a`（main-4 `judged-scope-v2/pinned/main/lock.json`） | `fa0934224cb3bed29b1e0d2ed1c8f6cb087822b6666bbfc751e959401d887250`（`k4-nollm-v2/lock.json`、measurement_id は `mlsys-main-4-v2-` の後に campaign sha256 の先頭 12 桁） |
| lock の file | `e6c7471090e6a11f2771c91a9f0cd1e679ce3f9698b01747a27ba13f5a4dbf99` | `299985bbb3c47a6b3c7fb1e6ee06d87470d438acd22d48a267b402b2d82c8546` |

v1 の case・grade-case・private の値は archive 段の commit「campaign: K4 archive manifest を old_sha256/new_sha256/removed にし、試験を script の --self-test に入れる」時点の file の sha256（case 段より前）。
v2 の値は提出時の HEAD の file の sha256 で、case.json の `inputs[0]`・`grade_case_ref`・`private_ref` の値と一致する。

## 採用版と adoption.json
- 版: `k4-public-input-v2`（v1 との区別はファイル名の `-public2` と sha256。schema に版欄は足さない。ADR D1）。
- `adoption.json` の形: `{"case_id","version","archive":{"path","sha256",…},"refs":{…},"previous":{"archive_sha256",…}}`。
  `refs` は `case_json`・`materials_task_md`・`materials_tree`・`kb_snapshot_md`・`grade_case`・`private`・`lock`・`manifest`・`exclude_list`・`audit`・`nollm_summary`。
  各 ref に `sha256`、旧版がある ref には `previous_sha256`（lock は `previous`）を持つ。repo の file の path は repo 相対。
- 書いた script は sha256 を実 file から計算し、case.json の `inputs[0]`・`grade_case_ref`・`private_ref` との一致と、旧 tgz の実 sha256 を assert してから書いた。

## 無 LLM 検証の証跡（`k4-nollm-v2/summary.json` の要点）
記録: `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-nollm-v2/summary.json`（sha256 `a666e40e976c0b7ce1d45a0d18b62c8a5cb524afc3067433f7f8b859ccabefcb`）。
再実行: `sh crates/campaign/scripts/nollm-k4-v2.sh <v2 tgz> <出力dir>`。
- 15 case すべて期待 exit と一致（`all_match: true`）。`llm_calls: 0`・`hpc_jobs: 0`・`production_daemon: false`・`measurement_claim: false`。
- archive は case の pin と一致（`matches_case_pin: true`）。
- 正例（exit 0）: `lock`・`stage-positive`・`preflight-positive`・`grade-positive`（32 セル/30 有効、原票からの再計算）・`grade-out`・`check`・`export`。
- 独立負例（exit 1）: 全セル有効、失敗 job の W2 だけ有効、有効セル 1 つの write_MiB_s を 2% 誤る、欠測を分母から落とす（30/30）。
- 事前拒否（exit 1）: archive 欠落・sha256 不一致の stage と preflight。不一致の stage は grade-case の隣に何も置かず、preflight は transport 呼び出し 0。
- preflight は偽 Host の library 呼び出し、executor は模擬。実 launcher・daemon・cap-env・release 接続は pending（`blocker`）。測定開始の証拠ではない。

## 旧資産の不変確認
- 無 LLM 通しの前後で旧資産 64 file（旧 tgz の写しを含む。mlsys-main-4 54・regrade-main-3-k4 7・ほか 3）の sha256 が固定値と一致（`old_assets.unchanged: true`）。
- 本 WU でも `old-assets-after.json` の 64 file を読み直し、不一致 0 件。旧 tgz の本体
  （`01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/measure-src/main/inputs/main-v3-n2-43706.tgz`）は `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1` のまま。
- main-4 の bundle（`judged-scope-v2/pinned/main/lock.json` 等）は v1 のまま。本 task は書いていない。

## 変更（本 task の commit、件名）
- 「docs: K4 公開入力監査の要約を書く」
- 「campaign: K4 公開入力の派生物を除く決定的な archive 生成 script を足す」
- 「campaign: K4 archive manifest を old_sha256/new_sha256/removed にし、試験を script の --self-test に入れる」
- 「campaign: K4 case を公開入力 v2（main-v3-n2-43706-public2.tgz）に揃え、数値誤りの独立負例を足す」
- 「campaign: K4 v2 の無 LLM 通しを script にし、独立負例と lock を記録する」
- 「campaign: K4 v2 無 LLM 通しの負例を script へ移し、試験を基点の版へ戻す」
- 「campaign: K4 v2 無 LLM 通しは前回の audit を消してから試験する」
- 「progress: K4 公開入力 v2 の採用版・新旧 hash・残件を固定する」（本ファイル。close WU はコードを変えていない）

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
（検査後に記入）

## 残件
- **P0-2:** main-4（`mlsys-main-4/revisions/judged-scope-v2`）の K4 pin を v2 に替え、lock を作り直して `adoption.json` の `refs.lock.expected_lock` と照合し、最終 freeze に取り込む。bundle は本 task では変えていない。
- **人の手順:** 本番の measure 入力置き場（main-3 の場合は `/local/celeris/data/measure/<run>/out/snapshot/inputs/` 相当）へ v2 tgz を置くのは人が行う。手順の例:
  `cp <adoption.json の archive.path> <measure の inputs dir>/` のあと `sha256sum <inputs dir>/main-v3-n2-43706-public2.tgz` が `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e` と一致することを確かめる。v1 の tgz は消さない。
- 判断要 26 件（`runner/` の実行コード、`review28/qstat-43722.txt`、`v3-submissions.json`、K2 の `task.md`）を除くかは人の決定待ち。除く場合は exclude.txt と script の規則に足して作り直し、本表と adoption.json を更新する。
- 合成 raw の試験 fixture 名は `main-v3-n2-43706.tgz` のまま（`crates/celeris/tests/campaign_lock_e2e.rs` の `K4_INPUT`）。

## 提案
- 合成 raw の fixture 名を実 archive と紛れない名前（例 `k4-synth-raw.tgz`）にし、`campaign_lock_e2e.rs` と campaign の試験の `K4_INPUT` を同じ変更で替える。
- 共有 materials の木を case ごとに分け、K4 の worker に K2 の task.md を見せない（`runner.rs` の build_seed）。
