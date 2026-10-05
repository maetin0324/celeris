---
title: admin 画面群（組織・知識・help・login・accounts・providers・clusters・daemon・releases）の visual QA
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: done
completed: 2026-10-04
updated: 2026-10-04
---

# admin 画面群の visual QA

build 済み web/（基点 7eab1be6a4e3）を `corepack pnpm@12.6.0 -C web screenshots` で撮影した pre screenshot（`qa-qa-admin-pre`: 32 画面 × 360/390/412/1440、`qa-qa-admin-pre-states`: 8 状態 × 代表画面 × 4 幅）を元に、ui-ux-quality-gate skill の観点（generic AI dashboard 化・Celeris 固有の情報構造・long text/長 ID/0 件/多数/loading/error/stale/403・keyboard/focus/contrast・touch target）で対象 9 画面（組織・知識・help・login・accounts・providers・clusters・daemon・releases）を確認した。既存の build 段 4 葉（org・knowledge-help・ops-config・ops-runtime）の自己レビューと `docs/frontend/UX_AUDIT.md`・`agent-docs/progress/2026-10-04-web-admin-screens.md` の残課題を踏まえ、未解決または新たに確認できた指摘だけを挙げる。

## critique

1. **[knowledge-help] `/knowledge/skills` — h1・タブ・本文が英語のまま**（重大度: 高）
   `web/features/knowledge/skills-screen.tsx` の `ScreenFrame` の title が `"skills"`、`/knowledge` のサブナビのタブも「候補」「skills」と後者だけ英語、本文も「skill 一覧」「skill 一覧から skill を選んでください。」。360px・1440px とも再現（`qa-qa-admin-pre/_knowledge_skills-{360,1440}.png`）。他の全画面が日本語の h1・ラベルで統一されている中、この画面だけ主見出しが英単語になっており、Celeris の運用語彙ではなく生の実装語がそのまま画面の顔になっている（典型的な generic AI dashboard）。`web/e2e/parity/knowledge.spec.ts:136` は h1 の `level` だけを見ており文言を固定していないため、h1 を含む日本語化はこの WU の範囲内で可能。

2. **[ops-runtime] `/daemon` — h1・nav ラベル・本文が英語の実装語のまま**（重大度: 高。h1 と nav ラベルは構造的制約あり）
   `web/features/ops/daemon-screen.tsx:171` の `ScreenFrame` title が `"daemon"`。本文も「dispatcher の状態はまだありません。」「保存された event から状態を再計算し、保存値との差を確認します。」「replay を実行」「最終 poll」と英語の実装語が日本語文に混在する（`qa-qa-admin-pre/_daemon-{360,1440}.png`）。nav の該当項目（`web/components/shell/nav-items.ts:17`）も `label: "daemon"` のみ英語（他 16 項目は日本語）。ただし h1 の文言は `web/e2e/parity/ops.spec.ts:130` が `getByRole("heading", { level: 1, name: "daemon" })` で固定しており、h1 自体の日本語化には parity spec の変更が要る（この WU の許可範囲外 `web/e2e/parity/` に抵触）。nav ラベルも `web/components/shell/` は対象外ディレクトリ。本文中の「dispatcher」「event」「replay」「poll」の訳語化は `daemon-screen.tsx` 側だけで直せる範囲。

3. **[ops-runtime] `/releases` 360px — 昇格前に確認すべき「問題・直近の失敗」列が初期表示で見えない**（重大度: 高。安全に関わる）
   一覧表の列は「版・状態」「操作」「ビルド」「昇格」「変更」「問題・直近の失敗」の順。360px では最初の 2 列しか画面に入らず、残りは表の枠内だけで横スクロールする作りだが、視覚的なスクロール手がかり（影・矢印等）が無い（`qa-qa-admin-pre/_releases-360.png`）。昇格・巻き戻しは本番 daemon を切り替える後戻りしにくい操作であり、判断材料のうち最もリスクに関わる「問題・直近の失敗」列が初期表示から隠れているのに気づきにくい。ConfirmDialog の確認文には対象・影響・戻し方が入るため実害は限定的だが、一覧段階での事前把握ができない。

4. **[ops-runtime] `/clusters` 360px — 最重要の「失敗理由」列が同様に隠れる／`host` ラベルが英語**（重大度: 中〜高）
   クラスタ一覧表は「クラスタ」「接続」「最終確認」「最後の切断」「失敗理由」の 5 列。360px では「失」の 1 文字で切れ、表はここでも手がかり無しに局所スクロールする（`qa-qa-admin-pre/_clusters-360.png`）。カードの DataList ラベル `host` も英語のまま（`認証`・`使用中`は日本語）。

