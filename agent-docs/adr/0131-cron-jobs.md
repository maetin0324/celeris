---
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
---
# ADR-0131: 汎用の定期実行（cron job）基盤と、知識ベース・受信箱の日次整理

- 日付: 2026-10-02
- 状態: 採用（実装前。実装は同 task の葉 cron-model / inbox-rules / cron-fire / api / cli / dispatch-tick /
  gui / curation-job / dry-run / ops-verify）
- 関連: CLAUDE.md「ディスパッチャやストアに LLM 呼び出しを入れない」、ADR-0047 D4（知識の自動メンテナンス
  `knowledge_maint`）、ADR-0068（Knowledge GC）、ADR-0069（担当・lane の決定的な選択）、ADR-0079（再帰 task 木）、
  ADR-0082（Dispatcher の分割）、ADR-0095 付記 D-d（本番 host の操作は人）、ADR-0010 D1（cancel の遷移）、
  docs/testing.md（時計の差し替え）、受信箱・通知の分離 task `01M3YFCJKMNWQ13HRS52M5BSWW`（D7 の分担）

## 状況

- 本番 KB（`~/.local/share/celeris/knowledge`）は 360 ページ、`_inbox/` に 158 件が未整理。`projects/` 104・
  `environment/` 31・`experience/` 31・`skills/` 31 で重複・古い記述が混ざり始めている。
- 既存の知識系の自動処理はどれも「task 単位」か「機械的な GC」で、KB 全体を見直す仕事は無い:
  - `crates/celeris/src/knowledge_maint.rs`（ADR-0047 D4）: 終端になり報告ができた task ごとに `knowledge`
    harness（adapter `langmem`、tier cheap）の支援 task を 1 件作り、出た候補を `_inbox/` に入れる。
  - `crates/celeris/src/knowledge_gc.rs`（ADR-0068）: 決定的な GC（索引・ハッシュ・死んだ参照）。
- 受信箱（`task_ops::inbox::build_attention`）は「24h 以内に `failed`」を無条件に attention に出す。親が別の子で
  通り直して `done` になっても、置き換えられた `failed` の子が残る（例 `01M3WCAQ762PKQ75CR6K03236M`）。`failed`
  は `Trigger::Cancel` の対象外（非終端からのみ `cancelled`）なので人が消す手段も無い。
- 「毎日 1 回 X をする」を書く場所が無い。daemon の tick には GC・doc gardener・knowledge_maint が個別に
  ハードコードされている。
- workspace の時刻依存は `time` 0.3 のみで、IANA タイムゾーン DB を読む手段が無い。

人の方針（2026-10-02）: 汎用の定期実行（cron job）を Celeris に持たせ、最初の job として KB と受信箱の日次整理を
載せる。job は DB に置き、API・celerisctl・GUI から操作する。発火は daemon の決定論的な tick の中で時刻を見て
task を作るだけ。作られた task は通常の task と同じく routing・計画・review を通る。

## 決定

### D1. DB の形・job 雛形・schedule の表現と時刻計算

migration `0039_cron_jobs.sql`（0039 が使われていれば次の空き番号）で 2 表を足す。

```sql
CREATE TABLE cron_jobs (
  id            TEXT PRIMARY KEY,          -- ULID
  name          TEXT NOT NULL UNIQUE,      -- 人が読む識別子（例 "daily-curation"）
  enabled       INTEGER NOT NULL,          -- 0 = 一時停止
  schedule      TEXT NOT NULL,             -- 5 欄の cron 式
  timezone      TEXT NOT NULL,             -- IANA 名（例 "Asia/Tokyo"）
  overlap       TEXT NOT NULL,             -- 'skip' | 'queue'（D2）
  catch_up      TEXT NOT NULL,             -- 'latest' | 'skip'（D3）
  template_json TEXT NOT NULL,             -- CronTaskTemplate（下記）
  next_fire_at  TEXT,                      -- UTC RFC3339。enabled=0 のとき NULL
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);
CREATE TABLE cron_job_runs (
  id            TEXT PRIMARY KEY,          -- ULID
  job_id        TEXT NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
  scheduled_for TEXT NOT NULL,             -- 発火の予定時刻（UTC）。手動実行は押した時刻
  trigger       TEXT NOT NULL,             -- 'schedule' | 'catch_up' | 'manual'
  outcome       TEXT NOT NULL,             -- 'created' | 'queued' | 'skipped_overlap' | 'skipped_missed' | 'error'
  task_id       TEXT,                      -- outcome = created のとき作った task
  detail        TEXT,                      -- skip/error の理由（人が読む 1 行）
  recorded_at   TEXT NOT NULL,
  UNIQUE (job_id, scheduled_for, trigger)
);
CREATE INDEX cron_job_runs_job ON cron_job_runs(job_id, recorded_at DESC);
```

