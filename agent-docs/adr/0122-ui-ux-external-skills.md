---
tasks: [01M3XTAH6KYYXTWRTNC2D4MDTG]
---
# ADR-0122: 外部 agent skill の vendoring・KB への取り込み・ui-ux への mount・quality gate の reviewer 配布

- 日付: 2026-10-02
- 状態: 採用・実装済み（2026-10-02）。D1〜D4 のコードと D3 の種、D5 の手順書まで実装済み。実装 commit:
  vendor-skills `d39733d8`、vet-skills `3cfdf69a`、adr `7ce72c5c`、skill-import `41e6324b`、
  review-skills `dcf7aefd`、e2e-verify `6d95a306`、runbook `99a529c6`。
  D5（本番の KB 取り込み・mount）は人が `docs/ops/ui-ux-external-skills.md` の手順で実行する。
- 関連: ADR-0056 D3（KB の `skills/` と `skills_mounts`、run への届け方）、ADR-0046（profile の継承、skill はタグ）、
  ADR-0073（ui-ux 課と routing）、ADR-0069（担当の決定的な選択）、ADR-0007 D5（合成 review task）、
  ADR-0095 付記 D-d（本番 host の操作は人）

## 状況

ui-ux 課（ADR-0073）に、Web UI の設計・実装に使う外部 skill を 4 件持たせたい。

| 名前（KB / mount） | upstream | commit | license |
|---|---|---|---|
| `frontend-design` | anthropics/skills `skills/frontend-design` | `8a1541c4` | Apache-2.0（`LICENSE.txt`） |
| `shadcn` | shadcn-ui/ui `skills/shadcn` | `d75a96ab` | MIT（`LICENSE.upstream`） |
| `web-design` | pascalorg/skills `web-design` | `7be87e92` | MIT（README の License 節。`LICENSE.upstream`） |
| `ui-ux-quality-gate` | atuizz/codex-ui-ux-skill `ui-ux`（改名） | `3c311f71` | MIT（`LICENSE.upstream`） |

いずれも 2026-10-02 取得。repo の `config/skills/<name>/` に本文・付属ファイル・`SOURCE.md` を置いた（vendor-skills）。

既存の仕組み（ADR-0056 D3）の現状:

- 本文の正本は KB の `skills/<name>/SKILL.md`（＋付属ファイル）。書き込みは
  `task_ops::knowledge::skills_put`（`crates/task-ops/src/knowledge/skills.rs:131`）だけで、
  `PUT /api/v1/skills/{name}`（`crates/task-api/src/skills.rs:38`、admin）と MCP の `skills_put` が同じ関数を呼ぶ。
  付属ファイルは `SkillFileBody { path, content: String }`（`crates/task-api/src/skills.rs:93`）でテキストしか運べない。
- frontmatter は `skill_frontmatter`（`skills.rs:55`）が「1 行 1 鍵、`split_once(':')`」で読む最小形。
  `name` が URL の名前と一致し `description` が空でないことだけを検証し、`source:` が無ければ足す（`prepare_skill_md`）。
  本文は書き換えないので未知鍵は残るが、複数行の値（`>` / `|` / 字下げ継続）とクォートは解釈しない。
- 課への付与は `Profile.skills_mounts`（`POST /api/v1/org/{id}/skills`、`crates/task-api/src/skills.rs:42`）。
  routing のタグ `Profile.skills` とは別の欄で、継承は knowledge mount と同じ和（`crates/task-core/src/profile.rs:378`）。
- worker run への届け方: `run_context.rs:308` で担当ノードの実効 profile の `skills_mounts` を
  `skills_context`（`run_context.rs:727`）が KB から解決して `RunContext.skills` に載せ、
  `crates/task-worker/src/skills.rs` が claude-code は `<cwd>/.claude/skills/<name>/` へ丸ごと写し、
  codex は `AGENTS.md` の区切り節、acp は前置きに足す。
- 一括取り込みの CLI は無い（`celerisctl` に `skills` 系の subcommand は無い）。

### review run への配布経路の現状（D4 の前提）

review run には skill は**届かない**。

1. `synthetic_review_task`（`crates/task-dispatch/src/review.rs:825`）は `skills: Vec::new()`（`review.rs:847`。
   これは routing タグの欄）で、担当は対象 task の `assignee` を継ぐ（`review.rs:862`）。
2. reviewer の node / profile は `pick_reviewer`（`crates/task-dispatch/src/dispatcher/review_spawn.rs:424`）が
   `task_core::department_of(&org, assignee)`（`crates/task-core/src/org.rs:291`）で決める。課（section）を
   親へ辿って**部**を返すので、ui-ux の task の reviewer は `engineering` の profile になる（ui-ux の mount は見えない）。
3. `run_reviewer_inner` の `RunContext` は `..RunContext::default()`（`review.rs:986`）で組み、`skills` は空。
   reviewer run は `work_dir: None`（対象 task のディレクトリで動く）。

## 決定

### D1. repo の `config/skills/<name>/` は正本の写し。本番 KB へは `skills_put` 経路で一括取り込みする