5. **[ops-config] `/providers` — 実装語を含む長文説明と英語の form ラベルが前面に出る**（重大度: 中〜高）
   見出し「adapter / harness の実行枠」の下の説明文は「道具（claude-code・codex・acp・paperqa・langmem・ldr など）…」「celeris/<tier> は実行時に proxy が供給元を選ぶ抽象モデルです。」と実装者向けの語彙が並ぶ。card 内の form ラベルも `concurrency`・`model`・`tiers`・`frontier`/`standard`/`cheap` が無訳のまま（`qa-qa-admin-pre/_providers-{360,1440}.png`）。`agent-docs/progress/2026-10-04-web-admin-screens/ops-config.md` の providers 節でも同種の課題（StatusBadge の設定語彙）が「提案」止まりで残っており、1440px では説明文＋英語ラベルの form が画面の大半を占め、運用者が最初に読みたい「使えるかどうか」より前に長文が来る。

6. **[org] `/org` 360〜412px — 「担当を追加」form が選んだ担当の詳細より前に来る**（重大度: 中）
   `web/features/org/org-screen.tsx:334-359` は `lg:grid-cols-5` の 2 カラムで、木→`CreateForm`→詳細の順に DOM が並ぶ。lg（1024px）未満では 1 カラムになるため、木でノードを選んでも「担当を追加」form を経てからでないと選択した担当の詳細（`担当の詳細` section）に到達できない。`agent-docs/progress/2026-10-04-web-admin-screens/org.md` のtree-detail 葉自身も「提案」としてこの点を認識済みだが未着手（デフォルト fixture が取得失敗のため screenshot では未確認、ソース読解で確認）。

7. **[knowledge-help] `/knowledge`・`/knowledge/inbox` — 本文 Markdown の見出しと Section の title が重複表示されうる**（重大度: 中）
   `web/features/knowledge/knowledge-screen.tsx:272-293`（`Candidate`）と同 239 行目（ページ本文）は `Section title={...}` の直後に `<Markdown source={...}/>` をそのまま描画する。fixture の候補本文が `# New knowledge` で始まるため、h2「New knowledge」の直後に本文側の見出し「New knowledge」がもう一度表示される（`qa-qa-admin-pre/_knowledge_inbox-{360,1440}.png`）。これは fixture 固有の偶然ではなく、本番の knowledge ページ・候補が `# <title>` で始まる markdown であれば一般的に起きる構造上の重複。`web/e2e/parity/knowledge.spec.ts:53` は h2「New knowledge」の存在だけを見ており、本文側の重複見出しを抑止する変更（leading heading の除去等）はこの WU の範囲内で可能。

8. **[ops-config] `/accounts` — `adapter`・`secret` など一部ラベルが英語のまま**（重大度: 中）
   一覧 card の DataList ラベル `adapter`、下段の節見出し `secret` が英語（`状態`・`認証情報`・`最終確認`等は日本語）。`qa-qa-admin-pre/_accounts-{360,1440}.png` で確認。/providers・/daemon・/clusters と合わせ、ops 系画面全体で英語の実装語と日本語の運用語彙が無秩序に混在しており、一貫した訳語表が無いまま個別に直されてきたことがうかがえる。

9. **[knowledge-help] `/knowledge` 1440px（未選択時）— 本文枠の過剰な空白が残る**（重大度: 低〜中）
   検索結果 1 件に対し右側の本文枠は「検索結果から知識を選んでください。」のみで、画面幅の約 2/3 が空白のまま（`qa-qa-admin-pre/_knowledge-1440.png`）。`docs/frontend/UX_AUDIT.md:144` が before 版で指摘した「過剰余白」と同じ症状で、knowledge 葉の変更は選択後の表示（出典・scope・更新日の追加）に留まり、未選択時の初期レイアウトは直っていない。

10. **[help-login] `/login` — 失敗後の「次にどこへ戻るか」が画面に出ない**（重大度: 低）
    `docs/frontend/UX_AUDIT.md:30` が指摘した「戻り先 `next` を画面に示さない」は help-login 葉の変更後も残る（`qa-qa-admin-pre/_login-{360,1440}.png` は初期状態のみで確認、`web/routes/login.tsx` のソース上も `next` の表示が無いことを確認）。help-login 葉の対応範囲は失敗理由の alert と focus 復帰までで、this は対象外のまま。