`UNIQUE (job_id, scheduled_for, trigger)` が二重発火の最後の砦になる（daemon の再起動・手動と定時の衝突でも
同じ予定時刻の task は 1 件）。行の挿入と task の作成は同じ transaction で行う。

**job 雛形**（`task_core::cron::CronTaskTemplate`、serde で `template_json`）:
`title`（`{date}`＝発火時刻の job タイムゾーンでの `YYYY-MM-DD` を置換）・`objective`・`acceptance`
（`task_ops::add` の受け入れ条件と同じ形）・`assignee`（任意。省略時は ADR-0069 の決定的選択）・`harness`
（genre/harness id、任意）・`lane`（`cheap|standard|frontier`、任意）・`project`（任意）・`repos`（任意）・
`priority`（任意）。雛形は作成・更新時に `task_ops::add::NewTaskSpec` へ変換して検証し、通らなければ 400。
雛形から作る task の `origin` は `cron`（新しい値）で、`lane` を書いた job だけ lane policy を明示で上書きする
（ADR-0069 D3 の `TaskFeatures` 上書きと同じ扱い）。

**schedule**: 5 欄（分 時 日 月 曜日）の cron 式 + IANA タイムゾーン。秒欄・`@reboot`・`L`/`W`/`#` は持たない。
`*`・数値・範囲 `a-b`・列挙 `a,b`・刻み `*/n`・`a-b/n`、月/曜日の英略称、曜日 `0` と `7` = 日曜。日欄と曜日欄が
両方とも `*` でないときは Vixie cron と同じ OR。`@daily`/`@hourly` 等の別名だけは展開して受ける。

**依存**: cron 式の parser は自前（task-core、数百行で済み、外部 crate の方言差を持ち込まない）。タイムゾーンは
`jiff`（既定 feature。Unix では `/usr/share/zoneinfo` を読み、host の tzdata 更新に追従する）を task-core に足す。
`tz-rs` は DST の曖昧・欠落時刻の解決を自前で書く必要があり、`chrono-tz` は tzdb を binary に焼き込み host の
更新に追従しないため採らない。`time` は既存の型のまま使い、境界で `jiff::Timestamp` と相互変換する。
存在しない IANA 名は作成・更新時に 400、起動時に読めなくなった job は `error` の履歴を 1 件残して `enabled` は
変えない（次の tick で再試行）。

**次回時刻の計算** `next_after(schedule, tz, after_utc) -> Option<Timestamp>`: job のタイムゾーンの壁時計で
`after` の次の分から、月→日→時→分の順に欄ごとに桁上げして探す（分刻みの総当たりはしない）。探索は 5 年先で
打ち切り、見つからない式（例 `0 0 31 2 *`）は作成時に 400。

**DST**:
- 存在しない壁時計時刻（春の飛び）: その日の分は「ずらして 1 回」発火する（jiff の `compatible`、飛びの後の
  最初の時刻）。`30 2 * * *` は飛ぶ日には 03:30 に 1 回。
- 重複する壁時計時刻（秋の戻り）: 早い方の offset で 1 回だけ。`next_after` は「前回の予定時刻より後」の UTC で
  探すので、遅い方の同じ壁時計時刻を 2 回目として拾わない（壁時計の (日付, 時, 分) が前回と同じなら飛ばす）。
- 毎時の式（`0 * * * *`）は UTC で見て等間隔でなくてよい（壁時計の各時に 1 回）。

### D2. 重ね掛けの規則（overlap）

job ごとに `overlap = skip | queue`（既定 `skip`）。判定は「この job の最後の `created` の task が非終端か」
（`Done`/`Failed`/`Cancelled` 以外）で行う。子 task・計画 run を含め木の根の状態で見る。

- `skip`: 非終端なら task を作らず、`outcome = skipped_overlap`（`detail` に前回 task id）を記録して次回時刻へ進む。
- `queue`: 非終端なら `outcome = queued` を 1 件だけ記録する（既に `queued` があれば新しい発火は `skipped_overlap`。
  溜めるのは高々 1 件）。前回 task が終端になった後の最初の tick で、`queued` の行を `created` に更新して task を
  作る。`queued` 中に job が一時停止されたら `queued` は `skipped_overlap`（detail "paused"）に閉じる。
- 手動実行（D5）も同じ規則に従う。`?force=true` は無い（重ねたいなら人が通常の task として作る）。

### D3. 取りこぼし（daemon 停止中に過ぎた時刻）

