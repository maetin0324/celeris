# 評価者 runbook: A/B 共通の人の介入・経過時間・やり直しの記録手順（human-ops）

---
tasks: [01M59KXY854W5C65GTG9GVQVAC]
---

対象: 多軸評価の評価者（A0/B0 主比較、B1/C1 Jev 副比較）。定義は `docs/ops/eval-raw-record-definitions.md`（以下「定義書」）、
根拠は ADR `agent-docs/adr/2026-10-19-eval-raw-human-ops-record.md`。実装は `crates/campaign/src/human_ops.rs`（`mod cli`）と
`human_ops_join.rs`。この文書の command は実装済み CLI の flag と一致させてある（`celeris-campaign human-op` の usage が正）。
測定開始目標は 2026-10-22。LLM は呼ばない（記録・検証・結合はすべて無 LLM）。

## 1. 基本方針

- A 腕と B 腕で **同じ command 列** を使う。違いは `--arm`（A0/B0/B1/C1）だけ。記録の形式・操作種別・時計は共通。
- 生記録は `<campaign out>/human-ops.jsonl`（`trials.jsonl`・`ledger.jsonl` と同じ out dir）。追記専用。直すときは訂正行を足す（元の行は書き換えない）。
- 分からない時刻は **書かない**（flag を省略する）。0 や推測値を入れない。欠測は `missing` に理由つきで残り、集計で 0 にならない。
- 評価者名は仮名（`--actor eval1`）。メール・本名は検証で拒否される。

以下の例は共通の識別を変数にして書く（`F` 以外は試行ごとに変わる）:

```sh
CC=celeris-campaign        # 実体は cargo run -p celeris-campaign -- か release の binary
F=<out dir>/human-ops.jsonl
ID="--file $F --campaign <campaign_id> --manifest <manifest.toml/json> --case <case_id> --arm A0 --rep 1"
```

`--manifest-sha256 <hash>` でも可（`--manifest` と択一）。`--trial-no` の既定は 1（runner と同じ）。B 腕は `--arm B0`（Jev 副比較は `B1`/`C1`）。
B0 と B1 は同じ ConfigId::B なので、必ず **別 campaign（別 manifest・別 out）** に記録する。

## 2. 記録の手順（A/B 同一）

### 2.1 人の操作（record）

人が何かしたら 1 件ずつ記録する。確認だけの操作（`status_check`）も記録する（人の時間を使ったため）。

```sh
# 終わった操作（開始・終了とも分かる）。時刻は UTC の RFC 3339。
$CC human-op record $ID --op-kind research_direction --actor eval1 --mode manual --intent research \
  --trigger system_request --logical-op p1/answer \
  --started-at 2026-10-22T00:10:00Z --ended-at 2026-10-22T00:12:00Z

# その場で記録（時計の値を使う。time_basis=evaluator_live になる）
$CC human-op record $ID --op-kind status_check --actor eval1 --mode manual --intent monitoring \
  --trigger scheduled --logical-op p1/look --started-at now --ended-at now

# 終了が分からない操作: --ended-at も --outcome も付けない → op_start だけが残り end は missing
$CC human-op record $ID --op-kind resubmit --actor eval1 --mode manual --intent system --trigger failure \
  --logical-op p3/submit-train --attempt 2 --reason "sbatch が queue 満杯で失敗したため出し直し" \
  --started-at 2026-10-22T03:00:00Z
```

- 終了が後で分かったら、**同じ identity・同じ開始時刻**で `--ended-at`（と `--outcome ok|failed|abandoned`）を付けて再実行する。
  開始行は同一なので増えず、`op_end` だけが足される。実際の開始時刻を変えると conflict で exit 1（上書きしない）。
- `now` で始めた記録の再開（live 開始 → 明示終了）は次の順で行う。前段の試験（`human_ops_cli.rs`）と同じ手順:
  1. live 開始: `$CC human-op record $ID ... --started-at now`（開始行は `time_basis=evaluator_live`、`recorded_at` は CLI の時計）。
  2. `$CC human-op list --file $F --arm <arm> --case <case>` で開始時刻（`start`）を読んで控える。
  3. 同じ identity で `--started-at <list に出た時刻> --ended-at <終了時刻>` を付けて再実行する（明示終了。`op_end` が 1 行足される）。
  4. 同じ command をもう一度流しても行は増えない（`duplicate`）。開始時刻が違えば conflict のまま。
  - 時計の保存規則: 開始行の `time_basis`/`time_precision` は**先に書いた行のものを保つ**（再送で `evaluator_live` →
    `evaluator_reported` に変わっても拒否・書き換えはしない）。比較から外れるのは `recorded_at`/`clock_source`/`host`/
    `time_basis`/`time_precision` だけで、開始・終了の時刻値と actor は比べ続ける。終了側の `time_basis` は終了行に別に残る。
