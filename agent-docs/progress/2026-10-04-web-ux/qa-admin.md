---
title: admin 画面群（組織・知識・help・login・accounts・providers・clusters・daemon・releases）の visual QA
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: running
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

4 つの修正葉を統合した worktree で指定順の検査を再実行した。2026-10-04 の再試行でも offline install が依存キャッシュ不足で exit 1 になった。前回 run 後に Celeris が実行した同じ check は `check:secrets` まで進んだが、最後の全画面 `mobile-audit` が exit 1 だった。監査だけを独立して再実行して違反箇所を特定した。検査全体を通過したとは扱わない。

| 検査 | 結果 |
| --- | --- |
| `install --offline --frozen-lockfile` | exit 1。初回は `react-remove-scroll@2.7.2`、再試行は `qs@6.16.0` の tarball が worktree の store に無い（`ERR_PNPM_NO_OFFLINE_TARBALL`） |
| `install --offline --frozen-lockfile --store-dir /local/.pnpm-store`（ローカル store の切り分け） | exit 1。共有 store に `@tailwindcss/vite@4.3.3` の tarball が無い（同じエラー） |
| `build` | 未実施（offline install 失敗、`web/node_modules` 無し） |
| `typecheck` | 未実施（同上） |
| `lint` | 未実施（同上） |
| `test`（vitest + node:test） | 未実施（同上。test 数は未計測） |
| `check:parity` | 未実施（同上） |
| `check:boundaries` | 未実施（同上） |
| `check:secrets` | 未実施（同上） |
| `mobile-audit` | 単独実行で exit 1。違反 21 件は `/projects/P1`・`/tasks/T1`・`/tasks/T1/changes` のみ。admin 対象 9 画面は 0 件 |
| `e2e`（functional scope） | 未実施（同上。件数は未計測） |

offline install の停止原因はローカル依存 store の欠落。担当範囲の画面ファイルを変更しても修復できず、offline 指定を外して取得することはこの検査条件と異なる。依存 tarball をローカル store に揃える必要がある。

さらに `mobile-audit` は `web/e2e/support/screens.ts` の全 fixture を固定で走査するため、admin 対象外の `/projects`・`/tasks` の違反でも全体 check が落ちる。`web/scripts/mobile-audit.mjs:65` は `<input>` の `labels` だけを読み、親 `<label>` で名前が付いた `<textarea>` を未命名と誤判定する。`/tasks/T1` の `integration repair` リンクは 17×44px で、これは実際の小さいタップ領域である。修正には `web/scripts/mobile-audit.mjs` と `web/features/tasks/overview-view.tsx` の変更が必要だが、両方ともこの葉の許可範囲外。詳細ログは WU artifacts の `install.log`・`install-local-store.log`・`mobile-audit.log` にある。最終受け入れ条件も全画面の監査なので、計画には監査コードの誤判定と task 画面のタップ領域を先に直せる担当範囲が必要である。