`next_fire_at <= now` のとき、`now` までに過ぎた予定時刻の列を見る（列挙は最新の 1 件と件数だけで足り、全件は
数えない。件数は `next_after` を最大 1000 回で打ち切る）。

- `catch_up = latest`（既定）: 過ぎた予定時刻のうち**最新の 1 件だけ**を `trigger = catch_up` で発火する
  （D2 の overlap 判定を通る）。それより古い分は 1 行にまとめて `outcome = skipped_missed`（`detail` に件数と
  範囲）を残す。
- `catch_up = skip`: 過ぎた分はすべて `skipped_missed` 1 行にまとめ、何も作らない。
- 遅れの許容: 予定時刻からの遅れが 1 tick 以内（通常運転）は `trigger = schedule`、それを超えたら `catch_up` と
  記録するだけで、上限で捨てる規則は持たない（job の周期が上限の役をする）。
- どちらの場合も `next_fire_at` は `next_after(now)` に更新する。
- 一時停止中に過ぎた時刻は取りこぼしではない: 再開時に `next_fire_at = next_after(now)` から始める。

### D4. 発火の置き場所（決定論・LLM なし）

- 発火処理は `task_ops::cron`（新 module）に置く: `fire_due(store, now) -> Vec<CronFireOutcome>` と
  `run_now(store, job_id, now)`。API の手動実行（D5）と tick は同じ関数を通る。中身は「`cron_jobs` を読み、
  D1〜D3 の規則で `cron_job_runs` を書き、雛形から `task_ops::add` で task を作る」だけで、LLM・外部 process・
  network を呼ばない。
- 起動は `Dispatcher::tick`（`crates/task-dispatch/src/dispatcher.rs`）の新しい段 `fire_cron_jobs` で、
  時刻は `self.now_utc()`（試験では `test_now` で差し替わる）を使う。段の位置は `drain_completions` と
  木の照合の後、新規 run の起動の前（同じ tick で作った task が ready なら同じ tick で拾われうる）。失敗は
  `tracing::warn` にして tick を止めない（approvals・browser wait と同じ扱い）。
- `fire_due` は 1 tick に job あたり高々 1 件の task しか作らない。複数 daemon が同じ DB を触る構成は持たないが、
  D1 の UNIQUE 制約で二重作成は起きない。
- 作られた task は通常の task と同じく routing（ADR-0069）・計画・review を通る。cron は担当・lane を
  雛形に書いたときだけ明示し、書かなければ通常の決定に任せる。作られた task に `Event::CronFired { job_id,
  cron_run_id, scheduled_for, trigger }` を追記し、task 詳細から job へ辿れるようにする。
- daemon 停止中は何も起きない（D3 で次の起動時に扱う）。tick の間隔（既定数秒）が発火の粒度で、分単位の
  schedule には十分。

### D5. 操作面（API・celerisctl・GUI）

API（`/api/v1`、task-api。schema・`docs/api` は既存の再生成手順に従う）:

| method | path | 内容 |
|---|---|---|
| GET | `/cron-jobs` | 一覧（name, enabled, schedule, timezone, next_fire_at, 最後の run の outcome/task） |
| POST | `/cron-jobs` | 作成（D1 の検証。作成直後の `next_fire_at` を返す） |
| GET | `/cron-jobs/{id}` | 詳細（雛形を含む） |
| PATCH | `/cron-jobs/{id}` | 更新（schedule/timezone を変えたら `next_fire_at` を再計算） |
| DELETE | `/cron-jobs/{id}` | 削除（履歴も消える。作った task は残る） |
| POST | `/cron-jobs/{id}/pause` / `/resume` | 一時停止・再開（D3 の再開規則） |
| POST | `/cron-jobs/{id}/run` | 手動実行（`trigger = manual`、D2 に従う。結果の outcome を返す） |
| GET | `/cron-jobs/{id}/runs?limit=` | 履歴（新しい順） |

`{id}` は ULID か `name` のどちらでも引ける。書き込み系は既存の API 認証に従う（人の操作）。

celerisctl: `celerisctl cron list | show <job> | add --name … --schedule … --tz … --template <file.json>
[--overlap skip|queue] [--catch-up latest|skip] [--disabled] | update <job> … | pause <job> | resume <job> |
run <job> | runs <job> [--limit N] | rm <job>`。表示は既存のサブコマンドと同じ表形式と `--json`。

GUI（`gui/`）: 最小限の 2 画面。`/cron` に一覧（有効/停止・schedule・tz・次回・最後の結果・一時停止/再開/
手動実行のボタン）、`/cron/{id}` に雛形の表示と履歴の表（予定時刻・trigger・outcome・task への link）。作成・
雛形の編集は GUI に置かない（celerisctl / API で行う）。新 SPA（`web/`）への移植は ADR-0081 の parity の
範囲で後から行い、この task ではしない。