- `--outcome` だけ付けると op_end は書かれるが end 時刻は missing（`unknown`。`--missing-reason` で変更可）。
- `--reason` は `resubmit`/`resume`/`system_adjustment`/`system_recovery`/`manual_handoff`/`unclassified` で必須（1〜3 文）。
- `--target kind:ref`（kind は artifact/job/task/run/session/config、繰り返し可）、`--task-id`/`--run-id`/`--celeris-event-ref event:<task_id>:<seq>` は B で対応 event があるとき付ける。
  A は task/run が `not_applicable` として自動で記録される。
- `--mode manual|automatic`: 人が手で → manual、評価者が起動した script → automatic（`--actor-kind script --actor <script名>`）。
  Celeris 自身の自動処理は event 側にあるので書かない。

### 2.2 人待ち・やり直し・外部 job・打ち切り（span）

```sh
# 人待ち（A のみ書く。B は event から導出されるので書かない）
$CC human-op span $ID --kind wait --wait-kind permission_prompt --logical-op p2/wait \
  --started-at 2026-10-22T01:00:00Z --ended-at 2026-10-22T01:05:00Z

# 論理作業の 1 回の試み。やり直しは同じ --logical-op で --attempt を増やす（result: ok|failed|cancelled|timeout|abandoned）
$CC human-op span $ID --kind attempt --logical-op p3/submit-train --attempt 1 --result failed \
  --started-at 2026-10-22T02:00:00Z --ended-at 2026-10-22T02:40:00Z

# 外部クラスタ job の scheduler 計上値（A/B 同じ書式 <scheduler>:<cluster>:<job_id>。sacct/qstat の値を使う）
$CC human-op span $ID --kind job --job-ref slurm:pegasus:12345 \
  --job-submit 2026-10-22T03:01:00Z --job-start 2026-10-22T04:30:00Z --job-end 2026-10-22T06:00:00Z \
  --time-basis scheduler_accounting --time-precision second

# 観測打ち切り（その試行の観測をここで止める）
$CC human-op span $ID --kind cutoff --occurred-at 2026-10-22T09:00:00Z
```

Celeris の run（B の自動 retry・repair）は `runs.seq` から導出されるので attempt として書かない。書くのは人の作業と A の作業。

## 3. 操作種別の選び方

定義書 §3.2 の表が正。早見:

| 状況 | op_kind | intent |
|---|---|---|
| 結果・ファイルを人が写した | `manual_transcription` | system |
| 進捗・状態を見ただけ | `status_check` | monitoring |
| 失敗した job/task/session を人が投入し直した | `resubmit` | 障害が原因 → system、研究上の条件変更 → research |
| 中断を同じ状態から続けさせた（API キー更新後、`--resume`） | `resume` | system |
| 研究の内容・方法・範囲を人が選んだ（質問・decision への回答） | `research_direction` | research |
| 危険・費用・外部送信・credential の可否を決めた | `safety_approval` | safety |
| 研究と無関係な道具・設定・権限の手直し（失敗前） | `system_adjustment` | system |
| 障害・停止から戻した（daemon/tunnel 再起動、kill、workspace 修復） | `system_recovery` | system |
| 本来システムがやる工程を人が自分で実行した | `manual_handoff` | research / system |
| 試行開始・cutoff 宣言・grade 起動（評価者の測定作業） | `protocol_step` | protocol（介入に数えない） |
| どれとも決められない | `unclassified`（理由必須。推測で寄せない） | 評価者が選ぶ |

判定規則: 1 つの行為が 2 種に当たるときは **その行為を必要にした原因** で選ぶ。`status_check` のあと何かを変えたら、変えた種別の op も別に書く。
`--trigger` は `system_request`（システムが人を求めた）/ `failure` / `scheduled` / `self_initiated`（既定）。

## 4. 時計の扱い

- `recorded_at` は CLI が自 host の時計で入れる（評価者は書かない）。`clock_source` の既定は `system_utc`。
  NTP 同期を確認できたときだけ `--clock-source system_utc_ntp_synced`、不明なら `unknown`。host 名は自動（`--host` で上書き）。
- 事実時刻（start/end/occurred_at）: `now` → 時計の値で `evaluator_live`、明示時刻 → `evaluator_reported`。
  transcript や scheduler 値を使うときは `--time-basis harness_transcript|scheduler_accounting|celeris_event` と
  `--time-precision second|minute|approximate` を付ける。幅のある申告は `approximate`。
- host 間の時計ずれは補正しない。測定開始前に 2 host の `date -u` を比べて差を進捗ファイルに控える（§8）。
- 時刻は必ず UTC（`Z` 付き）。ローカル時刻を書かない。

## 5. 中断・再開時の注意

