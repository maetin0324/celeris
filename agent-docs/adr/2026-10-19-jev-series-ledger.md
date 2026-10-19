# Jev 副比較 B1/C1: 測定 ID ごとに固定した系列台帳と、整合判定と live 判定の分離

tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]

## 決定
- 旧 `budget::series_prior_spend`（`out.parent()` の兄弟 out を走査して既消費を合算）を**削除**し、`crates/campaign/src/series.rs` に置き換える。出力先を親ごと変えると既消費が 0 になる抜け穴が、通常実行器の不具合だった。
- 系列台帳は測定 ID ごとに 1 か所: `<SERIES_ROOT>/<measurement_id>/{series.json, series-ledger.jsonl, series.lock, runs/}`。位置は launcher.env（`SERIES_ROOT`・`MEASUREMENT_ID`）から `--series-root` / `--measurement-id` として CLI に渡り、副比較の campaign lock の `series` 欄（`SeriesPin`）に封印される。`series-init`（launcher の `prepare` が実行、冪等）だけが台帳を作る。
- 台帳は追記専用の out 単位 snapshot（確定消費+予約）。同じ out は新しい行が勝つので再開・重複で二重計上しない。他 out は snapshot と、その out が読めるときの実測の大きい方を使う（crash で snapshot が遅れても予約を落とさない）。manifest hash・版・OUT を変えても合算される。
- 出力先は `runs/` の下だけ。`..`・symlink・root 外・root 自身は、副作用（out 作成・executor・台帳追記）の前に拒否する。`SERIES_ROOT` 未設定・台帳欠損・破損・読取不能・別測定 ID は拒否（既消費 0 にしない）。同じ系列の同時 run は排他 lock で拒否（二重投入で枠を超えない）。
- 既存消費の引継ぎは `series-init --import-out`（env の `SERIES_IMPORT_OUTS`）で明示する。主比較（side でない manifest）は台帳を使わず、計上は不変。承認総枠は増えない。
- `jev-live-evidence.py`（`--require-live` の名は維持）は原記録との**整合**だけを確かめる。出力は「整合 OK（live かどうかは未判定）」で、`live_proven` は `chain_ok`（Rust の `jev_smoke` / `jev_stages` も同名へ）に改めた。live の判定は operator の承認項目（`docs/ops/jev-c1-live-check.md` §7: run/ の証跡と承認記録）。`scripts/dev/jev-live-approval-check.py` は証跡の実在確認だけで、欠ければ非 0。

## 影響
旧 `series_prior_spend`・「同じ親 dir の兄弟だけ合算」の記述・`live_proven` の名前と「live 合格」の文言を消した（互換は残さない）。side manifest の run は `RunOptions.series` が必須。
