# [アーキv4 親] 責務境界と疎結合性を実測で改善し、汎用の健全性検査で維持する（子 A〜H の取りまとめ）

created_at: 2026-11-17T14:50:43.197010878Z

依頼キー celeris-architecture-scalability-v4-20261117 の親。人の依頼（chat message 01M7MZJ152449WKHW0VET5ZE43、2026-11-17）の全文は下にある。
親は子 A〜H（依存 A→B→C→(D,E)→F→G→H）の完了を待つ。子がすべて done になってから、closeout だけを行う: H の artifacts/final-report.md の数値を A のベースラインのコマンドで再計測して一致を確かめ、統合後の release の gate の合否を確かめ、親の進捗 file を agent-docs/progress/ に commit する（子は親の branch を経ず main に入る）。親は自分で実装しない。未達の項目があれば done にせず、原因と追加の候補を task の質問で人に聞く。本番への昇格はしない（人だけ）。

## 共通の規則（CoS が依頼から整理した。依頼の全文は末尾）
- 基準は main bc665168c9caf1898adc4c611404bbee056daf9a（2026-11-17）。B〜H は A の成果（artifacts/baseline.md と A の進捗 file）にある計測コマンドと規則をそのまま使い、同じルールで before/after を測る。A と違う数え方をするなら理由と両方の値を書く。
- 共通受入条件 (1)〜(6) のうち、この子に関わる項目を before/after で記録する。前後比較できない項目は「未達/要判断」と書き、done と主張しない。未達のときは数値・理由・次の候補を書いて task の質問で人に聞く。
- 成果に数えないもの: コードの移動・改名・分割、Manager・trait・Port・Facade の追加だけで LoC・依存・影響範囲が減らない変更、分母の増加。self_improve 専用のロジック分岐を作らない。新しい framework や巨大な基盤を足さない。
- 安全不変条件を維持する: SQLite 正本・append-only events・lease/atomicity・replay・ACL・credential・CoS 監査・project controller の安全停止・selfdeploy の本番権限・Rust/TS canary。旧実装と新実装を並存させるなら撤去条件と期限を進捗に書く。
- policy の file（repo-health の profile・規則など）の変更は、既存の policy.changed の別 review（policy だけの branch、人の差分承認、operator の main への merge）を守る。自分の変更を自分で承認しない。
- 本番への昇格はしない（人だけが行う）。API・CLI・DB の契約を壊す変更は、実装の前に task の質問で人の承認を得る。
- 完了報告（進捗 file と artifacts）には、測定結果・再現コマンド・リリースゲート（統合後の release の gate の合否。未実施ならそう書く）・回帰条件を書く。
- 教訓（依頼の指示で全員に渡す）: 健全性v2 ではコードが約 2 万行純増して失敗した。v3 では本体 −11,239 行を実測で達成した。Worker の親 task の objective が先頭 1,500 字で切れる不具合があり、全文配信に直した。Worker も Reviewer もこの objective を末尾まで読んでから作業・判定する。
- 子は親の branch を経ず main に入る。進捗は agent-docs/progress/ に書く。

## 人の依頼の全文
依頼キー: celeris-architecture-scalability-v4-20261117。案件 agent-platform、project_id=01M2WTS3DKNZBSZ2JMVB4CZMBW。これは実タスク起票依頼。Celeris正規のCoS/タスク機構で親1件＋子A〜Hの8件を今すぐ登録し、Aをready/runningにしてタスクID・依存・状態を返す。同一依頼キーで重複起票しない。起票だけでなく実作業タスクを登録。危険な本番昇格や契約変更は人が承認。変更対象の競合を避ける。

目的：エージェントによる今後の機能追加・修正を、安全で速く局所的にするため、現存のCelerisアーキテクチャの実際の責務境界と疎結合性を改善し、通常のGitプロジェクト一般にも適用できる健全性検査で維持する。self_improve専用のロジック分岐は作らない。新しいFrameworkを丸ごと追加すること自体を成果としない。コード移動、Manager・trait・Port追加だけでLoC・依存・影響範囲が減らない場合は未達。安全不変条件と機能を維持したbefore/afterの実測を受入条件にする。