### D6. 最初の job: 知識ベースと受信箱の日次整理

seed（`config/celeris.example.toml` の `[[cron.seed]]`、空の `cron_jobs` に一度だけ入れる。org.toml と同じ
「DB が正、設定は種」の扱い）:

```toml
[[cron.seed]]
name = "daily-curation"
enabled = false                      # 本番で有効にするのは人（ops 手順）
schedule = "30 4 * * *"              # 時刻は設定で変える
timezone = "Asia/Tokyo"
overlap = "skip"
catch_up = "latest"
[cron.seed.template]
title = "日次整理: {date}"
harness = "knowledge-curation"
lane = "cheap"
project = "agent-platform"           # 例。本番では人が選ぶ
mode = "dry_run"                     # 'dry_run' | 'apply'
```

- **harness**: 新しい `[[harnesses]] id = "knowledge-curation"`（coding 系の汎用 adapter、tier cheap、
  fallback は既存 `knowledge` と同じく cheap の汎用 harness）。既存の `[[harnesses]] knowledge`（adapter
  `langmem`）は task 単位の候補抽出に特化していて KB 全体の統合・削除はできないので流用しない。
- **既存経路との関係**: `knowledge_maint`（langmem）は今のまま task ごとの候補を `_inbox/` に入れる「入口」で、
  日次整理は `_inbox/` を空にする「出口」。`knowledge_gc` も今のまま（索引・死んだ参照の機械的な修復）で、
  日次整理はその後の内容判断を受け持つ。task ごとの「知識整理: …」支援 task を減らすことはこの ADR の範囲外
  （日次整理の記録を見て人が決める）。
- **run の入出力**: run には本番 KB の**写し**（作業場所の中）と、D7 の決定論的規則を適用した後の受信箱の
  一覧（JSON）を渡す。run は写しを書き換え、`artifacts/curation-plan.json`（件ごとの `merge|new|delete|keep`
  と理由・移動先）、`artifacts/curation.diff`（写しの before/after の unified diff）、
  `artifacts/daily-summary.md`（D8）を出す。人が書いたページ（`user/` 等、frontmatter に `source: human` の
  もの）を消す・大きく書き換える案は実行せず、1 件の decision にまとめる。
- **本番 KB への反映**: `mode = dry_run` では何も反映しない（差分と要約だけ）。`mode = apply` は、人が日次の
  要約を承認した後に daemon が `curation-plan.json` を決定的に適用する（既存の
  `task_ops::knowledge::apply_candidates` と同じく、書き込みは daemon のコードで行い LLM は書かない）。削除を
  基本とし archive はしない。消したページの path と 1 行の理由は要約と `knowledge/_curation/YYYY-MM-DD.md`
  （整理の記録）に残す。seed は `enabled = false` / `mode = dry_run` で入り、`apply` への切り替えは本番 KB の
  dry-run 差分を人が確かめた後に人が PATCH する（ADR-0095 付記 D-d）。
- 受信箱について run に任せるのは D7 の決定論的規則で外せなかった項目の判断（古い報告を閉じてよいか、
  failed を諦めてよいか等）で、run は提案を `curation-plan.json` の `inbox` 節に書くだけ。状態を変える操作
  （cancel 等）は人が要約から行う。

### D7. 受信箱の決定論的な片付け規則

規則は `task_ops::inbox` の attention 構築（`build_attention`）に入れる**派生の絞り込み**で、状態を書き換えない
（表示から外すだけなので取り消しが効き、DB の正を変えない）。外した件数は規則ごとに数えて Inbox の応答に
`suppressed: {rule: count}` として返す。

- **R1 置き換え済みの failed 子**: `failed` の task の親（`parent_id`）が `done` なら attention から外す。親が
  `done` になった時点で、その子の失敗は別の子・再計画で吸収されている（ADR-0079 の木では、親の `done` は
  final review を通った後にしか来ない）。例 `01M3WCAQ762PKQ75CR6K03236M`。
- **R2 取り消された木の failed 子**: `failed` の task の祖先のどれかが `cancelled` なら外す。
- **R3 終端 task に残った attention**: attention の対象 task が `done` / `cancelled` なら、理由の種類（requeue
  上限・unroutable の古い snapshot・未決の review 等）に関係なく外す。`failed` 自身は R1/R2 に当たらない限り残す。
- **R4 同じ unit の再試行で通った failed**: 同じ親・同じ計画 unit key の兄弟 task に、この task より後に作られた
  `done` があれば外す（親がまだ `done` でなくても置き換えは確定している）。