## 修正

### ops-config（accounts・providers）

対象: `web/features/ops/{providers-screen.tsx,providers-form.ts,accounts-screen.tsx,secrets-section.tsx,mcp-clients.tsx}`、unit 期待の更新（`providers-screen.test.tsx`）、`web/e2e/admin/ops-config.spec.ts` の label 1 か所。parity spec が見る accessible name（`adapter`・`concurrency`・`新規 id`・`secret id`・`secret 値`・`secret を保存`・listitem 名・h1）と本文 `concurrency N` は語として残し、日本語を主にして設定語を括弧で添える形にした。

- 5. [修正: `/providers` の節見出しを「adapter / harness の実行枠（n）」→「実行枠（n）」、説明は「使えるか・休止中か・失敗しているか」を確かめる 1 文に縮め、主文に残っていた `run` も「作業」に替えた。道具（adapter / harness）・受ける段（tiers）・LLM source・celeris/&lt;tier&gt; の説明は開閉式の「用語の説明」（`<details>`、summary は min-h-11）へ下げた。form ラベルを「同時実行数（concurrency）」「モデル（model）」「受ける段（tiers）」「道具（adapter）」「新規の同時実行数（concurrency）」「新規のモデル（model）」に、tier の checkbox を「frontier（最上位）」「standard（標準）」「cheap（安価）」に、表の列「tiers」→「受ける段」、要約行を「設定: codex・concurrency 4・段 …・モデル …」「道具の種類: …」に、検証文言を「同時実行数は 0 以上の整数…」に替えた]
- 5. [残課題: StatusBadge の設定語彙（`agent-docs/progress/2026-10-04-web-admin-screens/ops-config.md` の提案）と見出し「LLM source」は accounts と providers 共通の定義語として残した。`concurrency N` の本文は parity（`web/e2e/parity/ops.spec.ts:148` の `/concurrency 4/`）が見るため英語の設定語を残す]
- 8. [修正: `/accounts` の card の DataList ラベル `adapter`→「道具」、追加 form の select を「道具（adapter）」、secret 節の h2 を「秘密の値（secret）」、MCP クライアント card の `scopes`→「権限の範囲」に替えた]
- 8. [残課題: 「secret id」「secret 値」「secret を保存」「secret を削除」や listitem 名「secret &lt;id&gt;」は parity が accessible name で固定しているため英語の `secret` を残す。ops 系画面全体の訳語表（daemon・clusters 側を含む）は fix-ops-runtime 葉と post-record で揃える]

確認（2026-10-04、この WU branch）:
- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` → exit 0（vitest 57 files・347 tests、node 42 tests）
- `corepack pnpm@12.6.0 -C web build` → exit 0、`corepack pnpm@12.6.0 -C web e2e e2e/admin/ops-config.spec.ts e2e/parity/ops.spec.ts` → 14 passed
- FRONTEND_CONTRACT §66 の生の色・任意値 grep を `web/features/ops web/routes/accounts.tsx web/routes/providers.tsx web/e2e/admin`（`*.test.*` 除く）に当てて 0 件
- `mobile-audit` は exit 1 だが違反は `/projects/P1`・`/tasks/T1`・`/tasks/T1/changes`（範囲外、未命名 textarea 等）のみで、`/accounts`・`/providers` は違反なし

### org

対象: `web/features/org/org-screen.tsx`（並べ替えのみ）、`web/e2e/admin/org.spec.ts`（順序と 1440px の配置の確認を追加）。`org-skills.tsx`・`org-tree.ts`・`web/routes/org.*` は [org] 指摘の対象外で変更なし。h1「組織」・region 名「担当の追加」「担当の詳細」「担当の編集」・URL は変えていない。critique の [org] タグは 6 のみ。

- 6. [修正: `/org` の DOM 順を 木 → 担当の詳細 → 担当を追加 に変えた。lg 未満の 1 カラムでは、木で選んだ担当の詳細が木の直後に来て「担当を追加」form は最後に回る。lg 以上は `lg:flow-root` の中で木と追加を `lg:float-left lg:w-2/5`（追加は `lg:clear-left`）、詳細を `lg:float-right lg:w-3/5` に置き、従来どおり木・追加が左、詳細が右に並ぶ。grid の `row-span` だと詳細の高さが木と追加の 2 行に割り振られて木の下に隙間ができ、`grid-rows-[...]` は任意値になるため float を選んだ。DOM 順と見た目の順は全幅で一致する（order での並べ替えはしていない）]
- 6. [残課題: 木が長い（担当が多い）とき、スマホ幅で選んだ後に詳細まで scroll が要るのは変わらない。選択後に詳細の見出しへ focus・scroll を移すかは、木を keyboard で続けて辿る操作と衝突するため post-record の再 critique で判断する]

確認（2026-10-04、この WU branch）:
- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test` / `check:boundaries` → exit 0（vitest 347 tests、lint warning 5 件は既存の states.spec.ts・styles.css）
- `corepack pnpm@12.6.0 -C web build` → exit 0、`corepack pnpm@12.6.0 -C web e2e e2e/admin/org.spec.ts e2e/parity/org.spec.ts` → 10 passed・1 skipped（WEB_SHOTS_OUT 無しの screenshot）
- FRONTEND_CONTRACT §66 の生の色・任意値 grep を `web/features/org web/routes/org.* web/e2e/admin`（`*.test.*` 除く）に当てて 0 件

