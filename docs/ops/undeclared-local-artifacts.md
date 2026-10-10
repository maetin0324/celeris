# Local workspace の成果物を補完する

既存 task の local workspace で `artifacts/`（共有 workspace では `.taskd/artifacts/<task-id>/`）にある成果物が一覧に出ない場合は、daemon の設定と DB を使って登録する。作業場所の写しが残っていることを先に確認する。

```sh
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts \
  --config ~/.config/celeris/config.toml \
  --task 01M4GYJ3XGJNWZQDF35F1MDE0H --dry-run
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts \
  --config ~/.config/celeris/config.toml \
  --task 01M4GYJ3XGJNWZQDF35F1MDE0H
```

`--dry-run` の一覧で `assignments.md`、`summary.md`、`reports/*.md` を確認してから本登録する。走査は D3-c の上限（最大 64 件、相対深さ 4、1 MiB/ファイル）を適用する。`checkpoint.json` や `result.json` などの管理用ファイル、すでに同じ path と SHA-256 で登録済みの成果物は対象外。登録後は `GET /tasks/01M4GYJ3XGJNWZQDF35F1MDE0H/artifacts` で一覧を確認する。

この manaba task は再実行 task が元 task の workspace を共有している。必ず `--task` を指定し、workspace 全体を別 task として backfill しない。成果物ディレクトリは task ごとに分かれているため、指定 task 自身の分だけが登録される。