- **failed の cancel**: `Trigger::Cancel` を `Failed` からも許す（`Failed → Cancelled`、attempts 不変、
  `Transitioned.reason = "cancel_failed"`）。対象は人の操作（API・celerisctl・GUI）だけで、daemon と run は
  自動で使わない。`cancelled` は再開できない（worktree が無い）ので、取り消しは「諦める」の意味として人が
  選ぶ。親の `ChildFailed` の判定は子の遷移時に済んでいるので、後からの cancel は親の状態を変えない。
- **分担**: 片付けの規則（R1〜R4 と failed の cancel）はこの task の葉 inbox-rules が `task_ops::inbox` に
  実装し、純関数 `attention_suppression(task, by_id) -> Option<SuppressRule>` として公開する。受信箱・通知の分離
  task（`01M3YFCJKMNWQ13HRS52M5BSWW`）は通知と受信箱の振り分けを受け持ち、規則はこの関数を呼んで使う
  （規則を重複して持たない）。
- 判断が要るもの（古い報告の既読化、R1〜R4 に当たらない failed を諦めるか）は日次整理 run の提案（D6）に回す。

### D8. 人への出力: 1 日 1 件の要約

日次整理 task の報告（既存の report の仕組み）を、人が受け取る**唯一の日次の 1 件**にする。本文は
`artifacts/daily-summary.md` で、次の固定の節を持つ（数十秒で読める量。1 節 10 行まで、超えた分は件数と
artifact への link）:

1. 見出し `日次整理 YYYY-MM-DD（dry_run|apply）`
2. KB: `_inbox` の処理件数（統合 n・新規 n・削除 n・保留 n）、重複の統合 n 組、古い記述の修正・削除 n 件
3. 削除したページ（path と 1 行の理由）
4. 受信箱: D7 で外れた件数（規則ごと）、run が「閉じてよい」と提案した項目
5. 人に決めてほしいこと: 1 件の decision（人の書いたページの削除・判断の要る受信箱項目をまとめる）

同じ日に手動実行で 2 件目が作られた場合も要約は task ごとに 1 件で、日付の重複は job の overlap と D1 の
UNIQUE が防ぐ範囲に留める。

### D9. 試験は注入した時計で決定的に

- 時刻計算（task-core）: `next_after` の純関数試験。DST は host の tzdata に依存しないよう POSIX TZ 文字列
  （`jiff::tz::TimeZone::posix("EST5EDT,M3.2.0,M11.1.0")` 等）で春の飛び・秋の戻り・月末・閏年・日と曜日の OR を
  固定する。IANA 名の解決は `Asia/Tokyo`（DST なし）で 1 件だけ。
- 発火（task-ops）: `fire_due(store, now)` に `now` を渡す試験で、overlap skip/queue・catch_up latest/skip・
  手動実行との衝突（UNIQUE）・一時停止中の時刻・再開を、メモリ上の SQLite store に対して検査する。
- tick（task-dispatch）: Dispatcher の `test_now` を進めて tick を回し、予定時刻の前は作られず、過ぎた最初の
  tick で 1 件、次の tick では増えないことを確かめる（既存の `tick_until` 型の helper を使う）。
- 受信箱（task-ops）: R1〜R4 と `cancel_failed` の遷移を固定の task 木で検査する。
- 実時間の sleep・busy loop・負荷は使わない（docs/testing.md）。LLM を伴う整理 run の実機確認は dry-run 葉で、
  本番 KB の**写し**に対して行い差分を記録する。

## 帰結

- 定期の仕事を足すのはコードの追加ではなく DB の行の追加になる。daemon の tick に個別に生えた GC 類を
  将来 cron job に寄せる余地ができる（この ADR では移さない）。
- `jiff` が task-core の依存に加わる（タイムゾーン計算の範囲だけで使い、既存の `time` は置き換えない）。
- 受信箱の attention は派生の絞り込みで減るが、DB の状態は変えないので、規則の誤りは規則を直せば戻る。
  `Failed → Cancelled` だけは状態遷移の追加で、人の操作に限る。
- 日次整理は初めは dry-run で、本番 KB の書き換えは人の確認後。apply の判断・時刻・project は人が選ぶ。
- 却下した案: dispatcher 外の別 thread の scheduler（tick と時計が二重になり試験が決定的でなくなる）、systemd
  timer + celerisctl（DB に job が無く API/GUI から見えない、本番 host の設定変更が要る）、failed 子の自動
  cancel（状態を書き換え、再開の道を断つ）。

## 付記 D10（2026-10-03）日次整理 job の実装方針

