# cos-operator: 登録 repo の変更と本番運用

登録 repo のコードを変える、または release・verify・promote など本番運用に関わる依頼のときに読む。

## 登録 repo の変更

- 登録 repo のコードを変えるときは、**専用の worktree**（CoS の作業 dir の下に `git worktree add` するか、
  task を起票して通常 worker に任せる）で行う。元 repo や別 worker の作業場所を直接変更しない。
- `main` に直接 commit しない。`git checkout` で他人の作業ツリーのブランチを変えない。push は人の認可がある範囲だけ。
- 大きな実装は自分でやらず起票して worker に任せ、CoS は依頼の整理・判断・監視に回る方がよい。

## 本番運用（release → verify → promote）

- 手順の正本は [docs/ops/selfdeploy.md](../../../docs/ops/selfdeploy.md)。台本は `scripts/selfdeploy/`
  （[release.sh](../../../scripts/selfdeploy/release.sh)、[verify.sh](../../../scripts/selfdeploy/verify.sh)、
  [status.sh](../../../scripts/selfdeploy/status.sh)、[promote.sh](../../../scripts/selfdeploy/promote.sh)、
  [rollback.sh](../../../scripts/selfdeploy/rollback.sh)）。
- 順序は必ず **検査 → promote**: `release.sh` でリリースを作り、`verify.sh` で検査し、`status.sh` で
  `verify.json.ok` と `live_ok` を確かめてから昇格する。検査を飛ばす・`ok` が偽のまま進めることはしない。
- selfdeploy.md §4「昇格する（人だけ）」・§5 rollback・§5b relocate-db・§7「禁止」は、人が人だけに限定した本番操作である。
  `promote.sh`・`rollback.sh`・`install-units.sh` の実行、`POST /releases/{sha12}/promote`、`systemctl`、
  本番 config の編集は **CoS もしない**。必要なら運用者向け task を起票し、コマンドと確認方法を「運用者の作業」に分ける。
  人には web でできる判断・承認だけを依頼する（[explaining.md](explaining.md)）。人だけの実行認可は task 起票で解除されない。
- その他の運用手順は [docs/ops/](../../../docs/ops/) の各文書（例: 受信箱・通知の設定は
  [inbox-notifications.md](../../../docs/ops/inbox-notifications.md)）に従う。手順書に「人が実行」とあるものも
  実行制約を守り、運用者の作業として分ける。
