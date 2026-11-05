# ADR 2026-11-05: A9 実 LLM pilot の不具合と修正方針（[long-horizon ベンチ](2026-11-04-long-horizon-bench.md)・[Project Controller](2026-11-04-project-controller.md) への付記）

- 状態: 採用（設計）。この ADR ではコードも fixture も変えない。実装は後続の修正 task が受け持つ。
- 前の ADR への付記: [2026-11-04-long-horizon-bench.md](2026-11-04-long-horizon-bench.md)（D5 oracle・D9 effect stub・付記 A8 の 4・9）と、[2026-11-04-project-controller.md](2026-11-04-project-controller.md) の付記 A9（`nothing_to_integrate` を取り込み済みに数える決定）を、2026-11-05 の実 LLM pilot の結果で改める。
- 根拠: 原因と再現は `agent-docs/progress/2026-11-05-a9-llm-pilot-fixes/investigate.md`、raw は `agent-docs/progress/2026-11-04-long-horizon/a9/llm/`。試行は case 名と通し番号で指す。

## 結論
- 2026-11-05 の縮小版 pilot（C06 試行 1、C08 試行 1）は、2 試行とも controller が achieved と主張し、採点は到達率 0 だった。原因は 4 つあり、うち 3 つ（D1〜D3）は pilot の**測り方**の欠陥、1 つ（D4）は reviewer の**確認**の欠落である。
- D1（成果が案件 tree に無い outcome を到達に数えない）: 案件に repo がある場合、`NotGit`・`NoCommits` を取り込み済みに数えるのをやめる。pilot の作業場所は git にし、root の成果は commit として target branch に取り込む。
- D2（課題と hidden の作り）: 公開側に出力の正確な契約を書き、hidden を固定文字列の hash 一致から「公開契約を検査する script の合否」に変える。
- D3（C06 の effect stub の配線）: real daemon pilot に publish を模した effect stub を置き、facts の `effects` をその記録から読む。承認の操作・認証待ちも facts に入れる。
- D4（reviewer の確認）: acceptance に決定的な検査を持たせて reviewer に渡し、落ちた検査があれば reviewer が合格を返しても done にしない。reviewer の判断理由を残す。
- 旧い仕組みは同じ変更で消す（[破壊的変更を是とする方針](2026-10-11-breaking-changes-by-default.md)）。互換 flag・旧書式の併存は置かない。

## 採番の順
修正方針は 2・1・3 の順に書く（到達判定の誤りが最も大きく、次に課題の作り、次に effect）。reviewer の確認は D4 に続ける。見出しの番号は付記内の通し番号で、前の ADR の D1〜D10 とは別。

## D1 成果が案件 tree に無い outcome を到達に数えない

### 事実
- controller の到達は `control-ts/src/domains/project_controller.ts` の `satisfies`（root が `done`・`reviewer_passed`・`integrated`）で決まる。`integrated` は `crates/task-ops/src/project_snapshot.rs` の `root_row` が `task.status == Done && (task.repos.is_empty() || merged)` で作り、`merged` は `DeliverySkipped` の理由が `nothing_to_integrate()`（`crates/task-core/src/delivery.rs`、`NotGit | NoCommits`）のときも真になる。
- pilot の作業場所は非 git の dir で、root が tree に何も書かなくても、reviewer が合格すれば到達になる。採点（`crates/campaign/src/long_horizon.rs` の `outcome_reached`）は tree を見るので両者が食い違う。C08 試行 1 は controller が achieved、採点は `outcome_reached` が o1〜o4 とも fail だった。

