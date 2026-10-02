---
tasks: [01M3XTAH6KYYXTWRTNC2D4MDTG]
---
# 本番手順: ui-ux 課への外部 agent skill 4 件の登録

対象: `frontend-design` / `shadcn` / `web-design` / `ui-ux-quality-gate`（出典・ライセンス・改変点は
`docs/adr/0122-ui-ux-external-skills.md` の表と `config/skills/<name>/SOURCE.md`、取得時の記録は
`docs/progress/ui-ux-skills.md` を参照）。

この手順は**人が本番 host（`~/.config/celeris` + `~/.local/celeris` で動く celeris）に対して実行する**。
エージェントの run から本番 API を叩かない（ADR-0095 付記 D-d）。前提として D1〜D4 を含む release（この
WorkUnit を含む統合）が昇格済みであること（`scripts/selfdeploy/release.sh` / `verify.sh`、
`projects/agent-platform/selfdeploy-release-verify-procedure.md`）。

以下は本番 `celeris` の admin トークン（`token_file` 設定済みの管理系呼び出し）を使う前提。`$CELERIS_API` は
本番 API のベース URL（例 `http://localhost:8080`）、`$TOKEN` は管理トークン。

## 0. 前提確認

```sh
# KB が init 済みか（celerisctl は本番の config.toml を使う）
celerisctl knowledge search ui-ux --config ~/.config/celeris/config.toml --json | head
```

`initialized: false` が返る、または KB 自体が無い場合は `celerisctl knowledge init` を先に行う運用（既存の
正規手順。本件のための新しい初期化ではない）。

## 1. 既存の同名 skill の有無を確認する（重複を作らない）

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/skills" | \
  jq '.items[] | select(.name | IN("frontend-design","shadcn","web-design","ui-ux-quality-gate"))'