### ops-runtime（clusters・daemon・releases）

対象: `web/features/ops/clusters-screen.tsx`・`daemon-screen.tsx`・`releases-screen.tsx` と `web/e2e/admin/ops-runtime.spec.ts`。PC の表、昇格の確認ダイアログ、parity が見る h1・操作名・URL は維持した。

- 2. [修正: `/daemon` の本文を運用者の語彙に整理した。「dispatcher の状態」→「実行管理の状態」、「event」→「履歴」、「poll」→「取得」、「tick」→「動作確認」。照合操作の見出し・説明・件数も日本語を主表示にした。parity が固定する操作の accessible name「replay を実行」と照合件数の英語表記は補助表示として残した]
- 2. [残課題: h1「daemon」は parity が固定し、nav の「daemon」はこの葉の対象外であるため残る。h1 と nav を揃えるには parity と shell を含む別の変更が必要]
- 3. [修正: `/releases` はスマホ幅で版・状態の直後に「問題・直近の失敗」を表示する縦の一覧に切り替えた。昇格・巻き戻しボタンはその判断材料の直後に置き、PC の表と確認ダイアログは維持した。360px の対象 e2e で問題欄と操作を確認した]
- 4. [修正: `/clusters` はスマホ幅で接続状態と失敗理由を各クラスタの先頭に示す縦の一覧に切り替えた。最終確認と最後の切断も同じ項目内に示し、PC の表は維持した。操作カードの `host` ラベルは「接続先」にした]

確認（2026-10-04、この WU branch）:
- offline install は依存 tarball のローカル store 不足で停止したため、同じ lockfile の既存依存から実行ファイルを参照した。`tsc -b`・`biome check .`・`vitest run`・`vite build` は exit 0（347 tests、lint warning 5 件は既存箇所）。
- `corepack pnpm@12.6.0 -C web e2e e2e/admin/ops-runtime.spec.ts && corepack pnpm@12.6.0 -C web e2e e2e/parity/ops.spec.ts` は exit 0（11 + 8 passed）。前回の check 失敗は worktree の `node_modules` にある `@playwright/test/index.mjs` が `index.d.ts` を指す壊れたリンクに起因し、同じ lockfile の依存を worktree 内へ復元して解消した。`mobile-audit --only` は `/clusters`・`/daemon`・`/releases` の各 4 幅で exit 0。
- この葉では指示に従い screenshot を撮らない。post screenshot と再 critique は後続の post-record 葉で行う。

### knowledge-help（知識・help・login）

対象: `web/features/knowledge/{knowledge-screen.tsx,skills-screen.tsx}`、`web/routes/login.tsx`、unit `web/features/knowledge/knowledge-screen.test.ts`（新規）、`web/e2e/admin/login.spec.ts`（1 件追加）。`help-screen.tsx` と `web/routes/{knowledge.*,help}.tsx` は該当する指摘が無く変更なし。URL・parity が見る accessible name（「編集」「保存」「作成」「削除」「skill「demo」を削除」「New knowledge」の h2 など）は変えていない。critique の [knowledge-help] / [help-login] タグは 1・7・9・10。