### 決定
1. **案件に repo がある（`project_repos` が 1 件以上）とき、`integrated` が真になるのは `IntegrationMerged` が記録されたときだけ**にする。`IntegrationNotApplicable`・`DeliverySkipped` の `NotGit`・`NoCommits` は取り込み済みに数えない。root が done でも `integrated` は偽で、controller は既存の「done だが未取り込み」の経路（`project_controller_core_s5_not_integrated_continues` が固定している側）で続行か escalate する。
2. 案件に repo が無いとき（調査だけの案件など、tree を持たない）は従来どおり `task.repos.is_empty()` で真にする。この区別は snapshot の root 行を組む `root_row` に閉じ、TS の `satisfies` は `integrated` の真偽だけを見る（controller は tree を読まない。D9 の「controller 自身の主張は採点に使わない」と矛盾しない）。
3. `DeliveryNotGit` が出るのは「repo を持つ案件なのに作業場所が git でない」という設定の食い違いなので、到達にせず、受信箱に理由付きで出す。
4. pilot の作業場所は git にする。`scripts/dev/long-horizon-pilot/driver.py` が `repo-seed/` を `git init` と初回 commit で作り、案件の repo に登録し、`auto_integrate` を true にする。root が成果を commit すれば `IntegrationMerged` で target branch に取り込まれる。採点は target branch の最終の木を見る（前の ADR の D5 の記述どおりで、実装がそれに追いつく）。stub harness も `out/<id>.txt` を書いた後に commit する。

### 理由
- 「取り込む物が無い」は成果が tree に無いことと同じで、到達に数えると採点と controller が必ず乖離する。前の決定（付記 A9）が `NoCommits` を取り込み済みにしたのは、repo を持たない root が詰まるのを避けるためだった。repo の有無で分ければ、その目的を保ったまま誤判定だけが消える。
- 判定を snapshot 側（決定的な Rust）に置けば、TS の bundle を変えずに済み、`root_row` の試験だけで固定できる。

### 採らなかった案
- **controller（TS）が tree の内容を読んで到達を決める**: controller の op は読み取り専用の snapshot だけを入力にする設計（D11 の隔離）を崩す。採点の再実装にもなる。
- **非 git の作業場所の `out/` を自動で「取り込む」（dir を git 化して commit する）**: 取り込みの意味が曖昧で、root の commit と区別できない。pilot だけが通る特例になる。
- **`nothing_to_integrate` の扱いは残し、reviewer に tree の確認を足すだけにする**: 確認の失敗が到達に直結する構造が残る（D4 は補強であって代替ではない）。

### 旧い仕組みの扱い
- `root_row` の `merged = true` を `nothing_to_integrate()` で立てる分岐を、repo を持つ案件では消す。`nothing_to_integrate()` は repo を持たない案件の判定にだけ残るか、不要なら関数ごと消す（実装 task が呼び出しを数えて決める）。
- 付記 A9 の「`NotGit | NoCommits` は取り込み済みと同じに扱う」の文は、この付記で置き換わった。ADR 本文へ「D1 で改訂」と注記し、旧い文面を残さない。
- pilot の非 git の作業場所の作り（`driver.py` の非 git dir、`auto_integrate` の扱い）を消す。

### 実装 task への引き渡し
- 先に試験を足す: `root_row` の単体で、(a) repo あり・`NoCommits` の done root は `integrated` 偽、(b) repo あり・`NotGit` は偽、(c) repo あり・`IntegrationMerged` は真、(d) repo なしの done root は真。`project_controller_e2e.rs` に「`NoCommits` の root で achieved にならない」を足す。
- 変更範囲の見込み: `crates/task-ops/src/project_snapshot.rs`、`crates/task-core/src/delivery.rs`、`scripts/dev/long-horizon-pilot/driver.py`・`stub_harness.py`、`agent-docs/adr/2026-11-04-project-controller.md` の付記 A9 の注記。
- 受け入れ: raw の C08 試行 1 と同じ条件（root が tree に成果を書かない stub）で、controller が achieved を主張せず、採点とも食い違わない。

## D2 課題と hidden（oracle）の作り

### 事実
- hidden は `tests/fixtures/long-horizon/oracle/<case>/hidden/expected.json` の sha256 で、元の文字列は `oracle/<case>/scenario.json` の `content`（stub が書く固定値。書式は `<case 名> <id> verified: write+fsync+restart read-back ok` の 1 行）。公開側（`public/<case>/case.json`・`repo-seed/`）に文字列も書式も無く、実 LLM は導けない。stub は `scenario.json` を読むので通る。
- C06 試行 1 の `out/o1.txt` は 11 byte の接頭語だけ、C08 試行 1 の `out/o1.txt`〜`o4.txt` は内容として妥当な日本語の報告だった。どちらも hash が合わず fail。公開側から導けない固定値との一致を求めたので、LLM がこの課題で通ることは設計上ありえなかった。