監査根拠：main bc665168c9caf1898adc4c611404bbee056daf9a（2026/11/17）。production全言語375099行、Rust330408行。Cargo依存49辺・通常依存にcrate循環0だが、汎用repo-healthでは既存モジュール循環17件、task-core内55単位SCC・210内部辺、task-api内49単位SCC・261内部辺。size.responsibility-growthの既存94件、dup.contract候補210件、arch-mapのowner不一致31件。Dispatcherはfieldを116→16にしたが、dispatcher/配下の非試験69ファイル中44ファイルがself.store（633出現）、35ファイルがself.config（218出現）を利用。task-core::SqliteStoreは約102 implブロック・pub method約700、直接/間接依存11 crate。HTTP handlerはcos::operationsを参照し、CoS opsがhandlers::orgを逆参照する循環。task-api/knowledge.rsがSQLite接続とIMMEDIATE txを直接持ち、task-ops/lifecycle.rsはraw Connectionを外部に渡す。10/28以降のproduction修正195件で約49%が複数領域へ変更波及。repo-healthのdeps.cycleは既存SCCへの新しい辺がnode countを変えない場合の悪化を見逃す可能性。#[path]でdispatcherの論理moduleをhealth_review dirへ置く6例があり、物理配置と論理所有者が食い違う。assess_riskは高リスクpath＋new_state/new_branch/new_fallback/new_portが主条件、既存の認可・lease・tx・event判定を変えるだけならLowになりうる。high_risk pathにtask-api/knowledge.rs・cos/operations.rs・dispatcher/worker_finish.rs・task-worker/browser_policy.rs等は現状含まれない。健全性v2でコード約2万行純増という失敗、v3で本体-11239行、Workerの親task objective先頭1500文字に切られた不具合の全文配信修正という教訓を各子・Worker・Reviewerへ必ず渡す。

共通受入条件：baseline SHAと同じルールでbefore/afterの(1)実際のimport/dependency/循環内辺、(2)直接共有状態・SQLite参照のある実装ファイル数、(3)互いのHTTP/CoS module importとSQL公開面、(4)変更波及のmodule/ファイル数、(5)incremental build・test-affected・review/health時間と費用、(6)productionとtest LoCの差分を記録。機能追加の通常タスクに一律LoC削減を強制しないが、単なる新しい抽象化の追加だけでは達成にしない。前後比較できない場合は未達/要判断。SQLite正本・append-only events・lease/atomicity・replay・ACL・credential・CoS監査・project controllerの安全停止・selfdeploy本番権限・Rust/TS canaryを維持。旧実装と新実装を並存させるなら撤去条件と期限を残す。政策変更は既存policy.changedの別reviewを守り、Worker・Reviewerの実プロンプトへの目的/受入条件伝達を検証。

子A：基準の確定・責務境界監査。HEADとソースを確認し、既存のmodule-deps-measure.py・repo-health・size-report・Git履歴とコンパイラの解析で既存循環と#[path]の真偽を確認。代表的な新機能/修正シナリオを2件固定（例：新CoS業務操作とWorkUnit終了処理の変更）。beforeの変更波及とビルド/テスト/レビュー時間を測り、監査済みの所有者・不変条件をリスト化。source改変0。Aの成果に各子が使うベースライン、計測スクリプト呼出し、誤検出の扱いを残す。

子B：Aの後。既存repo-healthのdeps.cycle/size/lang_depsを小さく修正して、既存SCCの内部にノード増加なしで新規辺を足したときも悪化を拒否する。#[path]実モジュールの所属を追跡し、ファイル移動によるサイズ上限・依存チェックのすり抜けを防ぐ。baseで既知の負債を一律failにせず差分で扱う。汎用Rust Git fixtureで新規内辺の拒否・既存内辺の保持・辺削除のpass・合法な型importを対照確認。Celeris向け特殊な検査器を新設せず既存を再利用。変更はpolicy-only reviewを守る。