この付記は日次整理に関して D6 の「既存経路との関係」「run の入出力」「本番 KB への反映」と D8 の出力方法を
具体化し、矛盾する箇所を上書きする。D1〜D9 の本文は変更しない。

1. **task ごとの知識整理を止める。** 人の決定に従い、`crates/celeris/src/knowledge_maint.rs` の
   `schedule` は呼ばず、終端 task ごとの「知識整理: …」支援 task を新たに作らない。既に作成された
   `knowledge_runs` の `apply_finished` と `retry_failed` は残し、進行中の run と一度だけの再試行を
   完了させる。`reports.rs` の「報告のまとめ」は従来どおり残す。これは D6 の「削減は範囲外」を
   改める決定であり、日次整理は task ごとの候補抽出も引き受ける。
2. **入力は daemon が決定的に準備する。** 日次整理 task の run ごとに、worker の作業場所の
   `inputs/kb/` に本番 KB の読み取り用の写し、`inputs/inbox.json` に `build_attention` の表示項目と
   `suppressed` の規則別件数、`inputs/reports.json` に前回の日次整理以降に終端になった task の
   報告の抜粋を置く。報告は task id と終端時刻で重複なく並べ、前回の基準は同じ job の直前の
   `created` run とし、初回は job 作成時刻からとする。日次整理 task 自身と報告のまとめなどの
   支援 task は除外する。`inbox.json` には古い報告と R1〜R4 に当たらない `failed` を判断候補として
   明示する。`inputs/` は run の入力であり、本番 KB の正本ではない。worker の出力は当該 task の
   `artifacts/curation-plan.json`、`artifacts/curation.diff`、`artifacts/daily-summary.md` に置く。
3. **mode は発火時に固定する。** `[[cron.seed.template]]` の `mode = "dry_run"` は
   `CronTaskTemplate.extra["mode"]` に入る。cron 作成・更新時に `dry_run|apply` だけを受け、
   省略時は `dry_run` とする。`template_to_spec` は `extra` を無視したままにせず、発火時の
   `mode` と job/run id を cron 由来の task のイベントにスナップショットとして保存する。
   daemon は保存された値を run の入力 manifest に書き、worker と終端処理はその値を読む。
   後の job PATCH が既存 task の mode を変えない。`Task.mode`（prototype 等）とは別の値である。
4. **版付きの計画を検証する。** `curation-plan.json` は `version: 1` と `kb`、`inbox`、
   `human_decisions` を必須とする。`kb` の各件は `{path, action, target, reason}` で、
   `action` は `merge|new|delete|keep|fix`、`target` は移動・統合先があるときだけ指定する。
   `inbox` の各件は `{task_id, proposal, reason}`、`human_decisions` の各件は
   `{subject, proposal, reason}`（`subject` は KB path または task id）とする。`target` が不要な
   action では `null` とし、`merge` では統合先の KB path を必須とする。人への候補は報告中の
   1 件の decision に束ねる。
   `task_ops::knowledge_curation` は版・型、入力に存在する path/task id、重複操作、対象の
   content hash、差分と計画の一致を適用前に検証する。KB 根の外、絶対 path、`..`、symlink 経由の
   脱出、`_curation/` への worker 由来の書き込みは拒否する。frontmatter が `source: human` の
   ページと `user/` 配下の削除・大幅書き換えは適用せず、`human_decisions` へまとめる。
   `inbox` の提案は DB の状態を直接変えず、人が既読化・cancel 等を選ぶ材料に限る。
5. **反映は承認した計画に限る。** `dry_run` は本番 KB を書かず、`curation.diff` と
   `daily-summary.md` を出す。`apply` でも worker は写しだけを編集する。daemon は検証済み計画と
   差分のハッシュを報告に添え、人が当該 task の decision でそのハッシュを明示して承認した後に
   だけ、同じ計画を再検証して本番 KB に決定的に適用する。未承認・ハッシュ不一致・元ページの
   変更時は適用せず、再度 dry-run を要する。承認は保留された `human_decisions` の個別案件を
   自動承認しない。削除は archive せず、daemon が `_curation/YYYY-MM-DD.md` に path・理由を
   記録する。反映後に `index.json` と KB の README を再生成する。本番で job の有効化・
   `mode = apply` への変更は人の操作とする。
6. **要約は task の報告 1 件に集約する。** `daily-summary.md` から統合・新規・削除・保留の件数、
   D7 の `suppressed`、人に残る判断件数を短く示す。承認待ちの apply は実施済みと書かず提案数と
   して示す。この task には「報告のまとめ」支援 task を作らず、日次整理専用の追加通知も作らない。
   人が受け取るのは日次整理 task の既存の報告 1 件だけとする。