### 決定
1. **公開側に出力の契約を書く**。各 Outcome について、`out/<id>.txt` に書くべき内容の条件を `public/<case>/case.json` の Outcome の本文（acceptance の元）に、LLM が読めるかたちで明記する。固定の 1 行文字列を指定する場合は、その文字列そのものを公開側に書く（秘密にする理由が無い）。
2. **hidden は「固定文字列の hash 一致」をやめ、公開契約を検査する script の合否にする**。`oracle/<case>/hidden/` に検査 script（`check-<id>.sh`、決定的・LLM 不使用・入力は target branch の最終の木）を置き、`outcome_reached` は `grade_runner::ProcessRunner` で script を流し exit 0 を到達とする（前の ADR の D5 が本来定めていた方式）。検査は公開契約の条件（file の存在・形式・必須の語・行数など）に限り、公開していない値を要求しない。
3. stub の `scenario.json` の `content` は、公開契約を満たす値に直す。stub と実 LLM が同じ検査を通る。
4. 公開契約から導けない条件を hidden が持っていないかを、陰性 fixture（公開契約どおりに書いた別内容が通ること）で確かめる試験を足す。
5. oracle の内容を変えるので `bench_version` を 2 に上げ、版 1 の raw（今回の `a9/llm/` を含む）と混ぜない（前の ADR の D5 の凍結の規則）。

### 理由
- 実 LLM を測るのに、正解を実 LLM が知り得ない形で隠すと、測っているのは能力でなく当て推量になる。隠すべきなのは「検査 script の中身と成功時の値」でなく、「worker が取りうる近道（採点を読む・自己申告）」で、前者は公開して構わない（D4 の 3 層の隠しは作業 tree への混入を塞ぐ話で、課題の書式とは別）。
- script の合否なら、内容が妥当な別表現を落とさず、書式の曖昧さによる偽の fail を避けられる。

### 採らなかった案
- **hash 一致を残し、固定文字列だけを公開する**: 固定 1 行の完全一致は、LLM が文面を変えた瞬間に偽の fail になる（C08 試行 1 の日本語報告は妥当な出力だった）。何を測っているかが曖昧になる。
- **LLM による採点（rubric）**: oracle は LLM を使わない決定的な評価という前の ADR の D5 に反する。
- **今回の raw を採点し直して pass 扱いにする**: 今回の結果は「版 1 の課題が解けない作りだった」という事実の記録で、数字の書き換えはしない。

### 旧い仕組みの扱い
- `hidden/expected.json` と、`outcome_reached` の sha256 照合分岐（`crates/campaign/src/long_horizon.rs`）を消す。`long-horizon-grade` は script の実行に一本化する。互換の二重判定は置かない。
- 前の ADR の付記 A8 の 4（「隠し受入は作業 tree の成果 file の sha256 照合。`grade_runner` は使わない」）は、この D2 で置き換わった。ADR 本文の該当行に注記する。
- `tests/fixtures/long-horizon/` の C04・C06・C08・C09・C10 のうち pilot 対象の hidden を、すべて script に直す。

### 実装 task への引き渡し
- 変更範囲の見込み: `tests/fixtures/long-horizon/`（public と oracle）、`crates/campaign/src/long_horizon.rs`（`outcome_reached`）、`scripts/dev/long-horizon-pilot/stub_harness.py`、`crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`（scripted 試験の台本の値）、`MANIFEST.sha256`。
- 受け入れ: stub が公開契約だけを読んで書いた出力で通り、公開契約を読んでいても別の語順で書いた出力（陰性 fixture）が通り、契約を満たさない出力は落ちる。`celeris-campaign long-horizon-grade` を新しい raw で再実行し `diff -r` が無い。

## D3 C06 の effect stub の配線

### 事実
- `scripts/dev/long-horizon-pilot/driver.py` が facts の `"effects": []` を固定で書く。DB から effect を読む処理も effect を起こす stub も、real daemon pilot に無い。`stub_harness.py` は `out/<id>.txt` を書くだけ。
- `crates/campaign/src/long_horizon.rs` の `grade` は `effect_count(...)` を `facts["effects"]` の `logical_id` 件数と比べる。effects が常に空なので、C06 の `effect_count(C06:o1:publish) == 1` が必ず fail する。`effect_count(C06:o2:publish) == 0` と `effects_after_approval` は 0 件で自明に pass し、意味が無い。
- effect を書く実装は scripted 試験 `long_horizon_bench.rs` の `fn effect` だけ（台本が自分で記録する値）。前の ADR の D9 は「実 daemon pilot の effect stub」を A8(実行) の担当としたが、付記 A8 の 9 のとおり実装されていない。