- `config/skills/<name>/` は upstream の写し（または最小の改変）で、`SOURCE.md` を必須とする（`source_url` /
  `commit` / `fetched` / `license` / `license_file`、意図的な改変は `modified:` 行）。ライセンス本文の写しを同じ
  ディレクトリに置く。`config/skills/README.md` が一覧と依存方針（D6）を持つ。
- 本番の正本は KB の `skills/<name>/`（ADR-0056 D3 のまま）。repo から KB への取り込みは新しい書き込み経路を作らず、
  `skills_put` を呼ぶ。
- CLI を 1 つ足す: **`celerisctl skills import <dir> [--name <name>]... [--config <path>]`**。
  - `<dir>` が `SKILL.md` を持てばその 1 件、持たなければ直下の各ディレクトリ（`SKILL.md` を持つもの）を取り込む。
    `--name` を繰り返すとその名前だけに絞る。名前はディレクトリ名（frontmatter の `name` と一致しなければ
    `skills_put` の `NameMismatch` で止める）。
  - KB の根は `celerisctl knowledge` と同じく設定（`--config` / `CELERIS_CONFIG`）の `knowledge.root` から取り、
    `task_ops::knowledge::skills_put(root, name, skill_md, files, Some("celerisctl"))` を直接呼ぶ
    （`celerisctl knowledge` / `mcp client` と同じ管理系の流儀。daemon の再起動は要らない — `skills_context` は
    dispatch のたびに KB を読む）。
  - 1 件ごとに `skills/<name>/SKILL.md` と付属ファイル数、飛ばしたファイル（D2）を 1 行ずつ出す。冪等
    （同じ内容の再取り込みは KB のコミットを増やさない。`knowledge.rs:103` の既存の扱い）。
- 同等の手段として、CLI が使えない場面では `PUT /api/v1/skills/{name}` に `{skill_md, files}` を送ってもよい（D5）。

### D2. SKILL.md frontmatter の互換と付属ファイル

- frontmatter の読み取り（`skill_frontmatter`）を次のとおり広げる。書き戻しは今までどおり**原文のまま**
  （`source:` の追記だけ）で、未知鍵（`license`、`metadata:` の入れ子、`user-invocable`、`allowed-tools` 等）は
  壊さない・消さない。
  - 字下げの無い行だけを最上位の鍵とする（`metadata:` の下の `author:` などを最上位として読まない）。
  - 値の前後の対になった `"…"` / `'…'` を外す（`ui-ux-quality-gate` の `description` は二重引用符の 1 行）。
  - `description: >` / `|`（`-` / `+` 付きを含む）と、字下げした継続行を値として連結する（`>` は空白で、`|` は改行で）。
- 取得記録の実形（2026-10-02）では 4 件とも `description` は 1 行で、上の拡張が無くても現行の検証は通る。
  拡張は upstream の更新で複数行になったときに `description` が `>` だけになる誤読を防ぐためのもの。
- 付属ファイルは **UTF-8 のテキストだけ**を取り込む。UTF-8 として読めないファイル（現状は
  `shadcn/assets/shadcn.png` と `shadcn-small.png` のアイコン 2 件）は取り込まず、import の出力に
  「skipped (binary)」と出す。repo の写しには残す。API の `content: String` を base64 などに広げることはしない
  （アイコンは指示として使われず、API の形を変える理由にならない）。シンボリックリンクは辿らず飛ばす。
- `SOURCE.md` とライセンスの写しも付属ファイルとして KB に入れる（再配布にライセンス本文を伴わせるため）。

### D3. ui-ux の `skills_mounts` に 4 件を足す。routing の `profile.skills` は変えない

- ui-ux の `skills_mounts` = `frontend-design` / `shadcn` / `web-design` / `ui-ux-quality-gate`。
  ui-ux の子ノードは無いので継承の影響は ui-ux だけ。software-engineering には mount しない。
- `profile.skills`（routing のタグ）は一切変えない。ADR-0073 D2 の振り分け
  （`[typescript, react, frontend]` → ui-ux、API / Rust → software-engineering）はそのまま。回帰は
  `config::tests::example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering` と、
  変更前後で担当が同じことを示す試験（名前に `ui_ux_skills` を含める）で固定する。
- 種 `config/org.example.toml` の ui-ux の `[org.profile]` にも `skills_mounts` の 4 件と D6 の policy を足す
  （空 DB のときだけ読まれる。本番は D5）。

### D4. ui-ux-quality-gate を reviewer にも届ける: frontmatter 鍵 `celeris-use`

上の「現状」のとおり、review run には担当課の mount が届かない。最小の変更で quality gate を review run に届ける。

- SKILL.md の frontmatter に鍵 **`celeris-use`** を置けるようにする。値はカンマ区切りの `work` / `review`。
  - 鍵が無い: `work` と同じ（今までどおり worker run にだけ届く。既存の skill の挙動は変わらない）。
  - `review`: review run にだけ届く。`work, review`: 両方に届く。未知の値は無視（`work` 扱いの既定は変えない）。