- 冪等: 同じコマンドの再実行・二重実行で行は増えない（出力は `appended` / `duplicate`）。途中で落ちた・出力を見失ったら **同じ command をそのまま再実行** してよい。
- 同じ record_id で内容が違うと conflict（exit 1）。直したいときは訂正を足す。時刻を間違えて入れたら、正しい値で再実行せず、
  まず `list` で現状を見て、訂正の方法を相談する（現 CLI に `correction` を書くサブコマンドは無い。§7 の限定参照）。
- 評価の中断（API キー上限・session 切れ）の前後は、人の `resume` 操作と `wait` を記録し、中断中の時間を 0 にしない。
  いつ止まったか分からない場合は start を省略（`unknown`）。
- 打ち切り（`cutoff`）後の出来事は数えない。試行を途中で止めるときは先に cutoff を記録する。
- 壊れた行（手編集・途中書き込み）がある file には追記されない。`validate` が `line N:` を出すので、先に直す。
- file を手で編集・並べ替えない。バックアップは複製のみ。

## 6. 確認と join

```sh
$CC human-op validate --file $F                     # ok: N records / 壊れた行は line N: 付きで exit 1
$CC human-op list --file $F --arm A0 --case <case>  # correction 適用後の行、op の start/end、duration_secs（端が欠ければ null）、redo_counts
```

join の入力は **別々の 2 つの出力**を合わせて読む。どちらも既存の通常経路の出力で、手で写さない・組み立て直さない。

| 出力 | 出す command | 書かれる file | join での意味 |
|---|---|---|---|
| runner の out dir | `celeris-campaign run`/`watch`（`--out <out dir>`） | `trials.jsonl`・`ledger.jsonl` | 試行の台帳（wall_secs・started/submitted・run_ids）。A/B 両方 |
| decision-log export | `celerisctl --db <DB> campaign export <campaign-id> --out <dir>` | `records.jsonl`・`summary.json` | B の Celeris event 由来（run・outcome・intervention） |
| 集計の export（join には使わない） | `celeris-campaign export --manifest <m> --out <out dir>`（`--dest` 既定 `<out>/export`） | `table(.incomplete).json/csv`・任意の `final-accounting.json` | 表と総消費。`records.jsonl`・`trials.jsonl`・`ledger.jsonl` は**出さない** |

`celeris-campaign export` の出力 dir を `--events` に渡しても入力無しエラーになる。join には上の 1・2 行目を使う。

decision-log の取得: `celerisctl` は `--db` > 環境変数 `CELERIS_DB` > `CELERIS_RUN_DB` > `./celeris.sqlite3` の順で DB を選ぶ（接続設定は
この既存の DB path だけで、token・URL は要らない。campaign export は DB を読むだけ）。本番 DB は直接読まず、写しに対して流す
（手順は `docs/ops/measure-run-final.md` §8 と同じ）:

```sh
celerisctl db backup $TMPDIR/eval-db.sqlite3 --src <Celeris DB path>
celerisctl --db $TMPDIR/eval-db.sqlite3 campaign export <campaign_id> --out <dl dir> --manifest-hash <manifest sha256>
# <campaign_id> は record の --campaign と同じ値。<dl dir> に records.jsonl と summary.json が出る
$CC human-op join --file $F --events <out dir> --events <dl dir>
```

- `--events` は繰り返せ、各入力は dir（`trials.jsonl`・`ledger.jsonl`・`records.jsonl` のうちあるもの）か file。runner の out dir と decision-log の dir を同時に渡す。
- A 腕には Celeris の DB event が無い（`celerisctl` の export に A の行は出ない）ので、対応する event が無い欄は `missing` のまま保つ。
  A の欄を trials/ledger やコピーで埋めない。
- 成果物（export 出力・human-ops.jsonl）を手作業でコピーして join の前提にしない。上の command の出力 dir を直接指す。
- 実 pilot でのこの経路の通し確認は未実施（試験は通常 runner の MockExecutor 出力→join と、`decision_log_export_campaign` の出力を使う）。

- `join` は identity（manifest_sha256・case・arm・rep）ごとに 1 行の JSONL を出す。各欄は `{value, source}`、source は
  `event`（Celeris 由来: wall_secs・run_wall_ms・started/submitted・run_ids・介入の件数と区分）/ `evaluator`（人の操作・attempt・待ち・job 観測）/ `missing`。
  `missing_fields` と `missing_count` が付く。A 腕は decision-log export に run 行が無いので event 由来の欄は missing のまま（正常）。
- 結べなかったものは捨てず `unjoined_event`（identity の無い task の介入）/ `unjoined_evaluator` 行に出る。0 件が理想だが、出たら理由を確認する。
- 出力は決定的（再 export・再実行で byte 一致）。B の介入件数は生記録を正とし、projection は件数の照合にだけ使う（加算しない）。
- join は `outcome`・`failure_reason`・`tokens`・`cost_micro_usd`（TrialRecord 由来。未実行の 0 は欠測）も持つ。

