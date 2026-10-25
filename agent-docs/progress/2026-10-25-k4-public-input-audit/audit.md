---
title: K4 公開入力の旧判定情報の監査
tasks: [01M5SRQMMXBXFENYGWJW8CBMZ3]
status: done
updated: 2026-10-25
---
# K4 公開入力の旧判定情報の監査

完了日: 2026-10-25

## 冒頭要約
- **漏洩あり（verdict: leak）。** K4 の実 tgz `main-v3-n2-43706.tgz` には旧判定・集計が入っており、A0・B0 のどちらの worker にも見える。
  `review28/review-43706.txt` の fs 別 pass 数（benchfs 6・chfs 8・gekkofs 8・unifyfs 8、計 30/32）は private の正解と一致する。
  `review28/cells-43706.csv` はセルごとの valid/invalid と数値列を持つ解答表に相当する（valid 28 は罠 `k4-claim-unifyfs-w3-invalid` と同じ値）。
- **除外の提案: 派生 85 件。** 対象は review28/ の集計物 18 件、`fs/*/logs-audit.json` 31 件、`cells/*/nic-audit.json` 30 件、runner の集計 script と `__pycache__` 6 件。
  grader が読む 147 件と重なる file は 0 件。runner の実行コード等 26 件は判断要として人に回す。
- **tgz は変えていない。** sha256 は監査の前後で
  `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1` のまま。写し・参照の値もすべてこれと一致する。
  除外を実施すると sha256 が変わるため、archive・case の後続段で case.json・grade-case・pinned_raw を作り直す必要がある。

機械向けの全件表: `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-input-audit/audit.json`
（5761 entry。各 entry は path・sha256・class・rationale・required_by_grader・visible_to を持つ。生成 script は同じ dir の `inventory.py`・`classify.py`）。

## tgz の sha256 と写し・参照の照合
本体: `/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/measure-src/main/inputs/main-v3-n2-43706.tgz`
（86097232 byte、5761 member = file 5759・dir 2、symlink なし）

| 写し・参照 | 値 | 一致 |
|---|---|---|
| `regrade-main-3-k4/case/main-v3-n2-43706.tgz` | 同じ sha256 | ○ |
| `mlsys-main-4/nollm/fixture/inputs-src/main-v3-n2-43706.tgz` | 同じ sha256 | ○ |
| `/local/celeris/data/measure/main-3/out/snapshot/inputs/main-v3-n2-43706.tgz` | 同じ sha256 | ○ |
| `crates/campaign/cases/final/k4-cmp4-validity-analysis/case.json` の `inputs[].sha256` | 同じ sha256 | ○ |
| `crates/campaign/fixtures/grade-cases/final/k4-cmp4-validity-analysis/k4-cmp4-validity-analysis.json` の `recompute.input_archive.sha256` | 同じ sha256 | ○ |
| `crates/campaign/fixtures/grader-private/final/k4-cmp4-validity-analysis/private.json` | sha256 を持たない（`expected.input` に名前だけ） | — |

補足: find で見つかったほかの写しのうち、`cmp4/raw/`、`01M58VFR…/measure-src/`、`mlsys-main-3/root/src/inputs/` の 3 件は同じ sha256 だった。
`real-daemon-rehearsal/` 配下の 25 byte の file は rehearsal 用の stub で、本物の写しではない（audit.json の `additional_copies` を参照）。

## A0/B0 の可視範囲
**A0 と B0 で、入力として見える範囲は同じ。**
- `inputs/main-v3-n2-43706.tgz` と、それを展開した `inputs/` の全 5761 entry
- `materials/k4-cmp4-validity-analysis/task.md`
- `materials/k4-cmp4-validity-analysis/case.json`（公開欄。`Case::public_json` が `private_ref`・`grade_case_ref` を除く）
- `materials/k2-bf-xattr-restart-campaign/task.md`（全 case が共有する materials の木から入る。判断要）
- `kb/snapshot.md`

A0 と B0 で同じになる理由: trial workspace は `Seeder::make(&pid, &ws)`（`crates/campaign/src/inputs.rs:243-250`、呼び出しは `runner.rs:1011`）が組み立てる。この関数は problem id だけを引数に取り、arm で分岐しない。
- A0 は `harness_exec` が bwrap で包み、trial workspace しか見えない。
- B0 は `celeris_exec` が `workspace_mode=shared` で起動する（`celeris_exec.rs:471-477`）。kb/ は knowledge 道具からも引けるが、中身は同じ `kb/snapshot.md` である。
- grader-private、ほかの試行、運用者の file はどちらの arm からも見えない。

task.md・kb/snapshot.md・case.json の公開欄は 30・32・cells_valid の値も既知欠陥も含まない（no_leak）。
ただし task.md は「同梱の集計・報告文言は証拠にしない」と書いているだけで、その file を可視範囲から外してはいない。

## 旧判定情報の有無と漏洩の根拠
private の正解（`private.json`、sha256 `2aeea5675525b8dd7959c87e5739be9608cccb6a3695c27a503549f821a08638`）: 32 セル中 30 が有効。fs 別は benchfs 6・chfs 8・gekkofs 8・unifyfs 8。
job 43706 は F / Exit_status -20。既知欠陥は `k4-chfs-fsync-zero`・`k4-job-failed`・`k4-w2-denominator`。