- 1. [修正: `/knowledge/skills` の h1 を「skills」→「手順書（skills）」、`/knowledge` のタブを「手順書（skills）」、操作 nav の名前を「手順書の操作」、節見出しを「手順書の一覧」「手順書の作成」、表の名前・0 件文・削除 dialog の題と対象を「手順書」に替えた。h1 の下に「手順書は組織の画面で課に付けるとその課の作業場所に配られる」と、組織と課の関係を 1 文で示した]
- 1. [残課題: `web/e2e/support/screens.ts` が h1 名を `skills`（部分一致）で引くため、h1 から英語の `skills` は外せない（括弧の補足として残した）。削除の確定ボタン「skill「demo」を削除」は parity が固定するので英語のまま]
- 7. [修正: 本文 Markdown の先頭の見出し（front matter の後の `#`〜`######`）が Section の title と同じなら外して描画する `withoutLeadingTitle` を足し、`/knowledge` のページ本文と `/knowledge/inbox` の候補本文に使った。見出しを外して空になる本文は「本文は見出しだけです。」と書く。unit 3 件で固定]
- 9. [修正: `/knowledge` は未選択のとき右の空の本文枠を出さず、検索結果を全幅で並べ、見出しの下に「タイトルを選ぶと本文と出典を開きます。」を置いた。同じ症状の `/knowledge/skills` 未選択時も一覧を全幅にした。選択後は従来どおり 2:3 の 2 カラム]
- 10. [修正: `/login` の h1 の下に戻り先を示す（`next` があれば「ログイン後に <path> へ戻ります。」、無いか不正なら「ログイン後はホームを開きます。」）。失敗後も残る。form は従来どおり JS 無しの POST で、daemon には触れない。`web/e2e/admin/login.spec.ts` に 1 件追加]

確認（2026-10-04、この WU branch）:
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → exit 0。`typecheck` / `lint`（warning 5 件は既存の states.spec.ts・styles.css）/ `test`（vitest 350 tests、node 42 tests）/ `check:boundaries` / `build` → exit 0
- `corepack pnpm@12.6.0 -C web e2e e2e/admin/login.spec.ts e2e/parity/knowledge.spec.ts e2e/parity/help.spec.ts` → 10 passed・1 skipped（WEB_SHOTS_OUT 無しの screenshot）
- `corepack pnpm@12.6.0 -C web e2e:all e2e/a11y/axe.spec.ts e2e/parity/mobile-gate.spec.ts -g "knowledge|login|help"` → 10 passed。`mobile-audit --only` を `/knowledge`・`/knowledge/inbox`・`/knowledge/skills`・`/help`・`/login` に当てて各 exit 0
- FRONTEND_CONTRACT §66 の生の色・任意値 grep を `web/features/{knowledge,help,org,ops} web/routes/{knowledge.*,help.tsx,login.tsx} web/e2e/admin`（`*.test.*` 除く）に当てて 0 件
- この葉では指示に従い screenshot を撮らない。post screenshot と再 critique は post-record 葉で行う。

## gate 結果

4 つの修正葉を統合した worktree で再検査した。offline store の不足は、ホストに既存のローカル pnpm store の内容をこの worktree の store に統合して解消した。ネットワーク取得はしていない。続けて、指定の install → build → typecheck → lint → test → check:parity → check:boundaries → check:secrets → mobile-audit → e2e の順に実行した。

