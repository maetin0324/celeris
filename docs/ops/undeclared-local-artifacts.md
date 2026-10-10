# Local workspace の成果物を補完する

既存 task の local workspace にある成果物が一覧に出ない場合は、配送後の `celerisctl` と稼働 daemon と同じ設定・DB を使って登録する。本番操作は運用セッションが行う。以下の DB/config のパスは稼働環境に合わせる。

成果物の保存先は次のとおり。

- 自分の task ID の workspace: `<workspace>/artifacts/`
- 別 task ID の workspace を使う retry（`parent_id` なしでも）と親から workspace を継いだ子: `<workspace>/.taskd/artifacts/<task-id>/`
- task ID ではない任意名の workspace を使う親なしの単独 task: 従来どおり `<workspace>/artifacts/`

共有 workspace では task 専用ディレクトリだけを走査する。workspace 直下の Markdown、元 task の `artifacts/`、兄弟のディレクトリは登録しない。必ず `--task` を指定する（省略時は remote task が対象）。

## 旧配置の manaba レポートを移す

対象 retry は `01M4GYJ3XGJNWZQDF35F1MDE0H`、共有 workspace は `/local/celeris/data/workspaces/01M4FPA6ADFH639XYP894ES87J`。修正前は retry も元 task と同じ `artifacts/` に書いていた。修正後の backfill はこの旧ディレクトリを探索しないため、**retry が作成したことを run ログ・内容で確認したファイルだけ**を専用ディレクトリへコピーしてから登録する。時刻やファイル名だけでは作成 task を判別できない。元 task のファイルが上書きされていた場合、その復元はこの手順ではできない。

運用調査で挙がった候補は `assignments.md`、`summary.md`、`reports/*.md` 28 件。まず次の読み取りで一覧を確認し、各ファイルを retry の出力と照合する。帰属が不明なファイルは移さず確認を止める。

```sh
ws=/local/celeris/data/workspaces/01M4FPA6ADFH639XYP894ES87J
find "$ws/artifacts" -maxdepth 2 -type f -name '*.md' -print
```

上記 30 件の帰属確認後に運用セッションで実行するコピー手順（元ファイルは残す）。対象の件数・サイズ・symlink・既存コピーの内容を確認し、異なる内容を上書きしない。`result.json`・`checkpoint.json` や browser 抽出 JSON はコピーしない。

```sh
python3 - <<'PY_COPY'
from pathlib import Path

ws = Path('/local/celeris/data/workspaces/01M4FPA6ADFH639XYP894ES87J')
src = ws / 'artifacts'
dst = ws / '.taskd/artifacts/01M4GYJ3XGJNWZQDF35F1MDE0H'
reports = sorted((src / 'reports').glob('*.md'))
assert len(reports) == 28, '候補が変わったため帰属を再確認する'
files = [src / 'assignments.md', src / 'summary.md', *reports]
copies = []
for source in files:
    assert source.is_file() and not source.is_symlink(), source
    assert source.resolve().is_relative_to(src.resolve()), source
    data = source.read_bytes()
    assert 0 < len(data) <= 1024 * 1024, source
    target = dst / source.relative_to(src)
    assert not target.is_symlink(), target
    if target.exists():
        assert target.read_bytes() == data, target
    copies.append((target, data))
for target, data in copies:
    target.parent.mkdir(parents=True, exist_ok=True)
    if not target.exists():
        with target.open('xb') as output:
            output.write(data)
print(f'{len(copies)} 件の専用ディレクトリへのコピーを確認: {dst}')
PY_COPY
```

## dry-run・登録・確認

```sh
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts \
  --config ~/.config/celeris/config.toml \
  --task 01M4GYJ3XGJNWZQDF35F1MDE0H --dry-run
celerisctl --db ~/.local/celeris/celeris.sqlite3 workspace backfill-artifacts \
  --config ~/.config/celeris/config.toml \
  --task 01M4GYJ3XGJNWZQDF35F1MDE0H
```

`--dry-run` で `.taskd/artifacts/01M4GYJ3XGJNWZQDF35F1MDE0H/` 配下の `assignments.md`、`summary.md`、`reports/*.md` だけが候補になることを確認してから本登録する。走査は D3-c の上限（最大 64 件、相対深さ 4、1 MiB/ファイル）を適用する。`checkpoint.json` や `result.json` などの管理用ファイル、すでに同じ path と SHA-256 で登録済みの成果物は対象外。

登録後は認証済み API クライアントで `GET /api/v1/tasks/01M4GYJ3XGJNWZQDF35F1MDE0H/artifacts` を取得し、追加された 30 件の `artifact.path`・`exists: true`・`sha256_matches: true` を確認する。既存の declared な browser 抽出物は一覧に残る。web の当該 task の成果物欄からレポートを開き、本文を確認する。もう一度 dry-run して追加候補 0 件になることも確認する。元 task の一覧には今回のコピー先の成果物が追加されないことを照合する。

旧走査で既に誤登録されたイベントがある場合、この修正はその履歴を削除しない。運用結果（実行日時・版・dry-run 件数・登録件数・GET/web 確認・帰属確認の根拠）は進捗ファイルに記録する。