```

- 4 件とも無ければ 2 節の新規取り込みへ進む。
- 同名が既にあれば、**新規作成はせず** 2 節の同じ手順（`PUT`/`skills import` は冪等）で上書き更新する。
  `GET /api/v1/skills/{name}` で現行の `skill_md` を見て、今回取り込む内容と無関係な改変（他の課が独自に
  足した節など）が無いか確認してから上書きする。疑わしければ止めて人に確認する。

## 2. 4 件を取り込む（KB の正本 `skills/<name>/SKILL.md` へ）

本番 host 上で、取り込みたい release のチェックアウト（`~/.local/celeris/releases/<sha>/`）から
`config/skills/` を使う。`celerisctl skills import` は **DB を開かず KB だけ**を読み書きするので daemon の
再起動は不要（`skills_context` は dispatch のたびに KB を読む）。

### 方法 A（推奨）: `celerisctl skills import`

```sh
cd ~/.local/celeris/releases/<sha>   # 昇格済み release の checkout
celerisctl skills import config/skills --config ~/.config/celeris/config.toml
```

- `config/skills` 配下の 4 ディレクトリ（`SKILL.md` を持つもの）を一括取り込みする。特定の 1 件だけ
  やり直したい場合は `--name <name>` を繰り返して絞る（例 `--name web-design`）。
- UTF-8 でない付属ファイル（`shadcn/assets/*.png`）は `skipped (binary)` と出て取り込まれない。これは
  想定どおり（ADR-0122 D2。アイコンは worker への指示に使われない）。
- 出力例: `imported frontend-design (2 attached file(s))` のように 1 件 1 行。4 行（または `--name` で絞った分）
  出ることを確認する。

### 方法 B（`celerisctl` が使えない環境向け）: `PUT /api/v1/skills/{name}`

`<name>` ごとに `config/skills/<name>/SKILL.md` と付属ファイル（`SOURCE.md` とライセンスの写しを含む、
UTF-8 のテキストファイルのみ）を JSON に組み立てて送る。

```sh
name=frontend-design
dir="config/skills/$name"
jq -n --rawfile skill_md "$dir/SKILL.md" \
  --arg f1 "SOURCE.md" --rawfile c1 "$dir/SOURCE.md" \
  --arg f2 "LICENSE.txt" --rawfile c2 "$dir/LICENSE.txt" \
  '{skill_md: $skill_md, files: [{path: $f1, content: $c1}, {path: $f2, content: $c2}]}' \
  | curl -s -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      --data-binary @- "$CELERIS_API/api/v1/skills/$name"
```

他の 3 件も同様に、その skill の全付属ファイル（`shadcn` はネストしたファイルが多いので方法 A を強く推奨）を
列挙して繰り返す。応答は `{"path": "skills/<name>/SKILL.md"}`。

### 取り込みの確認

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/skills" | \
  jq '.items[] | select(.name | IN("frontend-design","shadcn","web-design","ui-ux-quality-gate")) | {name, description, updated}'

curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/skills/ui-ux-quality-gate" | \
  jq '.skill_md' | grep -o 'celeris-use:.*'
```

- 4 件が `GET /api/v1/skills` の `items` に出ること。
- `ui-ux-quality-gate` の `skill_md` に `celeris-use: work, review` があること（D4。review run にも届く鍵）。

## 3. ui-ux 課へ mount する

```sh
for name in frontend-design shadcn web-design ui-ux-quality-gate; do
  curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d "{\"skill\":\"$name\"}" "$CELERIS_API/api/v1/org/ui-ux/skills"
done
```

`POST /api/v1/org/{id}/skills` は `task_ops::knowledge::set_skill_mount` を呼ぶ管理系で、既存の
`skills_mounts`（`frontend-design` 等を含む）を壊さず 1 件ずつ追記する（同じ名前を 2 回呼んでも冪等）。

### mount の確認

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/org" | \
  jq '.items[] | select(.id=="ui-ux") | {skills_mounts: .profile.skills_mounts, skills: .profile.skills}'
```

- `profile.skills_mounts` に 4 件が入っていること。
- `profile.skills`（routing 用のタグ。`software`, `ui-design`, `ux`, `accessibility`, `frontend`, `react`,
  `typescript`, `css`, `responsive`, `usability` の既存 10 件）が**変わっていない**こと。routing 用タグと
  skill 本文の mount は別欄であり、ここで routing の振り分けに影響は出ない（`ui_ux_skills` という名前を含む
  試験 `crates/task-dispatch/src/dispatcher/tests/ui_ux_skills.rs` が CI でこれを固定している）。

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/skills/frontend-design" | \
  jq '.mounted_by'
# => ["ui-ux"] のように出る（継承後の判定。親ノードで mount していれば子にも現れる）
```

## 4. ui-ux の `profile.policy` に依存方針を足す

`profile` は `PATCH /api/v1/org/{id}` で**丸ごと差し替え**になる（部分更新ではない）。先に現在の
`profile` を取得し、`policy` 配列に 1 行追記した全体を送り返す。

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/org/ui-ux" | jq '.profile' > /tmp/ui-ux-profile.json

jq '.policy += ["skill のコード例に出るライブラリ（next-themes, motion, react-hook-form, zod, lucide-react, figma-squircle, ForesightJS, next/font/google 等）は導入しない。依存は決定済みの範囲（shadcn）のみ、それ以外は提案に留める。テストやビルドで外部ネットワークに出ない。"]' \
  /tmp/ui-ux-profile.json > /tmp/ui-ux-profile.new.json

jq -n --slurpfile p /tmp/ui-ux-profile.new.json '{profile: $p[0]}' | \
  curl -s -X PATCH -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    --data-binary @- "$CELERIS_API/api/v1/org/ui-ux"
```

同じ内容が既に `/tmp/ui-ux-profile.json` の `policy` にあれば二重に足さない（`jq` で確認してから実行する）。

### 確認

```sh
curl -s -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/org" | \
  jq '.items[] | select(.id=="ui-ux") | .profile.policy'
```

- 追加した 1 行が入っていること。
- `skills_mounts` / `skills` / その他の欄が 3 節実行前と変わっていないこと（丸ごと差し替えなので、
  `/tmp/ui-ux-profile.json` を元にした差分だけが入っているはずで、誤って他欄を消していないか必ず diff する）。

## 5. ui-ux 担当 run での到達確認

次に ui-ux 課が担当する実装系 task（worker run）の作業場所と、その task の review run を 1 件ずつ確認する。

```sh
# worker run: 作業場所に 4 skill 全部
ls <ui-ux task の work_dir>/.claude/skills/
# => frontend-design  shadcn  ui-ux-quality-gate  web-design
cat <ui-ux task の work_dir>/.claude/skills/shadcn/SKILL.md | head -5

# review run: ui-ux-quality-gate だけ（主たる design generator 3 件は届かない）
ls <同じ task の review run の作業場所>/.claude/skills/
# => ui-ux-quality-gate
```

claude-code harness の場合、届け方は `crates/task-worker/src/skills.rs` が mount された skill を
`<cwd>/.claude/skills/<name>/` へディレクトリごと複製する（codex は `AGENTS.md` の節、acp は前置き）。
review run は対象 task の `assignee`（= ui-ux）の実効 `skills_mounts` のうち、frontmatter に
`celeris-use: review`（または `work, review`）を持つものだけを受け取る（D4）。`frontend-design` /
`shadcn` / `web-design` は `celeris-use` を持たない（既定 `work`）ので review run には届かない —
これが期待どおりの挙動であり、不具合ではない。

もし本番にまだ ui-ux 担当の run が無ければ、GUI から ui-ux 課が担当する小さな task（例: 既存の
GUI コンポーネントの軽微な調整）を 1 件流してから上記を確認する。

## 6. 戻し方（unmount / delete）

課から外すだけ（KB の本体は残す。他課や将来の再 mount に備える場合）:

```sh
for name in frontend-design shadcn web-design ui-ux-quality-gate; do
  curl -s -X DELETE -H "Authorization: Bearer $TOKEN" \
    "$CELERIS_API/api/v1/org/ui-ux/skills/$name"
done
```

4 節で足した `profile.policy` の 1 行も戻す場合は、4 節と同じ手順で `.policy` から該当文字列を除いた配列を
組み立てて `PATCH` し直す（`jq 'del(.policy[] | select(. == "<追加した文字列>"))'` 等）。

KB から skill 自体を完全に削除する場合（通常は不要。mount を外せば worker には届かなくなる）:

```sh
curl -s -X DELETE -H "Authorization: Bearer $TOKEN" "$CELERIS_API/api/v1/skills/<name>"
```

どこかの組織ノードにまだ mount されていれば `409 skill_mounted` で拒否される（先に unmount が必要。
このリクエストはデータを消す操作なので、他課が同名を再利用していないか `GET /api/v1/skills/{name}` の
`mounted_by` を確認してから実行する）。

## 審査で改変した点（第三者 skill review の反映、再掲）

取り込む本文は upstream のままではなく、人による第三者審査の指摘 3 点を反映済み（詳細は
`docs/progress/ui-ux-skills.md` の「第三者審査の反映」節、各 `SOURCE.md` の `modified:` 行）。

1. **`web-design` の hit-area 導入コマンドを提案化** — worker がそのまま実行できる外部 registry 向け
   導入コマンドだった箇所を、「人に提案し、承認後にのみ導入する」文に書き換えた。
2. **`ui-ux-quality-gate` から `scripts/init_frontend_quality.py` を非同梱** — 対象リポジトリの
   `AGENTS.md` への無条件追記、`--force` での無検査上書き、`--project` パスの未検査書き込みという 3 つの
   リスクがあり、worker に mount しない（ディレクトリごと取り込まない）。`SKILL.md` の Templates 節は
   「手で写す」運用に書き換えてある。
3. **依存方針の明文化**（`config/skills/README.md`、4 節で ui-ux の `profile.policy` にも反映）—
   skill のコード例にだけ出るライブラリを worker が無断導入しない、決定済みの `shadcn` 以外は提案に留める、
   テスト・ビルドで外部ネットワークに出ない。
