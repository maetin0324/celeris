# ADR 2026-10-17: campaign lock — 測定の入力・grader・試験・実行構成を版付きで固定する共通契約

- 日付: 2026-10-17
- 状態: 採用・実装済み（lock.rs の型・hash・検証、lock_tree.rs の内容証明、K2 の hash 固定、契約試験 campaign_lock.rs。実行系への配線は未着手: D7）
- 関連: 2026-10-17-mlsys-pilot2-fixes（外部入力 D1）、2026-10-15-mlsys-min-campaign、2026-10-11-breaking-changes-by-default

## 背景
campaign hash（`CampaignManifest::compute_sha256`）は problem id・材料 hash・設定を縛るが、初期 tree の展開済み内容、
submodule 実体、外部入力、grader 定義、公開/隠しテストの実体は manifest の外にあり、実行・再開・grade-out・bind・export の
どこでも同じ物を使った保証が無い。後続の WorkUnit が同じ契約で照合できるよう、型と検証 API を先に固定する。

## 決定
### D1 `CampaignLock` は 1 つの版付き記録で、campaign hash に結び付く
- 欄: `schema`, `version`（lock の版）, `measurement_id`, `campaign_sha256`（manifest の `compute_sha256`）, `materials_sha256`, `kb_snapshot`,
  `cases[]`（採用 case id・split・初期 commit/tree/展開済み内容 hash・submodule の path/commit/内容 hash・外部入力・grader 定義 hash・
  公開/隠しテスト実体 hash）, `execution`（configs・budget・termination・order・seed・repeats）, `sha256`（`lock_hash`）。
- 既存の部品を再利用する: 外部入力は `record::DeliveredInput`、展開済み内容 hash は `snapshot::hash_tree` の値、split は `manifest::Split`、
  正規化 JSON は manifest と同じ canonical（キー順固定）。新しい hash 方式は作らない。
- `lock_hash` = 正規化（cases・submodule・入力・テストを整列）した lock の canonical JSON の sha256（`sha256` 欄は空として数える）。
  `campaign_sha256` を含むので lock は campaign hash に結び付き、`bind_to_manifest` で manifest と再計算照合する。

### D2 未固定値は拒否する（fail-closed）
空文字、hex でない hash、長さの違う hash、空の測定 ID・版、case 0 件を拒否する。case id・submodule path・入力名・テスト path の重複、
絶対 path と `..` を含む path も拒否する。
実装を含む case（`stages` に implementation）は公開テストと隠しテストの実体 hash をそれぞれ 1 件以上持つ。空一覧で済ませない。
公開試験が実装内（初期ソース）にあるなら、初期ソースの該当 file を公開テストとして固定する。

### D3 `verify_lock(expected, actual)`
両方を構造検査し、各 `sha256` が内容と一致するか再計算して照合したうえで、版違い・測定 ID 違い・campaign hash 違い・
資料/KB 違い・case 集合違い・初期 commit/tree/内容 hash 違い・submodule の欠落/差替え/追加・入力・grader・テスト実体・実行構成の
違いを区別して `LockError` で返す。`expected` は信頼できる側（commit/tree/gitlink から導いた物）で、渡された repo を
その場で期待値に採用しない。git metadata の有無は lock の欄にしない（metadata 無しの archive でも内容 hash で同一性を言える）。

### D4 内容変更は新しい測定 ID と版を要求する
同じ `measurement_id` と `version` で内容が違う lock は `ContentChanged` で拒否する。旧 lock・旧 grader 条件を自動更新する API は置かない。
`check_successor(old, new)` は、新 lock が別の測定 ID であること（かつ内容が同一でないこと）だけを許す。

### D5 初期 tree / submodule の内容証明（`lock_tree.rs`）
- 内容 hash = file 一覧（path 昇順）× mode（100644 / 100755 / 120000）× blob 内容 sha256 の正規化列の sha256。`snapshot::hash_tree` は mode を見ず
  git 由来と照合できないため、この用途だけ別の正規化にした（`snapshot::hash_tree` の挙動は変えない）。空 dir・owner/mtime・root の `.git` は数えない。
- 期待値は commit/tree/gitlink からだけ導く: `derive_initial(repo, rev, submodule_repo)`。submodule 実体の repo が無ければ `TreeProofError::Unavailable`
  で、成功扱いにも skip 合格にもしない。入れ子の submodule は `Unsupported`。
- 照合: `verify_repo(expected, repo)`（commit・tree・内容・gitlink）、`verify_archive(expected, dir)`（`.git` の有無を問わず本体と各 submodule を別に照合）。
  submodule の欠落・差替え・追加・1 byte 修正、本体の修正・余分な file は拒否する。metadata の有無自体は拒否理由にしない。

### D6 K2（k2-bf-xattr-restart-campaign）の固定
- 人が承認し運用者が配置した資産 `/local/celeris/data/campaign-assets/k2/`（ローカル・読み取り専用、`SHA256SUMS` 検証済み）から初期 tree を組み、
  `crates/campaign/cases/final/k2-bf-xattr-restart-campaign/initial.lock.json`（`LockedInitial`）に固定した。tree は case.json の `56d2f021…` と一致。
- 旧 `k2-initial.tar` の sha256 は再現できない（tar の sha256 は pack 方法に依存）。今回の内容 hash を正とする（本測定の出力はまだ無い）。
- 資産が無い host: `tests/k2_pin.rs` は失敗する（skip 合格にしない）。`tests/campaign_lock.rs` の K2 archive 照合は資産が無ければ失敗にせず
  k2_pin.rs に任せる（その旨を stderr に出す）。lock 構造としての検証（`LockedInitial` を case に入れて封印・照合）は資産なしでも常に走る。

### D7 後続への API と未着手の配線
- 後続が使う API: `CampaignLock::{new, validate, normalized, lock_hash, seal, bind_to_manifest}`、`LockedCase::from_declared`、
  `verify_lock(expected, actual)`、`check_successor(old, new)`、`lock_tree::{derive_initial, verify_repo, verify_archive, archive_content_sha256}`。
- 契約試験: `crates/campaign/tests/campaign_lock.rs`（公開 API のみ）。
- **未着手**: 実行・再開・grade-out・bind・export から lock を読んで照合する配線、lock ファイルの配置（例 `<out>/snapshot/lock.json`）、
  全 case の LockedCase を実体から組む CLI。いずれも後続の WorkUnit で行う。この ADR 時点では型・hash・照合 API と試験までが済んでいる。
