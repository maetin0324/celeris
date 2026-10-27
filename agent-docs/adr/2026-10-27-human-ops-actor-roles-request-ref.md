# human-ops /2: 主体の種類・役割・credential・論理要求の参照と、主体別・論理要求単位の集計

- 日付: 2026-10-27
- 状態: 採用（実装済み: `crates/campaign/src/human_ops.rs`・`human_ops_report.rs`、定義書 `docs/ops/eval-raw-record-definitions.md`）
- tasks: [01M5XXF7Q8133JDMR051R4MHJX]
- 関連: [2026-10-19-eval-raw-human-ops-record](2026-10-19-eval-raw-human-ops-record.md)（D2・D9 の schema 版上げ）、
  [2026-10-27-external-operator](2026-10-27-external-operator.md)（判断・実行・確認の分担）、
  [2026-10-06-cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（source triple）

## 背景

human-ops /1 の `actor.kind` は `human` / `script` だけで、CLI は `--actor-kind` を省くと `human` を入れた。
CoS（案件・統括）・外部 operator・MCP client の操作を人の操作と区別できず、判断した者・実行した者・承認した者も分けられない。
論理要求（同じ問いの再通知・改訂）への参照も無く、report は再通知や改訂を別の介入として数え得た。
本番の `cos_inbox_items` は `UNIQUE(source_kind, source_key, source_revision)` で、notice 963 item に対し論理キーは 56 だった。

## 決定

- D1（schema 版）: `schema = "celeris.eval-human-op/2"`。/1 の行は読み込みで拒否する（互換分岐を置かない）。
  /2 の新しい欄はすべての行で JSON に存在しなければならない（使わない型では null）。欄の無い行は形式エラー。
- D2（主体の種類の語彙。これを Celeris の actor_kind の唯一の語彙とする）: `actor.kind` は次の 7 種の閉じた列挙。
  | kind | 意味 |
  |---|---|
  | `human` | 人（評価者・利用者）が自分で行った |
  | `script` | 評価者が起動した script・cron |
  | `cos_project` | 案件の CoS（案件の固定スレッドの CoS run） |
  | `cos_general` | 統括の CoS |
  | `external_operator` | Celeris の外の operator（Claude Code 等の外部 session。ADR 2026-10-27-external-operator） |
  | `mcp_client` | MCP client（ChatGPT RDC 等） |
  | `unknown` | 主体を確かめられない。推測で `human` に寄せない |
  外部 operator の principal（起票元の A1/A3）を作るときはこの語彙を使い、別の語彙を作らない。
- D3（必須化）: CLI の `--actor-kind` は必須。既定の `human` を消す。
- D4（役割）: op 行に `decided_by`・`executed_by`・`approved_by`（各 `{kind, id}` | null）を持つ。`actor` は記録の主体
  （その op を行った者）のまま。役割が `actor` と同じ・分からないときは null。id にメールを書かない規則は `actor` と同じ。
- D5（credential）: op 行に `credential`（必須）。値は `admin_token` / `web` / `operator:<name>` / `mcp:<id>` / `cos_run` /
  `none` / `unknown` のどれか。token の値は書かない（種類と名前だけ）。CLI の既定は `unknown`（推測で埋めない）。
  主体の種類との組み合わせは検証しない（CoS が admin_token を使った等の不整合を記録として残すため）。
- D6（論理要求）: op 行と wait 行に `request_ref`（`<source_kind>:<source_key>`、両方非空）と `request_revision`
  （文字列。`cos_inbox_items.source_revision` が TEXT のため）を持つ。両方あるか両方 null。
  `request_ref`・`request_revision` は record_id の材料に入れる（同じ論理作業・attempt の別 revision が衝突しない）。
  訂正（correction）では変えない（直すなら取り消して書き直す）。
- D7（方針の参照）: op 行に `policy_ref`（string | null）。適用した方針・既存承認への参照（例 `adr:2026-10-27-external-operator`、
  `approval:<id>`）。
- D8（集計: 論理要求単位）: report の介入は `request_ref` で 1 件にまとめる（`request_ref` の無い op は 1 op = 1 件）。
  改訂・再通知・差戻し（同じ `request_ref` の 2 件目以降の op）は件数を増やさず、その介入の `history` に時刻順に並べる
  （各要素に `revision` と `event`: `initial` / `revision`（revision が変わった）/ `repeat`（同じ revision の再対応・差戻し））。
  介入の区分（op_kind）は最初の op の種別。
- D9（集計: 主体別）: 介入（protocol を除く）を主体の種類別に数える（`interventions_by_actor_kind`。1 件の介入に複数の種類が
  関われば各種類に 1 ずつ数えるので合計は介入数を超え得る）。`interventions`（人の介入）は `actor.kind=human` の op を含む
  論理要求だけ。`actor.kind=unknown` の op は人にも 0 にも寄せず `unknown_actor`（件数と区間）に別掲する。
- D10（人の能動時間）: 人の時間は `actor.kind=human` かつ `mode=manual` の op 区間の和集合だけで数える。
  質問・承認が開いていた時間（human_wait・Celeris の待ち）では代用しない。CoS・外部 operator・MCP client・script の op は
  人の時間・介入・再作業に入れない（`excluded_ops.non_human` に主体の種類別の件数で残す）。

## 移行

/1 の `human-ops.jsonl` は新しい版では読めない。file は消さず残す。論文初版の本測定は /2 で記録を始める。
/1 の記録を /2 へ移す変換は作らない（/1 の `human` は既定値で入った可能性があり、主体を機械的に決められないため）。