| path | 判定 | 根拠 |
|---|---|---|
| `review28/review-43706.txt` | **leak** | fs×workload ごとの log 審査 pass 数。fs 別の和が 6/8/8/8（計 30）で、private と完全に一致する。BenchFS W2 を fail、UnifyFS W3 を pass としている |
| `fs/*/rep*/*/logs-audit.json`（31 件） | **leak** | review-v3.py が job の後にセルごとに書いた pass/fail。集めると上と同じ 30/32 になる |
| `review28/cells-43706.csv` | partial | 32 行。BenchFS W2 の 2 行を missing で invalid とする扱いは正解と同じ。valid は 28 で、UnifyFS W3 を mdtest warning で落としている（罠 `k4-claim-unifyfs-w3-invalid` と同じ）。有効セルの数値列は grader が再計算する値と許容 0.1% の範囲で一致する |
| `runner/summarize.py`・`review28/collect.py`・`review-v3.py`・`shutdown_review.py` 等 | partial | 実行すると旧判定（28/32）や log 審査を再現できる集計・判定 script |
| `cells/*/nic-audit.json`（30 件）・`review28/nic-audit.txt` | 派生 | job の後に書かれた NIC 選択の判定（pass） |
| `review28/log-review.txt`・`cells-43722.csv`・`manifest-*.json`・`v3-released.json`・`accounting.json` | no_leak（派生） | 別 job 43722 のものか、目録・会計。43706 の判定件数は入っていない |

## 除外リスト案（派生 85 件）
- `review28/`（18 件）: `__pycache__/`（dir と `shutdown_review.cpython-39.pyc`）、`accounting.json`、`archive-43722.sha256`、`cells-43706.csv`、`cells-43722.csv`、`collect.py`、`collision-proof.py`、`log-review.txt`、`log-survey.txt`、`manifest-43706.json`、`manifest-43722.json`、`nic-audit.txt`、`review-43706.txt`、`review-v3.py`、`shutdown_review.py`、`test-shutdown-review.py`、`v3-released.json`
- `runs/43706-20261008T045834Z/fs/*/rep*/*/logs-audit.json`（31 件）
- `runs/43706-20261008T045834Z/cells/*/rep*/*/nic-audit.json`（30 件）
- `runs/43706-20261008T045834Z/runner/` の 6 件: `summarize.py`、`audit-nic.py`、`__pycache__/submit-v3.cpython-39.pyc`、`__pycache__/summarize.cpython-39.pyc`、`lib/__pycache__/gkfs-hosts.cpython-39.pyc`、`lib/__pycache__/plan.cpython-39.pyc`

全件の path は audit.json の `exclude_proposal` にある。この中に `required_by_grader=true` の file は 0 件（`exclude_check.excluded_required_by_grader = 0`）。

**判断要（26 件。除外案には入れていない）:**
- runner の実行コード（`cmp4-job.sh`・`fs/*.sh`・`lib/*`・`submit-v3.py`・`test-*.py`・README など）: 原票の来歴を示すもので、判定値は持たない。
- `review28/qstat-43722.txt`: 別 job の qstat 生出力。grader は読まず、30/32 も含まない。
- `review28/v3-submissions.json`: 投入受領の一次記録。
- `materials/k2-bf-xattr-restart-campaign/task.md`: 別 case の資料。

いずれも正解は含まない。残すか除くかは範囲の方針として人が決める。

## grader（`crates/campaign/src/cells_validity.rs`）が要る原票
grader は tgz 全体の sha256 を照合してから `inputs/` に展開する。そのうえで `cells_spec` に従って次の 147 件を読む。すべて原票に分類した。
- `review28/qstat-43706.txt`（`cells_spec.jobs["43706"]`。`audit_cell` が job の終了状態を判定する）
- `runs/43706-20261008T045834Z/planned-cells.json`（期待セル集合 32）
- `runs/43706-20261008T045834Z/cells/<fs>/rep<n>/<w>/` の `cell.json`・`exit.json`・`stdout.log`、IOR セルでは `ior.json`・`stderr.log`、w3 では `mdtest-ranks.csv`
- `cells_spec.table = results/cells.csv` は worker の出力で、tgz の file ではない。

`read_paths` のうち 7 件（benchfs rep1 w2 の cell.json・exit.json、rep2 w2 の 5 file）は tgz に無い。grader はこれらを欠測（missing）として扱い、それが既知欠陥 `k4-w2-denominator` の根拠になる。したがって除外とは関係しない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `sha256sum <本体 tgz>` → 監査前後とも上の全桁と一致
- `python3 -I` で audit.json を読んで確認: 5761 entry、すべての entry に sha256 と class がある。`review28/cells-43706.csv`・`review28/qstat-43706.txt` を含む。分類は原票 5650・派生 85・判断要 26
- 文書検査（`check-doc-layout.sh`・`check-doc-links.sh`・`progress-index.sh --check`）→ 3 本とも exit 0
- `crates/` の変更なし

## 変更
- `docs: K4 公開入力監査の要約を書く` — 本ファイルを追加した

## 未解決
- 除外は実施していない。tgz・case.json・grade-case・pinned_raw は元のまま。
- 判断要 26 件を残すか除くかは、人の決定を待っている。
- `real-daemon-rehearsal` の 25 byte の stub は監査の対象外。

## 提案（後続段 archive/case）
1. archive 段: 派生 85 件を除いた新しい tgz を作る（原票の byte は変えない）。新しい sha256 を記録し、元の tgz は残す。
2. case 段: case.json の `inputs[].sha256`、grade-case の `recompute.input_archive.sha256` と `pinned_raw` を新しい tgz に合わせて作り直す。そのうえで grader の再計算が 30/32 と既知欠陥 3 件を変えずに出すことを回帰試験で確かめる。
3. 判断要 26 件については、「runner のコードは来歴として残す」「別 job の qstat と K2 の task.md は除く」のどちらかを人が決める。K2 の task.md は materials の木を case ごとに分ければ外せる（`runner.rs` の build_seed）。
4. task.md の「同梱の集計は証拠にしない」という文は、除外を実施した後も残す。
