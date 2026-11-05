# ADR 2026-11-05: 第 4 フェーズ P4-D — Project の wake・判断・compare の記録の保持と GC

- 状態: 採用。
- 根拠: `agent-docs/progress/2026-11-05-phase4/p4d.md`、raw は `agent-docs/progress/2026-11-05-phase4/p4d/`。前提は
  [P4-A](2026-11-05-phase4-p4a-baseline-contract.md) の G5（migration 0117 の予約）と [ADR 2026-11-04-project-controller](2026-11-04-project-controller.md) の D12・A1。

## 文脈
`project_wakes`・`project_controller_decisions`・`script_compare_log` は毎時の cron の照合で増え、消す仕組みが無かった。
判断の表と compare の記録は trigger で DELETE を拒む追記専用で、guard（同じ失敗の数え・TTL・再開の回数）と画面の要約は
`project_wakes_all`・`project_controller_decisions(…, 100000)` で案件の全部の行を毎回読む。

## 決定
- D1 events は正本のまま触らない。消すのは派生の行のうち次だけ:
  消費済みの `cron` の wake（`wake_keep_secs` より古い）、効果 `none` の判断（`decision_keep_secs` より古く、案件ごとの新しい
  `decision_keep_latest` 件の外）、TS が適用していない compare の記録（`compare_keep_secs` より古い）。
- D2 必ず残す: 未消費の wake、`cron` 以外の wake、pending・applied・suppressed・failed の判断、案件の最後の判断、案件の最初の判断
  （TTL の起点の代わり）、`pc/resume/%`・`pc/authority-required` の判断、TS が適用した compare の記録。新しい 256 件は guard と
  再開が読む窓（`project_controller_decisions(…, 256)`）と同じで、窓の中の答えは GC で変わらない。
- D3 監査: 消す行は先に archive の JSONL に書いて fsync し、`project_retention_runs`（追記専用）に件数・cutoff・path・sha256 を残す。
  archive の file は `create_new` で作り、同じ時刻の pass や並行の GC が上書きしない。
- D4 二重の守り: GC は同じ transaction の中で `project_retention_permit` に 1 行入れてから消し、commit の前に消す。判断と compare の
  記録の trigger は、許可の行が無いとき・D2 の行のときに DELETE を拒む。wake の表は P4-B の migration 0115 が作り直す（trigger が
  消える）ので trigger を置かず、GC の DELETE 文の条件（`consumed_by IS NOT NULL AND kind = 'cron'`）で守る。
- D5 crash・並行: archive の後・commit の前に落ちても行は残り、次の GC がやり直す（記録の無い archive の file が残るだけ）。
  並行の GC・controller の書き込みとは書き込みの lock と DELETE 文の条件の付け直しで両立する（先に消された行は `deleted` に数えない）。
- D6 起動: `[project_controller.retention]`（既定は無効。`enabled = true` なら絶対 path の `archive_dir` が必須）。dispatcher の
  `tick_project_retention` が `interval_secs` ごとに 1 回（process の中で数える。再起動の後は最初の tick で 1 回）、表ごとに
  `max_rows_per_pass` 行まで消す。`VACUUM` はしない（file を縮めるのは人の操作）。
- D7 可視化: `GET /api/v1/projects/{id}/controller` の `summary.retention`（案件の行数）と画面の `controller-retention`。
  表全体の数は `SqliteStore::retention_stats`（試験・計測が使う）。
- D8 migration は 0117。0115（P4-B）・0116（未使用）は `RESERVED_VERSIONS` に入れ、取り込みで本物が入ったら外す。

## 結果
- 30 日分・100 万行（694 案件）の計測で、wake 582,960 → 199,872 行、判断 582,960 → 235,960 行、効果のある判断と最後の判断は全部残った。
- 1 評価の store の読みは 3.1 → 1.9 ms、GC の 1 回は中央値 0.5〜1.1 秒（host の負荷で振れた）。
- 未解決: archive の置き場の容量の管理と古い archive の削除は人の運用（自動化しない）。
