# ADR: K4 の公開入力を旧判定・集計を除いた v2 archive（main-v3-n2-43706-public2.tgz）に替える

- 日付: 2026-10-25
- 状態: 実装済み（agent-docs/progress/2026-10-25-k4-public-input-audit/case.md 参照）
- 関連: ADR 2026-10-23-k4-cells-validity-grading（採点規則。本 ADR は変えない）、ADR 2026-10-21-grader-own-input-extraction-and-max-turns

## 背景
K4（`k4-cmp4-validity-analysis`）は原票 tgz を試行 workspace の `inputs/` 直下に展開し、A0/B0 の worker に `runs/` と `review28/` を見せていた。
監査（agent-docs/progress/2026-10-25-k4-public-input-audit/audit.md）で、旧 tgz に旧監査の判定・集計が同梱されていることを実 file で確かめた。
`review28/review-43706.txt` と `runs/<run>/fs/*/logs-audit.json` は採点の期待値（30/32、FS 別 6/8/8/8）と一致する。
`review28/cells-43706.csv` はセルごとの valid/invalid・理由・数値列を持つ（解答表に相当。採点規則とは一部ずれる）。
採点器は原票から独立に再計算するので採点そのものは正しい。しかし、公開入力から答えを写せる状態では A/B の差を測れない。

## 決定
### D1 公開入力の版: `k4-public-input-v2`
- 新 archive: `main-v3-n2-43706-public2.tgz`
  sha256 `0671a85528b4a29a8f6fb5176986c178afb2174cb22e56b003c33ed5f2da3a1e`（5676 entry、53234387 bytes）。
  作ったのは archive WU の `crates/campaign/scripts/build-k4-public-archive.py`。manifest は
  `/local/celeris/data/workspaces/01M5SRJMA9CFNG2C9KCCS5VS4B/artifacts/k4-inputs-v2/manifest.json`（`changed: true`）。
- 旧 archive: `main-v3-n2-43706.tgz`
  sha256 `128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1`（5761 entry）。版名は `k4-public-input-v1`。
  旧 raw・mlsys-main-4 の bundle・採点・revisions は書き換えない。旧版で採った試行の記録は v1 のまま読む。
- 新旧はファイル名（`-public2`）と sha256 で区別する。`Case` の schema に版欄は足さない。版名はこの ADR と case の `materials/task.md`（「公開入力 v2」）に書く。

### D2 除外したもの（85 entry）と理由
| 除外理由（manifest の `reason`） | 件数 |
|---|---|
| `review-v3.py` が job 後に書いた log 審査の判定（`fs/*/logs-audit.json`） | 31 |
| `audit-nic.py` が job 後に書いた NIC 選択の判定（`cells/*/nic-audit.json`） | 30 |
| 集計・判定 script（`review28/collect.py`・`collision-proof.py`・`review-v3.py`・`shutdown_review.py`・`test-shutdown-review.py`、`runner/summarize.py`・`audit-nic.py`） | 7 |
| Python の bytecode cache | 6 |
| `collect.py` が作った file 目録・別 tgz の sha256（`manifest-*.json`・`archive-43722.sha256`） | 3 |
| `collect.py` が qstat から計算した vnode 時間の会計（`accounting.json`、`v3-released.json`） | 2 |
| 旧判定 CSV（`cells-43706.csv`・`cells-43722.csv`） | 2 |
| log 審査の出力（`log-review.txt`・`review-43706.txt`） | 2 |
| job の log・NIC 審査から作った抜粋・判定（`log-survey.txt`・`nic-audit.txt`） | 2 |

残したもの: scheduler の原票（`review28/qstat-43706.txt`・`qstat-43722.txt`・`v3-submissions.json`）と
`runs/43706-20261008T045834Z/` の原票（`planned-cells.json`、`cells/*` の `cell.json`・`exit.json`・`ior.json`・stdout/stderr・`mdtest-ranks.csv`、
`fs/*` の FS log、`runner/` の投入・実行 script、job・host・topology の記録）。残した全 entry の bytes は旧版と同じ（manifest の `kept` の sha256）。
grader が要る原票（`pinned_raw` の 32 件: `cell.json` 30 件・`planned-cells.json`・`review28/qstat-43706.txt`）は除外と交差しない。
`gen_recompute.py` で v2 から grade-case を作り直すと、`pinned_raw`・metrics・cells の定義は v1 と同じで、`input_archive` の名前と sha256 だけが変わる。

### D3 揃えたもの
- `cases/final/k4-cmp4-validity-analysis/case.json`: `inputs[0]` の name・sha256・fetch 手順、`snapshot.materials_sha256`、`grade_case_ref.sha256`、`private_ref.sha256`。
- `materials/task.md`: 展開後の構成を v2 に合わせる（除いた file に触れない）。KB（`kb/snapshot.md`）は archive に触れていないので変えない。
- grade-case の `input_archive`（generator `fixtures/grade-cases/tools/gen_recompute.py` で再生成）、`private.json` の `expected.input`。
- `crates/campaign/scripts/verify-real-assets.sh` の既定 path、`scripts/campaign/test_check_main_pins.py`、`docs/ops/measure-run-final.md`。
- 試験の既定 path と固定 sha256（`k4_cells_validity`・`k_cases_grade`・`real_asset_grade`）。
- 合成 raw の試験 fixture（`cells_validity::synth::Plan::install_k4_fixture`）は合成用の名前 `main-v3-n2-43706.tgz` を使い、case の `inputs[0].name` もその名前に替える。
  合成 raw は実 archive と別物なので、実 case の名前（v2）とは分ける。

### D4 評価規則と分母は変えない
- 規則は ADR 2026-10-23-k4-cells-validity-grading の D1 のまま。分母は `planned-cells.json` の 32 セル、有効 30（FS 別 BenchFS 6・CHFS 8・GekkoFS 8・UnifyFS 8）。
- BenchFS W2 の 2 セルは `missing` と `job_failed(exit=-20)`（job 43706 は F / Exit_status -20）。
- `private.json` の期待値・既知欠陥・誤主張の罠は変えない（入力名だけ v2）。許容差（相対 0.001）も変えない。

### D5 試験
- 実 archive（`CAMPAIGN_K4_TGZ`=v2）: `real_k4_archive_grades_thirty_of_thirty_two` が 32/30 を出し、正しい報告は通り、
  B 腕相当（28/32）・全セル有効・失敗 job の W2 だけ有効・欠測を分母から落とす（30/30）・有効セル 1 つの帯域を 2% 誤る、を落とす。
- 合成 raw: 既存の負例（全 valid・実効 fsync=0・失敗 job を有効・欠測の分母除外・表と報告の不一致）に加え、
  件数・valid 列・報告が正しいまま有効セル 1 つの値だけを許容差の外にずらす独立の負例 `numeric_error_in_one_valid_cell_is_rejected_on_its_own` を足す。

## 帰結
- 本測定の準備（`INPUTS_SRC`・asset store の `k4/`）には v2 を置く。v1 を置くと preflight が sha256 不一致で止める。
- 判断要として残した 26 件（`runner/` の実行コード、`qstat-43722.txt`、`v3-submissions.json` など）は人の決定が出ていない。外す決定が出たら v3 として同じ手順で作る。
