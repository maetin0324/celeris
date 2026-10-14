---
title: "クラスタ画面のローカルマシン行と store 経由のローカル設定（workspace_root・max_concurrency）"
tasks: [01M4X1AKJ2CG1F7S5YTMYFJ3B0]
status: running
updated: 2026-10-14
---
# クラスタ画面のローカルマシン設定

設計判断は [ADR 2026-10-14-local-machine-settings](../adr/2026-10-14-local-machine-settings.md)（D1〜D5）。

## 進み具合

- [x] adr: 範囲・store の表・優先順位・API・既存 task／run への影響を ADR に決めた（コードは未変更）
- [ ] backend: migration 0078、store、`GET /clusters` の `local`、`PUT /clusters/local/settings`、resolver、dispatcher 反映
- [ ] web: クラスタ画面のローカルマシン行と設定フォーム
- [ ] closeout: 全体検査・本節の更新

## 範囲

画面から変える: `workspace_root`（必須）、`max_concurrency`。表示のみ: `scratch_root`、各欄の設定ファイル値。
変えない: `[scratch]`・`[maintenance]`・`db`・`[api]`・`[selfdeploy]`・`[storage]`・tick 等の内部調整値（理由は ADR D1）。

## 優先順位

欄ごとに上から最初に値があるもの（ADR D3）:

1. CLI の `--workspace-root`（verify.sh の staging）。あるときは store の値と task ごとの固定を**どちらも無視**する
   （staging の DB は本番の写しで、固定は本番の path を指すため）。
2. store（`local_settings` の欄が非 NULL）。
3. 設定ファイル（無ければ既定値）。

応答の各欄の `source` は `cli` / `settings` / `config`。`reload` は設定ファイル値を読み直すが store の値が勝つ。

## 反映タイミング

- `workspace_root`: PUT で即時に store へ入り、`apply: "new_tasks"`。この後に**足回りを初めて用意する task から**効く。
  daemon の再起動は要らない。
- `max_concurrency`: `apply: "next_tick"`。API が admin チャネルで dispatcher に知らせ、次の tick から効く。
  下げても走行中の run は止めず、超過分が終わるのを待つ。
- 画面は各欄の横に `apply` を「新しい task から」「次の tick から」と出す。

## 既存 task／run への影響

- 足回りの root は task ごとに `task_workspace_roots` に固定する（初めて用意するときに 1 回 INSERT。子 task は親の
  固定を継ぐ）。全ての `workspace_root.join(task_id)` は resolver を通す。
- 既存 task（固定の無い task）は設定ファイルの root のまま。作業場所は**動かさない・写さない**。
- 走行中 run は自分の task の固定 root を使い続けるので、設定を変えても作業場所は壊れない。
- 掃除（workspace_prune・target_sweep）は task ごとの固定 root を見る。旧い root は既存 task が終わるまで使われる。
- path 検証（PUT 時、422）: 展開後に絶対 path・`..` なし、書き込める、本番 DB・releases・`~/.config/celeris`・
  設定ファイルの dir・`/`・`$HOME` と同じ／中／含むなら拒否（canonicalize 後に照合、db_guard と定義を共有）。

## 本番への移行

- migration 0078（`local_settings`・`task_workspace_roots`・`local_settings_events`）は起動時に走る。既存行の変換は無い
  （既存 task は固定なし = 設定ファイルの root で従来どおり）。
- 本番の config ファイルは**書き換えない**。ただし `[[clusters]] id = "local"` は読み込みエラーになる（予約 id）。
  本番 config に `id = "local"` のクラスタがあれば、昇格前に人が別の id へ書き換える（確認:
  `grep -n 'id *= *"local"' ~/.config/celeris/config.toml` が何も出さないこと）。
- 昇格後、画面で `workspace_root` を変えるときは、移動先が D5 の検証を通る（例 `/local/celeris/data/workspaces`）ことを
  先に確かめる。変更は新しい task からだけ効くので、旧い root は既存 task が終わるまで残す（手で消さない）。
- release 昇格だけで反映（daemon の再起動は昇格の handoff に含まれる）。

## 証拠

- adr 葉: 文書のみ。`git diff --name-only $CELERIS_WU_BASE` が ADR と本ファイルだけであることを確認。
- migration 番号: 全ブランチの `crates/task-core/migrations/` に 0078 以降が無いことを `git ls-tree` で確認。

## 未解決事項

- backend 葉は `workspace_root` 直参照（130 箇所余り、`crates/` 全体）を resolver に置き換える。write set は
  `crates/` 全体が要る。

## 提案

- なし
