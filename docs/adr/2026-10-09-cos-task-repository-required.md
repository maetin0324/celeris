# ADR 2026-10-09: CoS 起票のリポジトリ必須検査と task への後付け

---
tasks: [01M4F5MBRPYGEK75001AMJKZ6Z]
---

状態: 採用。実装と検証は同名の進捗ファイルに記録する。

## 背景

CoS が案件・リポジトリ無しで coding task を起票し、空の workspace で worker や子 task が停止した。
既存の案件後付け API は未実行の親 task だけを受け付けるため、blocked の子を救済できなかった。

## 決定

### D1: CoS の起票

`/cos/operations` の `task.create` は、解決後の genre が `coding`、または objective・acceptance の文と
command check にリポジトリのコマンド・path の字句が含まれる場合、解決後の `project_id` と非空 `repos` を必須とする。
不足は 422 `repository_required`、理由は「リポジトリを使う task は project_id と repos を付けて起票する」。
拒否の監査を保存し task は作らない。親・案件 primary の既存の解決規則は維持する。
明示 workspace（local / remote / shared）も免除しない。古い会話 actions の起票にも同じ検査を適用する。

判定は task-ops の共通関数で行い、dispatcher・store に LLM を入れない。
コマンドは `cargo`・`pnpm`・`npm`・`npx`・`yarn`・`git`・`make`・`rustc`・`pytest`・`nextest`・`clippy` の語一致、
path は `src/`・`crates/`・`scripts/` 等のディレクトリ、ソース拡張子、ビルド設定ファイルを照合する。
自然文の意味理解ではないため検出に限界がある。coding は字句によらず必須とし、CoS skill にはすべての
リポジトリ作業で案件と repo 名を明示する規則を置く。

### D2: 人の POST

人の `POST /tasks` は互換性と案件未整理の起票を保つため、同じ判定で `celeris-warning` 応答ヘッダを返す。
従来の status の既定（ready）を維持する。後付けが必要なら人は `status: draft` で作成してから PATCH する。
警告は自動停止ではない。CoS credential の直接 POST は従来どおり禁止される。

### D3: 後付け

`PATCH /tasks/{id}` の既存 `project_id` を、未実行の draft/ready または lease の無い blocked に受け付ける。
子 task も対象とし、親が案件を持つ場合は同じ案件だけを許す。既存案件の変更・削除は引き続き拒否する。
案件付与は保存 transaction 内でも状態・親の案件を再確認し、dispatch との競合を拒否する。
同じ PATCH の `repos` は指定案件内で解決する。省略時は同じ案件の親の repos、次に案件 primary を採用する。
無効な repo の指定は task を変えない。状態は変えず、blocked の解除は既存の回答・再開操作で行う。
次の run は保存した task と repo 登録を再読込し、通常の worktree 準備を行う。過去の run の workspace は消さない。
既存の子 task への一括伝播はしない。それぞれ停止中に後付けし、新たな子には D4 を適用する。

### D4: 計画の子 task

task unit から生成する子には親の project_id と repos を継承する。unit に repo 名があれば親の部分集合に限る。
既存の継承実装を回帰試験で固定する。

### D5: skill と適用

`config/skills/cos-operator` の起票節に規則と例を追加する。稼働 KB が旧版の場合にも適用できる差分を成果物に残す。
本番 KB・DB・daemon はこの task では変更しない。KB への取り込みは運用者が行う。

## 検証

外部ネットワーク・LLM を使わない API 試験で拒否と監査、正常起票、PATCH の原子性と状態制限を確かめる。
一時ローカル git repo を登録し、後付け後の通常の workspace 準備で追跡ファイルが見えることを確かめる。
全体検査は `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings`。