7. **責務を分ける。** `task_ops::knowledge_curation` が版付き計画の型・検証・差分生成・
   決定的適用を持つ。`crates/celeris/src/knowledge_curation.rs` が入力準備と終端時の検証・
   承認待ち・適用・要約を扱う。`config` の `[[cron.seed]]` が初期 job を定義し、
   `harness = "knowledge-curation"`、`lane = "cheap"`、`overlap = "skip"`、
   `enabled = false`、`mode = "dry_run"` を初期値とする。試験は注入した時計と偽 worker を使い、
   外部ネットワークへ出ない。

## 付記 D11（2026-10-03）日次整理の review 指摘への対応

この付記は D10 (3)〜(5) の計画項目、mode 検証、差分照合、人の decision に関する矛盾箇所を上書きする。
D1〜D10 の本文は変更しない。

1. **KB 操作の本文と変更前 hash を計画に含める。** `curation-plan.json` の `kb` の各件は
   `{path, action, target, reason, content, expected_hash, target_hash}` の 7 項目とする。
   `content` は `new` / `fix` では新しいページの本文、`merge` では統合先の完成した本文であり、
   `delete` / `keep` では `null`。`expected_hash` は `path` の変更前 SHA-256 を 16 進小文字で表し、
   `new` だけは `null` とする。`target_hash` は `merge` の統合先の変更前 SHA-256 であり、
   それ以外では `null`。変更前 hash は `inputs/kb/` に置いた写しのページの生バイトから求め、
   文字列の正規化や frontmatter の再生成はしない。欠けた hash や hash 不一致は計画全体を拒否する。
   型は `crates/task-ops/src/knowledge_curation.rs` の `KbAction`、hash 算出は同じ module の
   `content_hash` を正とする。
2. **人への候補を独立した decision に束ねる。** 検証後の `human_decisions` が 1 件以上なら、
   `dry_run` / `apply` のいずれでも日次整理 task に key `curation-human` の
   `DecisionRequested` を 1 件だけ出す。質問文には各候補の `subject` と `proposal` を並べ、
   選択肢は「次回の整理まで保留」「人が手で対応する」等とする。回答は判断の記録にとどめ、
   daemon はそれを根拠に KB を変更しない。`apply` 用の承認（key `curation-apply`）とは別件であり、
   `curation-apply` の承認対象には `human_decisions` を含めない。D8 の要約には人に残る判断件数を
   示すが、要約の記述だけで decision の発行を代用しない。
3. **mode は登録・更新時に検証する。** `[[cron.seed]]` の読み込みと、
   `task_ops::cron_jobs::validate_job` を通る cron 作成・PATCH の双方で、
   `CronTaskTemplate.extra["mode"]` は文字列 `dry_run` または `apply` だけを受ける。
   文字列以外の値やその他の文字列は `Validation` エラーとし、API と celerisctl の
   作成・更新にも同じ規則を適用する。省略した場合は `dry_run` とする。
4. **worker の差分を計画と照合し、正本を生成する。** worker が `curation.diff` を出した場合、
   unified diff の `---` / `+++` 見出しから触れた KB path の集合を取り、検証済み計画が変更する
   path の集合（`merge` の統合元と統合先を含み、`keep` や人の判断に移した操作を除く）と
   `task_ops::knowledge_curation` で照合する。不一致なら計画全体を拒否する。worker の差分は
   `curation.worker.diff` に移し、daemon が検証済み計画から生成した差分で正本の
   `curation.diff` を上書きする。`curation-apply` の承認 hash はこの正本の差分から計算する。
   worker の `curation.diff` が無ければ照合を省略し、daemon が正本の差分を生成する。

## 付記 D12（2026-10-03）計画の形の事前検証と `_inbox` の 1 回あたりの上限

2026-10-03 の本番初回 dry-run（task `01M410C9X7TGMRBJ6NYXDSW07P`）で、worker は `curation-plan.json` を独自の形
（最上位に `task_id`・`date`・`mode`・`operations`・`manual_review` …）で出し、reviewer も通したが、daemon の
`task_ops::knowledge_curation::CurationPlan`（`deny_unknown_fields`）は `unknown field task_id` で計画全体を
拒否し、何も使われなかった。原因は (1) harness の instructions が `kb` 1 件の欄しか示さず計画全体の形と例を
与えていない、(2) worker が出す前に daemon と同じ検証を走らせる手段が無い、(3) acceptance が
`artifact_exists` だけで形を見ない、の 3 点。また `_inbox` には候補が 283 件あり、`max_turns = 30` の cheap
worker が 1 回で全件を扱える量ではない。この付記は D10 (4)・D11 (1) の計画の形は変えず、次を決める。