### 6.1 比較表・機械可読出力・タイムラインの再生成（`human-op report`）

```sh
$CC human-op report --file $F --events <out dir> --events <dl dir> --out <report dir>
```

`<report dir>` に `report.json`（機械可読。正本）・`report.md`（表。注意書き付き）・`timeline.jsonl`（区間 1 行ずつ）を書く。同じ入力は byte 一致で再生成される。
表・集計値は手編集しない（直すときは生記録に訂正を足して再生成）。

- 人の時間は `mode=manual` の op の区間の**和集合**（重なりの二重計上なし。`sum_secs` と `overlap_secs` を併記）。`protocol_step`・script の op は数えない。
  終了不明は `open`、始点不明は `missing_start`、`end<start` は `inverted` に件数で残し、長さに入れない（`lower_bound: true`）。cutoff 以降は数えず、跨ぐ区間は cutoff で切る。
- 待ち: `human_wait`（`wait_*`）、`external_queue_wait`（job の submit〜start）、`external_run`（start〜end）。同じ `job_ref` は 1 件。`internal_wait` は現在の入力に無く欠測。
- 再作業: 同じ `logical_op` の attempt n+1 を再作業とし、原因は n+1 の op の intent（system→`infrastructure`、research/safety→`research`、他→`unclassified`）。失敗 attempt の所要時間も併記。
- 完遂/失敗原因は TrialRecord の outcome・reason から。分類は **暫定 schema** `provisional-failure-cause/1`（分類 task 確定後に `failure_cause` の写像を合わせる）。
- 記録された人の時間は観測範囲の下限で、総労働時間・認知負荷ではない。A/B の観測方法差（time_basis を併記）、case/arm/rep の無い共有 overhead（`shared_overhead` に別掲し配分しない）と欠測を `report.md` に併記する。
- 実 pilot 記録（研究評価接続 3）との接続は未確認。試験は fixture（`crates/campaign/tests/human_ops_report.rs`）。

## 7. 記録できない指標と主張の限定

定義書 §6 が正。要点:

- 評価者が気付かなかった・画面外の時間は記録されない → 人の時間は **下限**。「X 減った」は記録された操作の範囲の主張に限る。
- 単一評価者で盲検でなく、分類の評価者間一致は測れない。
- A の harness 内部（tool 再試行・自動 compact・内部待ち）は transcript を見ない限り見えない。A の ready_not_running は定義上 not_applicable。
- human_wait は B が event（秒精度）、A が評価者記録で測り方が違う → 比較表に `time_basis` を並べる。
- intervention projection は `wall_ms` を持たず ApprovalDecided/Edited を区別しない。campaign 識別の無い task の介入は projection に出ない。
- `TrialRecord.wall_secs` は整数秒・timeout で丸め。未実行・executor_error の 0 は欠測として扱う。
- external_queue_wait はクラスタの混雑に依存し、システム性能として帰属させない。
- 試行数は承認枠（12 試行以内）で小さく、統計的有意性は主張しない。研究の質・人の判断の質・認知負荷は測らない。
- 実装上の限定: `--target` の sha256 は常に null／訂正（`correction`）を書く CLI は無い（型・読み出しは対応）／並行追記は排他せず読み出し側で潰す。

## 8. 測定開始（10/22 目標）前のチェックリスト

- [ ] 評価者が §2 の command を A0・B0 で 1 回ずつ、試し用の out dir で通した（本番 out に試し行を入れない）。
- [ ] `human-op validate` が ok、`list` で A0/B0 の行が同形式（`arm_label` だけ違う）。
- [ ] `human-op join --events <out dir> --events <dl dir>` を試し out（mock 実行の trials/ledger）で通し、A 腕の event 欄が missing、B 腕が event になることを見た。
- [ ] 実験セットごとに campaign_id・manifest・out が別（A0/B0 と B1/C1 を混ぜない、B0 と B1 も別 manifest）。
- [ ] 評価者の仮名（actor id）を決めた。メール・本名を使わない。
- [ ] 2 host（評価者端末・Celeris host）の `date -u` と NTP 同期を確認し、差と `clock_source` の方針を進捗ファイルに控えた。
- [ ] 操作種別表（§3）を読み、迷う例（再投入が障害か研究か等）の方針を決めた。
- [ ] 記録できない指標（§7）を報告の限定欄に入れる前提で合意した。
- [ ] 実 LLM 検証・本測定の自動開始・予算増額は別承認。この runbook は承認を含まない。
- [ ] `human-ops.jsonl` を測定の out と一緒に保全する手順（複製のみ）を決めた。