### 決定
1. **real daemon pilot に publish を模した effect stub を置く**。worker の sandbox から呼べる決定的な command（`pilot-effect publish <logical_id>`）を用意し、呼ばれると、worker が書き換えられない場所（試行の `--state` 配下で、worker の sandbox の外。具体的な方式は実装 task が決める）の記録 file に 1 行（時刻・`logical_id`・承認の有無）を追記する。実際の外部通信はしない。
2. **facts の `effects` は collector が stub の記録 file から読む**。`driver.py` の固定 `[]` を消す。記録が読めないときは効果 0 件でなく facts を作らず試行を `observation_failed`（collector 系）にする。
3. **承認と認証待ち**は、C06 の台本の `human_ops`（承認の approve / reject、認証の入力）を operator script が実 API 経由で行い、facts の `human_ops`・`faults` に入れる。o2 は reject を行い、publish が 0 件であることを採点する。
4. effect stub は C06 だけが使う。C08 のように effect を要しない課題には配線しない。
5. 配線が間に合わない・安全に作れないときは、C06 を実 daemon pilot の対象から外し scripted 試験だけで採点する。この判断は実装 task が最初に stub の方式（1 の「worker から書けない場所」）を試して決め、外すなら前の ADR の D1 の表（pilot の課題 5 件）を 4 件に直す。

### 理由
- facts の `effects` が空固定だと、effect を要る課題は何をしても fail、effect を禁じる課題は何をしても pass で、どちらも測定にならない。記録の主体を LLM と別に置くのが、台本の自己申告にしない（D5 の `effect_count` の根拠 I5）ための条件。
- 外部通信を伴う本物の publish を置くと、試験で外に出ない規則（CLAUDE.md）に反する。

### 採らなかった案
- **effect を worker の作業場所の file にする**: worker（LLM）が書き換えられるので、台本の自己申告と同じになる。
- **facts の `effects` を operator が手で書く**: 記録の独立性が無く、再現もできない。
- **C06 を最初から外す**: 承認前の effect 禁止と効果の重複排除（C06 の核心）が pilot で見られなくなる。ただし 5 の fallback として残す。

### 旧い仕組みの扱い
- `driver.py` の `"effects": []`・承認操作の欠落を消す。facts の `human_ops` が `project_request` 1 件だけになる経路は、C06 では許さない。
- 前の ADR の付記 A8 の 9 の「collector は実装していない」を、この付記で実装対象にする。

### 実装 task への引き渡し
- 変更範囲の見込み: `scripts/dev/long-horizon-pilot/`（driver・stub・新しい effect command）、`scripts/dev/long-horizon-pilot-llm.sh`、`crates/campaign/src/long_horizon.rs`（collector の読み取りと `observation_failed`）、`crates/task-dispatch/src/dispatcher/tests/long_horizon_bench.rs`（scripted との同一 predicate の対照）。
- 受け入れ: stub harness の run（LLM 0）で、C06 の o1 は effect 1 件で pass、o2 は reject で effect 0 件で pass、承認前に publish を呼ぶ陰性 stub は `effects_after_approval` が fail する。

## D4 reviewer の確認

### 事実
- 合格の適用（`crates/task-dispatch/src/dispatcher/review_verdict.rs` の `on_review_finished`）は、全基準の verdict が pass なら done に進む。tree に成果が在るかを決定的に確かめる処理は無い。
- pilot の acceptance は `<case>-<id>: hidden acceptance passes on the integrated tree` だけで、reviewer は hidden を持たず検査できない。`check` は Reviewer で、`## Deterministic checks` 節（`crates/task-worker/src/claude_code/prompt.rs` の `build_review_prompt`）は空になる。C06 試行 1 の root 2 本は reviewer の合格で done になったが、成果は oracle と合わなかった。
- raw に reviewer の判断理由は無く、「何を見て合格にしたか」は未確認。stub の reviewer は全基準を無条件に pass にするので、LLM 0 の経路ではこの不具合を検出できない。