子C：B後。現行のdesign risk/reviewer判定と規約を改善。新しいstate/branchが無くても、既存の認可・lease・transaction・event apply・Browser credential/ACL・Worker終了処理・selfdeploy安全境界の意味が変わる改修は高リスク設計reviewを要求。レビューが始まる根拠を具体的なdiff・owner・不変条件で説明でき、普通のUIや無関係な変更は過剰に止めない。上述の抜けた危険なpathのCeleris profileを補完。worker/reviewerの実promptへ新しい依存辺と安全条件を渡す。最低6ケース（認可条件1行変更高リスク、同等の普通変更低リスク、移動でのcheck回避不可能、悪化依存ブロック、policy変更の自己承認不可、失敗時fail-closed）を実証。新しい巨大なreviewer基盤を作らない。

子D：C後。HTTP/CoSの循環を実際に解消する狭いpilot。task-api handlers/org.rsとcos/ops/admin.rs周辺を選び、両者に混ざった業務ロジックを既存のtask-ops又はtask-api内の共有use-caseへ集約。HTTP AdapterとCoS Adapterから一方向に呼ぶ。AuditContextと業務更新とCoS操作記録の同一transaction、拒否理由、Authorization、response互換をテスト。handlersとcosの双方向依存が実際に減ることを測定。単なる別moduleへのコピーや新しい抽象Facadeの追加は不合格。

子E：C後、Dとwrite-set非衝突なら並行可。Dispatcherのpilotとしてwork_unitsとreview_verdict又はcos_chatの2領域で、共通self.store/self.configに触るファイルを減らし、既存のowner/Decision/Coordinationが提供する狭い操作だけを呼ばせる。現行非試験69ファイル、self.store参照44ファイル、self.config35ファイルを同じscopeで再計測し、目安としてstore参照5ファイル以上・config3ファイル以上純減。分母増加/別path移動/フィールド梱包を成果と数えない。余計なManager/traitを乱立させず、実際の状態変更の所有者が限定されることをコンパイル/依存/負テストで証明。lease・restart・stale review・parallel WU・replanの反例維持。達成できない時は無理に合わせずreplan。

子F：D/E後。業務処理と永続化の境界pilot。task-api/knowledge.rsのattachment pin・raw SQLite接続とtask-ops/lifecycle.rsのraw transactionを候補に、既存のSqliteStore::write_txや業務操作を使い、上位に漏れたConnection操作を減らす。認可・監査と同一transaction保証、ロールバック・ビジー時エラー・イベントを反例テスト。TaskStore巨大traitを再導入しない。featureごとに無数の新Portを増やすより使う側が必要とする狭い公開面を保持。public surfaceの縮小、直接SQLite参照の減少、何が削除されたかを報告。分離の妥当性が低ければ限定的に実施して理由を返す。

子G：F後。ビルド・変更局所性・反復速度の測定と必要な小規模調整。Aで固定した2シナリオをもとに、sourceの変更scope・影響するcrate数・test-affected runtime・Cargoのcold/warm incremental check/test・reviewer/health所要時間と費用を、同条件各3回の比較で評価。悪化するなら原因を絞って改善。task-coreのcrate分離は利益を測った場合だけ小範囲pilot、既存のドメイン実装を大量移動することは禁止。変更後の反復が遅くなっていたら未達/対策。

子H：G後。最後の統合/健全性/安全/実効性チェック。責務境界、循環内部辺、cross-boundary import、Storeの公開面、Dispatcherの参照、実際の変更波及、build/test/health/review cost、増減LoCを数値でまとめる。review/CIで危険な変更が確実に止まり、通常の変更は通ることを実際に確認。未達はdone扱いせず原因・追加候補を提示。各子と親の完了報告には測定結果/再現コマンド/リリースゲート/回帰条件が必要。自動本番昇格なし。

依存A→B→C→(D,E)→F→G→H。Aを今から進められるよう設定。完了予定をユーザーへ勝手に約束しない。親1＋子8のID、状態、依存を返してください。