1. **検証の入口を worker に渡す。** `celerisctl curation validate [PLAN] [KB] [--diff] [--inbox] [--no-diff] [--json]`
   を追加する（`crates/celerisctl/src/commands/curation.rs`）。DB を開かず、ネットワークにも出ず、ファイルだけを
   読む。検証は daemon の `check_plan` と同じ順・同じ関数（`parse_plan` → `validate_with_inbox`（`inputs/inbox.json`
   の `candidates[].task_id` と `attention[].task.id` を既知の id とする）→ `check_diff_matches`）で、拒否の文言も
   同じにする（`parse_plan` は `task_ops::knowledge_curation` に 1 つだけ置き、daemon もそれを呼ぶ）。引数を
   省略したときは cwd から上へ `artifacts/curation-plan.json` と `inputs/kb` を持つ作業場所を探す（検査コマンドの
   cwd は作業場所の `repos/<name>` なので、`celerisctl curation validate` だけで動く）。成功は exit 0 と件数 1 行、
   失敗は stderr に `error: <理由>` と exit 1。
2. **形は instructions と ops 手順に書き、acceptance で決定的に判定する。** harness `knowledge-curation` の
   `instructions` と `[[cron.seed]]` の `objective`（`config/celeris.example.toml`）、`docs/ops/cron-jobs.md` に、
   最上位 4 欄（`version`・`kb`・`inbox`・`human_decisions`）だけという規則・最小の計画
   `{"version":1,"kb":[],"inbox":[],"human_decisions":[]}`・`_inbox` 候補の merge/delete の例・出す前に
   `celerisctl curation validate` を走らせることを書く。cron の雛形の `acceptance` に
   `{ type = "command", cmd = "celerisctl curation validate", expect_exit = 0 }` を足し、形の判定は reviewer の
   自己申告ではなくこの検査が決める。この検査は daemon の PATH の `celerisctl` がこの付記以降の release である
   ことを前提にする（古い release の `celerisctl` には `curation` が無く、検査は非 0 になる）。
3. **`_inbox` 候補は 1 回の run で 40 件まで。** `task_ops::knowledge_curation::MAX_INBOX_CANDIDATES_PER_RUN = 40`。
   計画の `kb` のうち `path` が `_inbox/` で始まる件数がこれを超えたら、daemon と `celerisctl curation validate` は
   計画全体を拒否する（`keep` も数える。`_inbox/` 以外の重複統合・古い記述の修正は数えない）。40 の根拠は
   `max_turns = 30` の cheap worker が候補ごとに本文を読み、統合先の完成本文と変更前 hash を計画に書く
   作業量と、`content` を含む計画 JSON の大きさ。
4. **残りは次回へ回す（持ち越しの規則）。** worker は `inputs/kb/_inbox` をファイル名の昇順（`YYYYMMDDTHHMMSSZ-` の
   時刻接頭辞なので古い順）に並べ、先頭 40 件だけを扱う。扱わなかった候補は計画に載せず、写しも触らない。
   `apply` では扱った候補が本番 `_inbox` から消えるので、次の run は自然に次の 40 件を見る。`dry_run` のあいだは
   本番 `_inbox` が減らないため毎回同じ先頭 40 件が対象になる（dry-run の差分を人が確かめる用途ではそれでよい。
   283 件は `apply` 後の約 7 run で消化する）。`daily-summary.md` の KB 節に「処理 n / 残り m」を書く。
   上限の引き上げは、`apply` の実績で 1 run の所要 turn と拒否率を見てから設定値にするかを決める（今は定数）。
5. **変えないこと。** D10 (4)・D11 (1) の `CurationPlan` の形と `deny_unknown_fields`、daemon の検証順、
   「LLM 呼び出しを daemon・store に入れない」「本番 KB は worker が書かない」はそのまま。

## 付記（main 取り込み 2026-10-03）

main の ADR-0128 D6 に合わせ、本文を `agent-docs/adr/0131-cron-jobs.md` へ移した。番号は維持する。
main の `agent-docs/adr/0133-inbox-and-notifications.md` がこの ADR を「ADR-0131 D7」として参照し、
crates の doc comment と生成 schema も ADR-0131 を参照しているためである。全 ref の `git for-each-ref`
と `git ls-tree` を走査し、0131 を別名の ADR に使う競合がないことを確認した。

main 取り込み後に migration の番号を `0046_cron_jobs` とした。main の `0041_feed_notices` と、並行作業で
確保済みの 0042〜0045 の後ろの空き番号を使う。
