# ADR 2026-10-08: CoS が thread workspace に作った文書を返事の添付にし、人への手順は画面の操作で書かせる

---
tasks: [01M4CDNAZF1CDBZVHFDZ1JZJPX]
---

- 日付: 2026-10-08
- 状態: 実装済み（task 01M4CDNAZF1CDBZVHFDZ1JZJPX。実機の LLM 確認は未実施）
- 関連: ADR 2026-10-05-cos-chat-home D1（chat_attachment_refs）/ D2（M の形・run の終端）/ D4（添付の保存・表示）/ D5（UX）、
  ADR 2026-10-05-web-artifact-inline-view（成果物の画面内表示）、ADR-0095 付記 D-d（本番の操作は人）

## 1. 文脈

2026-10-08 の人の指摘: manaba の件を CoS チャットで聞いたら「`artifacts/manaba-monitor-setup.md` を見て」と返され、
その file を web で開く手段が無く、中身も curl・toml・systemctl など運用者の作業だった。

- CoS は `<data_dir>/cos/threads/<thread>/workspace` を cwd として走り、そこに file を書く。web にこの dir を読む API は無い。
- chat の添付（D4）は人の upload だけが作る。assistant の message に添付を付ける経路が無かった。
- skill（cos-operator・cos-inbox-triage）に「人への説明の書き方」の規則が無かった。

## 2. 決定

### D1. run の前後の workspace 差分と、返事が触れた path を返事の添付にする（決定的）

- dispatcher は CoS chat run の起動直前に thread workspace を走査し、`(相対 path, 大きさ, mtime)` を覚える。
  run の終端（`chat_run_finish` の直前）でもう一度走査し、**新しく現れたか変わった通常 file** を候補にする。
- 返事の本文（最終本文、無ければ流した本文）が触れた path も候補にする。対象は
  workspace の絶対 path（`…/cos/threads/<thread>/workspace/<rel>`）と、workspace にある file を指す相対 path
  （`artifacts/x.md`・`./x.md`）。区切りは空白・引用符・backtick・括弧・全角の句読点。
- 走査の除外: dot で始まる名前（`.taskd/`・`.claude/`・`.git/` など run の内部物）、`attachments/`（人の添付を stage した写し）、最上位の `runs/`（adapter が run ごとに書く request.json・stdout.jsonl・prompt.txt）、
  symlink（辿らない）、通常 file 以外。深さ 6・走査 4096 項目で打ち切る。
- 順序は「本文が触れた順 → 差分の path 順」。件数・大きさは `[cos.attachments]` の上限（1 件 `max_file_bytes`、
  1 message `max_files_per_message`・`max_message_bytes`）に従い、超える分は付けない（失敗にしない）。
- 各候補は既存の `ChatAttachmentStore::upload` で保存する（`client_upload_id = cos-workspace:<run_id>:<rel>` で冪等。
  原名は file 名だけ。hash・MIME 判定・保存先・権限は D4 と同じ）。出力 message に `owner_kind=message` で pin し、
  message の `metadata_json.workspace_files = [{"path":<rel>,"attachment_id":<id>}]` を同じ transaction で書く。
- 読めない走査項目は省略し、候補ごとの保存失敗は debug、store の起動・pin の失敗は warn を残す。
  run の終端を添付の都合で変えない。
- LLM は呼ばない。CoS の申告（result.json 等）にも頼らない。

### D2. API: M に `workspace_files` を足す

`M` に `"workspace_files":[{"path":"artifacts/x.md","attachment_id":"a"}]`（無ければ空配列）を足す。
`attachment_ids` は従来どおり pin の一覧（`workspace_files` の id はその部分集合）。既存の欄・意味は変えない。

### D3. web: 添付を画面内で開き、本文の path を添付への link にする

- 添付一覧の md・text に、成果物の表示部品（`artifact-viewers.tsx` の MarkdownViewer・TextViewer）による
  「本文をここで見る」を付ける。種類は名前の拡張子（`artifactKind`）で決め、md は描画、text・csv・code は行番号付き。
  画像は既存の chat プレビュー（`preview_url`、再エンコードした raster）を使う。HTML・SVG・PDF・不明は D4 のとおり download だけ。
  text は 1 MiB 以下だけ画面内で読む（content API に範囲取得が無いので 1 回で読み切る大きさに限る）。
- assistant の本文で `workspace_files` の path に触れた所（絶対 path・相対 path、backtick 囲みも）を、その添付の
  content URL への link に置き換える。fenced code の中は変えない。link を押すと画面遷移せずに該当添付の本文を開いて
  そこへ scroll する。既存の Markdown link も workspace path 宛てなら対応させる。
  画面内表示の無い形式・修飾キー付き操作・別 tab では content URL の download を使う。

### D4. skill の規則: 人への説明は画面の操作で、運用者作業は分け、結論を先に

cos-operator（§10 新設）と cos-inbox-triage（escalation packet の書き方）に次を足す。
両 skill の metadata.version を 2 にし、旧運用節の「人にコマンドを渡す」記述も揃える。
KB 正本は直接変更せず、読み取った正本から `skills/<name>/SKILL.md` を対象とする差分を提出する。

- 返事の最初の 1〜3 行に結論（人が今何をすればよいか、または何もしなくてよいこと）を書く。
- 人にしてほしいことは web の画面名とボタン名（例: 「受信箱」画面の「回答する」）で書く。
- curl・config（toml）・systemd・shell の作業を人に求めない。必要なら「運用者の作業」として節を分け、
  CoS が自分でできるものは自分で行い、できないものは運用者向けの task を起票して card で示す。
- 長い手順・表は workspace に md で書いてよい（D1 で返事に添付され、画面で開ける）。返事には要点と file 名を書く。

## 3. 却下した案

- **workspace を丸ごと web から閲覧させる API**: run の内部物・秘密を含み得る dir を広く晒す。添付は hash 付き blob の写しなので
  run 後の書き換えの影響も受けない。
- **CoS に upload API を叩かせる**: LLM の手順に依存し、忘れたら今回と同じになる。差分は dispatcher が決定的に取れる。
- **原名に相対 path を入れて web が名前で照合**: 原名は表示情報（D4）で、Content-Disposition にも流れる。対応表を別に持つ。

## 4. 試験

- task-dispatch: fake adapter が workspace に md・csv を書き、返事で絶対 path に触れる run → 出力 message の
  `attachment_ids`・`workspace_files`、hash 一致、除外（dot dir・`attachments/`・`runs/`・symlink）、上限超過の切り捨て。
- task-core: `workspace_files` の保存と M への写像、assistant 以外の message の拒否。
- web（vitest）: path と Markdown link の変換、fenced code の保持、添付の表示部品と形式選択。
- web（Playwright）: fake daemon の SSE で届く返事から path/Markdown link を押して md の見出し・本文と csv を表示し、
  PDF のリンクはダウンロードできる。画面遷移しないことも確認。