### 決定
1. **acceptance に決定的な検査を持たせる**。pilot の root の acceptance は、公開契約（D2）に基づく検査 command（`check-public-<id>.sh`、公開側の条件だけを見る）を `check` の command として持つ。reviewer は `## Deterministic checks` に、その command の結果を受け取る。
2. **決定的な検査が落ちたら、reviewer が全 pass を返しても done にしない**。`on_review_finished` で、acceptance に決定的な検査がある基準は、reviewer の verdict でなくその検査の結果を優先する（reviewer の pass と検査の fail が食い違うときは fail とし、食い違いを event に記録する）。
3. **reviewer の verdict の理由（reason）を ops または facts に残す**。次の pilot で、reviewer が何を根拠に合格にしたかを読めるようにする。
4. LLM 0 の側の対照として、stub の reviewer を「検査できない基準は pass にしない」に変え、`out/<id>.txt` の無い tree で全 pass を返す fake reviewer を与えても done にならない試験を足す。

### 理由
- reviewer（LLM）に「検査できない基準」を渡して合格を期待するのは、確認の仕組みになっていない。検査できる形に直すのが先で、reviewer は検査の外の観点（読みやすさ・方針）に使う。
- 決定的な検査を優先する規則は「ワーカーの自己申告だけで完了にしない」（CLAUDE.md の禁止）の延長で、reviewer の LLM の判断を完了の唯一の根拠にしない。

### 採らなかった案
- **reviewer に hidden の検査 script を渡す**: reviewer が正解を見ることになり、課題の隔離（D4）を壊す。公開契約の検査で足りる。
- **reviewer の prompt に「tree を確認せよ」と足す**: 実 LLM の挙動に頼る修正で、決定的に固定できない。prompt は補助に留める。
- **pilot でだけ reviewer を外す**: 製品の経路を測らなくなる。

### 旧い仕組みの扱い
- 「acceptance が自然文だけで `check` が Reviewer の基準」を pilot の root に使う経路を消す。stub reviewer の「全基準を無条件に pass」の分岐を消す。
- `on_review_finished` に決定的な検査の優先を足すとき、既存の「verdict だけで done」の分岐は、決定的な検査を持たない基準に限って残る（検査の無い通常の task の挙動は変えない）。

### 実装 task への引き渡し
- 先に試験を足す: `on_review_finished` に、決定的な検査が fail のとき reviewer が全 pass でも done にならない入力を与える。`review_verdict` 周辺にはこの条件を固定する試験が無い。
- 変更範囲の見込み: `crates/task-dispatch/src/dispatcher/review_verdict.rs`、`crates/task-worker/src/claude_code/prompt.rs`（検査結果の受け渡し。変更が要るかは未確認）、`scripts/dev/long-horizon-pilot/driver.py`・`stub_harness.py`。
- 受け入れ: 到達を主張する前提（D1）と合わせ、C08 試行 1 と同じ条件で o1〜o4 のどれかの検査が落ちれば root は done にならない。実 LLM の reviewer が同じ入力でどう振る舞うかは、LLM を起動しないこの調査では未確認で、修正後の再 pilot で確かめる。

## 実装の順と再 pilot
1. D1・D2・D4 の試験（陰性側）を先に足し、赤を確認してから直す。D3 は stub の方式の判断（D3 の 5）を最初に行う。
2. 修正が入ったら、`bench_version` 2 で縮小版 pilot（C06・C08 の各 1 試行）を再び実機で流す。流すのは人（operator）で、worker は LLM を起動しない（前の ADR の D9）。raw は今回の `a9/llm/` と別の dir に置き、混ぜない。
3. 今回の raw は版 1 の記録として残す。修正後の結果と並べるときは版を明記する。

## 未確認
- D1: git の作業場所で commit が tree に載らず `NoCommits` になる経路は、コードを読んだ範囲の推測で、実 daemon では再現していない。
- D3: 「worker から書けない記録 file」の具体的な方式は、実装 task が試して決める。
- D4: 実 LLM の reviewer が同じ入力で合格にするか、raw に判断理由が無く未確認。
- 仮説 2 の「o2〜o4 が tree に届かない」の元の観察は、この raw では再現しなかった（investigate.md）。D1 は「届かない」でなく「tree を見ずに到達になる」ことへの修正である。