- `ui-ux-quality-gate/SKILL.md` に `celeris-use: work, review` を足す（upstream からの改変として `SOURCE.md` の
  `modified:` に記録）。worker run では実装中の自己点検に、review run では loading / empty / error / mobile /
  accessibility と AI 生成 UI のアンチパターンの判定に使う。主たる design generator は `frontend-design` /
  `web-design` / `shadcn` で、quality gate はその位置に置かない。
- review run の skills は、reviewer の node（部）ではなく**対象 task の `assignee`（課）の実効 profile の
  `skills_mounts`** から解決する（課の作法で判定するため。reviewer の node / lane の決め方は変えない）。
  `pick_reviewer`（`review_spawn.rs:424`）で `skills_context` を呼び、`celeris-use` に `review` を含むものだけを
  `ReviewerRun` の新しい欄で運び、`run_reviewer_inner` の `RunContext.skills` に入れる。
  worker run の側（`run_context.rs:308`）は `celeris-use` が `review` だけのものを除く。
- 届け方は worker run と同じ `task-worker/src/skills.rs`（reviewer の cwd は対象 task のディレクトリなので、
  claude-code では `<task dir>/.claude/skills/` に写る。`.celeris/skills.json` の掃除も同じ）。
- 判定は決定的（frontmatter を読むだけ）。dispatcher に LLM 呼び出しは入れない。

### D5. 本番への登録は人が実行する

本番の KB と DB（org）への書き込みは worker から行わない（ADR-0095 付記 D-d）。runbook の WorkUnit が手順書を書き、
人が実行する。手順の骨子:

1. D1〜D4 を含む release を昇格させる（`scripts/selfdeploy/release.sh` / `verify.sh`）。
2. KB へ取り込む: `celerisctl skills import config/skills --config ~/.config/celeris/config.toml`
   （または 4 件それぞれ `PUT /api/v1/skills/{name}`）。確認: `GET /api/v1/skills` に 4 件、
   `GET /api/v1/skills/ui-ux-quality-gate` の `skill_md` に `celeris-use: work, review`。
3. mount する: `POST /api/v1/org/ui-ux/skills` に `{"skill": "<name>"}` を 4 回。
   確認: `GET /api/v1/org/ui-ux` の `skills_mounts` に 4 件、`skills` が変わっていないこと。
4. policy を足す: `PATCH /api/v1/org/ui-ux` で D6 の 1 行を `profile.policy` に追加。
5. 次の ui-ux 担当 run の `request.json` の `context.skills` に 4 件、review run に `ui-ux-quality-gate` だけが
   載っていることを確かめる。

### D6. 第三者 skill の審査結果の反映（vet-skills で実施済み）

- `web-design`: 本文の hit-area ユーティリティ導入のコマンドを、実行指示から「提案として人に示す」文に改変した
  （`SOURCE.md` の `modified:` に記録）。
- `ui-ux-quality-gate`: upstream の `scripts/init_frontend_quality.py` を**同梱しない・mount しない**。理由:
  対象 repo の `AGENTS.md` に追記する、`--force` で既存の文書を上書きする、`--project` の書き込み先パスを検査しない。
  worker が skill の指示どおり実行すると repo の規約文書を壊しうる。本文の Templates 節は「手で写す」文に改めた。
- ui-ux worker の依存方針（`config/skills/README.md` に記載済み。D3 / D5 で ui-ux の `profile.policy` にも置く）:
  「skill のコード例に出るライブラリ（`next-themes`, `motion`, `react-hook-form`, `zod`, `lucide-react`,
  `figma-squircle`, ForesightJS, `next/font/google` 等）は導入しない。依存は決定済みの範囲（shadcn）のみ、それ以外は
  提案に留める。テストやビルドで外部ネットワークに出ない」。
- `web-design` のライセンスは LICENSE ファイルが無く README の License 節（MIT）による。より厳しい基準を採るなら
  KB から外す判断は人に残す（`docs/progress/ui-ux-skills.md`）。

## 結果

- 外部 skill は「repo の写し（出典・ライセンス付き）→ `skills_put` → mount」の 1 本の経路で入る。新しい保管場所や
  DB の表は作らない。
- ui-ux の worker run には 4 件、ui-ux の task の review run には `ui-ux-quality-gate` だけが届く。
  routing は変わらない。
- `celeris-use` の既定は今までの挙動と同じなので、既存の mount に影響しない。

## 代替案

- review 用の mount を profile に別欄（`review_skills_mounts`）で持つ: org の API・migration・GUI まで変わる。
  skill の性質（reviewer で使うか）は skill 自身の属性なので frontmatter に置く方が小さい。
- reviewer の node（部）の mount を使う: `engineering` に quality gate を mount すると software-engineering など
  全課の review に届いてしまう。
- 付属ファイルを base64 で運べるよう API を広げる: 対象はアイコン 2 件で、指示として使われない。
- `celerisctl` に CLI を足さず curl だけで入れる: 付属ファイルを JSON に詰める手作業が誤りやすい。CLI は
  `skills_put` を呼ぶだけで新しい書き込み経路にはならない。