| 検査 | exit | 要点 |
| --- | ---: | --- |
| `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | lockfile 固定・offline のまま完了 |
| `corepack pnpm@12.6.0 -C web build` | 0 | Vite build 完了 |
| `corepack pnpm@12.6.0 -C web typecheck` | 0 | TypeScript project build 完了 |
| `corepack pnpm@12.6.0 -C web lint` | 0 | 5 warnings（既存の states.spec.ts・styles.css）、error なし |
| `corepack pnpm@12.6.0 -C web test` | 0 | Vitest 58 files・350 tests、node:test 42 tests |
| `corepack pnpm@12.6.0 -C web check:parity` | 0 | parity static check 完了 |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 0 | import 境界 check 完了 |
| `corepack pnpm@12.6.0 -C web check:secrets` | 0 | build・HTML・API・error・log に token なし |
| `corepack pnpm@12.6.0 -C web mobile-audit` | 0 | 31 経路 × 360/390/412/1440 px、違反 0 |
| `corepack pnpm@12.6.0 -C web e2e` | 0 | functional 166 passed・8 skipped |

対象外の 2 file を修正した理由は、受け入れ条件が admin 9 画面だけでなく全画面の mobile-audit を要求するためである。`web/scripts/mobile-audit.mjs` は input に加え textarea/select の関連付けられた `labels` を読むようにし、親 label 付き textarea の未命名という誤判定を解消した。`web/features/tasks/overview-view.tsx` は短 ID と integration repair のリンクに `min-w-11` / `min-h-11` を与え、実際に 17×44px だったタップ領域を 44×44px 以上にした。`/projects/P1` と `/tasks/T1/changes` の残りは同じ label 判定が原因で、画面側の変更は不要だった。


## 再critique（post）

post-record（実行日 2026-10-05 UTC、検証対象 `5c9a7f555f11`）。front matter の completed・updated は指定された工程日 2026-10-04 とした。この葉では画面・fixture・parity のコードを変更していない。

ui-ux-quality-gate の project cognition と web-design の構造・状態・操作順を基準に再評価した。完成済みの管理・運用画面で、利用者は担当の設定と運用状態を確認する人。入口は管理ナビ、最初の判断は「どの担当・接続・版を調べるか」、成功は対象の状態・失敗理由を読んで操作と結果を追えること。スマホでは対象 → 状態・根拠 → 操作を優先し、失敗時は再取得または権限の案内から復帰する。基盤は既存の Celeris token と共通部品を維持。巨大 hero や統計カードの反復はなく、組織の継承、手順書の配布先、稼働版と昇格判断という固有の情報構造を保っている。

### 撮影と比較の所在

- pre の実体: `/local/celeris/data/workspaces/01M44FP86J0JAAKENF8SGBZ36B/repos/artifacts/qa-qa-admin-pre/` と同階層の `qa-qa-admin-pre-states/`。
- post の成果物ルート（以下 `ART`）: `/local/celeris/data/workspaces/01M44FP86J0JAAKENF8SGBZ36B/wu/post-record/artifacts`。作業場所の指定を優先した。`$(git rev-parse --show-toplevel)/../artifacts` はこの配置では `wu/post-record/repos/artifacts` を指すため使用していない。
- 通常: `ART/qa-qa-admin-post/`、124 PNG（31 unique fixture × 4 幅）。script の表示は 32 台帳行 × 4 = 128 回だが、`/org/cos` を共有する行は同名へ上書きされる。pre も実ファイルは 124 枚。
- 状態別: `ART/qa-qa-admin-post-states/`、116 PNG（8 状態の計 29 経路 × 4 幅）。pre も 116 枚。
- 補足: `ART/qa-qa-admin-post-supplement/`、24 PNG。既存 admin spec の正常な組織 fixture、下方の設定欄、login の失敗後、503 確定後を撮影した。再現スクリプトは `ART/supplement.mjs` と `ART/error-state.mjs`、実測は `ART/supplement-checks.json`。
- すべて 360 / 390 / 412 / 1440 CSS px、viewport 高さ 800、ローカル Chromium・偽 daemon・loopback gateway。外部ネットワークは使っていない。通常 script は描画完了を待たず撮るので、通常画像と操作後の補足画像は同一状態と見なさない。

以下の `pre/`・`post/` は上記の通常撮影ディレクトリ、`補足/` は post-supplement を指す。`{360,1440}` 等は各幅の実ファイル名に展開する表記。

| 指摘 | 再判定と画像で確認した結果 | pre → post の screenshot |
| --- | --- | --- |
| 1 | **[修正済み]** 英語単独の主見出し・一覧見出しを「手順書（skills）」「手順書の一覧」に変更。組織の課へ付けると作業場所へ配られる関係が読める。括弧の skills は台帳互換の補足として残る。 | `pre/_knowledge_skills-{360,1440}.png` → `post/_knowledge_skills-{360,1440}.png`。タブは `pre/_knowledge-390.png` → `post/_knowledge-390.png`。 |
| 2 | **[修正済み] 本文**は「実行管理」「保存履歴の照合」「最終取得」となり、何を確かめる操作か読める。**[未解決・理由] h1・nav の daemon** は parity 固定と shell が範囲外のため残る。全体を日本語化済みとは判定しない。 | `pre/_daemon-{360,1440}.png` → `post/_daemon-{360,1440}.png`。post の版「取得中」は撮影時点の過渡状態であり、取得結果の退行とは断定しない。 |
| 3 | **[修正済み]** 360・390px の右に隠れた問題欄が、版・状態の直後、昇格ボタンの前に移った。412px も同じ順。1440px は比較用の表を維持。fixture の問題は「なし」であり、長い失敗文までこの通常画像で検証したわけではない。 | `pre/_releases-{360,390,1440}.png` → `post/_releases-{360,390,1440}.png`、追加確認 `post/_releases-412.png`。 |
| 4 | **[修正済み]** 360px の切れた失敗理由列を縦の要約に変更し、接続状態・失敗理由・最終確認を同じ対象内に表示。host は「接続先」になった。1440px は表を維持。 | `pre/_clusters-{360,412,1440}.png` → `post/_clusters-{360,412,1440}.png`。 |
| 5 | **[修正済み] 主説明・フォームラベル**は短い運用文と「同時実行数」「モデル」「受ける段」に整理され、長い道具の説明は「用語の説明」へ折り畳まれた。設定カードに残る LLM source / llm_source 等の診断語と常設編集欄の密度は残課題。 | `pre/_providers-{360,1440}.png` → `post/_providers-{360,1440}.png`。下方の設定全体は `補足/providers-form-390.png`。 |
| 6 | **[修正済み] 表示順**は正常系の補足で 木 → 担当の詳細 → 担当を追加、PC は木・追加が左、詳細が右。親部・継承元・配下の課も読める。**[未解決・理由] 通常 pre/post の正常系比較**は両方が取得失敗のため不可能。pre のソース読解と既存修正記録、post の配置実測を根拠とし、同条件の画像比較に成功したとは扱わない。 | `pre/_org-360.png` → `post/_org-360.png`（両方 error）。正常系は `補足/org-selected-{360,390,412,1440}.png`・`補足/org-detail-360.png`。 |
| 7 | **[修正済み]** 候補の「New knowledge」が二重に出る状態を解消。見出しのみの本文はその旨を短く表示し、取り込み先と採用・却下に進める。ページ本文側の共通処理は先行葉の unit で確認済みで、今回の画像比較は候補画面を根拠とする。 | `pre/_knowledge_inbox-{360,1440}.png` → `post/_knowledge_inbox-{360,1440}.png`。追加確認 `post/_knowledge_inbox-412.png`。 |
| 8 | **[修正済み] 指摘したラベル・節見出し**は adapter →「道具」、secret →「秘密の値（secret）」。状態を読んでから操作へ進む構造と秘密値の非表示は保たれる。**[未解決・理由] secret の操作名**は parity が固定するため残り、説明文にも form 等が残る。 | `pre/_accounts-{360,1440}.png` → `post/_accounts-{360,1440}.png`。下部は `補足/accounts-secret-412.png`。 |
| 9 | **[修正済み]** 未選択時の空の本文枠が消え、検索結果の一覧と選択案内に幅を使う。1 件しかないため下の空間は残るが、内容の無い詳細パネルを主表示にしなくなった。 | `pre/_knowledge-{390,1440}.png` → `post/_knowledge-{390,1440}.png`。同様の手順書一覧は指摘 1 の画像。 |
| 10 | **[修正済み]** 初期状態で「ログイン後はホームを開きます」と表示。補足では next を含む戻り先が失敗後も残り、理由の alert とパスワード欄の focus が同時に見える。 | `pre/_login-{360,412}.png` → `post/_login-{360,412}.png`。失敗後は `補足/login-next-error-{360,390,412,1440}.png`（pre の失敗後画像は無し）。 |

help は今回の修正対象指摘がなく、`pre/_help-390.png` と `post/_help-390.png` で目次の折り返しと本文の順を比較し、`post/_help-1440.png` で読み幅を確認した。新たな装飾カードの反復はない。組織の人は `post/_org_cos-390.png` で会話・入力を確認したが、宛先の人名化は共通 Console と表示契約の課題として残る。

### 状態・accessibility・互換性

- `loading-_providers-360.png`（pre-states → post-states）は shell と Skeleton を維持。5 秒後の遅延案内までをこの画像の根拠にはしない。
- `error-_providers-390.png`（pre-states → post-states）は両方まだ Skeleton。`screenshots.mjs` が `page.goto` 直後に撮るため、ファイル名だけでは 503 の表示を証明できない。補足 `providers-error-ready-{360,390,412,1440}.png` は「再試行」の出現を待ち、取得不能の対象・影響・次の操作、focus 輪郭が見えることを確認した。
- `stale-_providers-412.png`（pre-states → post-states）は「再接続中」を示しながら既存一覧を残す。初期接続失敗の fixture なので、接続済みから切断した後の最終取得時刻や操作制限を証明するものではない。
- `forbidden-_providers-1440.png`（pre-states → post-states）は権限が無い対象、管理者への確認案内、「一覧へ戻る」を表示し、編集フォームは出ない。
- long-text / long-id / empty / many は撮影済みだが `web/e2e/support/states.ts` に admin 経路がない。admin 全画面の 8 状態を検証済みとはしない。組織の長い名称・ID は補足の既存 admin fixture で確認した。
- touch target と横溢れは前節の post-gates（31 経路 × 4 幅、違反 0）を根拠とする。この葉は正常な組織の補足で axe critical / serious 0 を各 4 幅で再確認。login 失敗後の focus 復帰も 4 幅で実測した。画像で focus の輪郭・入力境界・文字の状態ラベルを確認したが、静止画像だけで全 keyboard 操作や全状態の contrast を保証しない。
- parity 4 spec を期待変更なしで実行し、**15 passed / 2 skipped、exit 0**。skip は `WEB_SHOTS_OUT` 未指定時の org/help の撮影専用ケースで、機能の失敗ではない。固定された h1、accessible name、URL（組織の旧 URL 転送、選択 query、help アンカー、設定操作）を維持している。指摘 1 の skills の h1 は先行修正で `skills` → `手順書（skills）` に変わっており、parity は文言を固定していないため通る。「全 h1 文字列が pre と同一」という意味ではない。

この葉での実行（すべて exit 0。install は先行 gate のローカル store を作業ツリーへ複製後、reused 278・downloaded 0 で成功）:

```sh
corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile
corepack pnpm@12.6.0 -C web build
ART=/local/celeris/data/workspaces/01M44FP86J0JAAKENF8SGBZ36B/wu/post-record/artifacts
corepack pnpm@12.6.0 -C web screenshots --out "$ART/qa-qa-admin-post"
corepack pnpm@12.6.0 -C web screenshots --out "$ART/qa-qa-admin-post-states" --states
corepack pnpm@12.6.0 -C web e2e e2e/parity/org.spec.ts e2e/parity/knowledge.spec.ts e2e/parity/help.spec.ts e2e/parity/ops.spec.ts
node "$ART/supplement.mjs"
node "$ART/error-state.mjs"
```

ログは `ART/build.log`・`screenshots.log`・`screenshots-states.log`・`parity.log`・`supplement.log`・`error-state.log`。build は既存の chunk size warning のみ。今回の追跡差分は本記録だけで、API 型・gateway・crates・DB schema・parity の期待を変更していない。

## 残課題

1. **admin の状態カバレッジと撮影待機（共通 QA）**: `web/e2e/support/states.ts` は admin では providers の loading/error/stale/forbidden のみ。全 admin の長文・長 ID・0 件・多数を追加し、`web/scripts/screenshots.mjs` で非 loading 状態の成立を待つ必要がある。通常 `/org` の fixture も取得失敗なので、今回の補足と同等の正常系を共通 fixture に反映する必要がある。これらはこの葉のコード変更範囲外。
2. **共通 shell・表示契約の日本語化（指摘 2、1・8 の互換性部分）**: daemon の h1/nav、secret の accessible name、skill 削除名、skills の台帳名は parity・shell・画面台帳と合わせて変更する必要がある。本タスクでは期待を書き換えないため残した。共通 StatusBadge の設定語彙も今回の画面修正だけでは統一できない。
3. **診断語と常設フォームの密度（指摘 5・8 の残り）**: providers の `LLM source`・`llm_source`、accounts の `run`・`form` 等はまだ見える。画面の主ラベルは改善したが、用語の整理は完了していない。設定欄・新規追加欄の折り畳みは別の画面変更として keyboard 順と一緒に検証する必要があり、コードを変えない post-record では扱わない。
4. **多数の担当を選んだ後の移動（指摘 6）**: 詳細は追加より先になったが、長い木の下まで scroll が必要。keyboard で木を辿る操作を邪魔する自動 focus 移動は今回追加していない。多数 fixture と、利用者が選べる「選択した担当へ」等の導線を別途評価する必要がある。
5. **UX_AUDIT の範囲外の導線**: `/org/cos` の宛先は ID のままで、人名・役割を示すには共通 Console と h1 契約の見直しが要る。help の状況別検索、知識候補件数・既存本文との比較も今回の 10 指摘の修正では解消していない。API/schema 変更が必須との根拠は今回得ていないため、必要性を断定せず次の調査項目とする。

この記録の done は、critique → 画面修正 → post の再評価を一往復完了し、検証できない範囲と未解決事項を明示したことを表す。全画面・全状態の visual QA 完了や、本番への反映を意味しない。
