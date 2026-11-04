---
tasks: [01M6DCPKPJZZ9F9J4QJ47FH8KK]
---
# ADR 2026-11-02: TS 制御層の Rust/TS 境界・Host API 契約・generation・版固定・数値目標

- 状態: 採用（設計）。この ADR ではコードを変えない。実装は親 task 01M6DCMB75G5F0DSE58ZNZMBAK の S1〜S6。
- 根拠の記録: 棚卸し [inventory.md](../progress/2026-11-02-ts-control-layer-s0/inventory.md)、
  baseline [baseline.md](../progress/2026-11-02-ts-control-layer-s0/baseline.md)（生データは同じ dir の `baseline/*.jsonl`）。
- 関係: ADR-0069（lane 規則）、ADR-0124（直行経路）、[2026-10-28 CoS 定型の代答](2026-10-28-cos-routine-delegated-answers.md)、
  [2026-10-30 plan_conforms の人へ回す条件](2026-10-30-plan-conforms-human-guards.md)、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)、
  [2026-10-20 全体試験は release だけ](2026-10-20-full-tests-at-release-only.md)。

## 結論
- Rust は main・永続状態・scheduler・認可・安全不変条件の主体のまま残る。TS は「入力 snapshot → 提案」を返す関数だけを持ち、
  Rust が提案を再検証してから実行する。既存の安全検査は 1 つも TS へ移さない。
- TS は daemon と同じ process の V8 専用 thread（actor）で動く。Rust との通り道は bounded mpsc と oneshot、native op だけにする。
- 採用版: `deno_core` 0.413.0（`=` で固定）→ `deno_v8` 0.5.0 → `v8` 150.4.0。型検査は TypeScript 7.0.2、TS→JS 変換と bundle は esbuild 0.28.2。
- 目標: TS だけの変更の edit→activate を warm で p50 30 秒以下・p95 60 秒以下にする（Rust の現状は warm 中央値 policy 221.0 秒・cos 155.7 秒）。
- S5 の停止基準: 説明のつかない比較不一致 1 件、安全違反 1 件で即 `rust` へ戻す。timeout 率・失敗率は直近 1000 呼び出しで 0.1% を超えたら戻す。

## 背景
人の依頼（2026-11-02、親 task の objective）は、頻繁に変わる制御ロジックを Rust から TS へ段階的に移し、
Rust の compile・link と無関係な試験の繰り返しを減らすこと。S0 の測定では、1 規則の変更でも warm で
policy（`task-core` の `RULES`）約 221 秒、cos（`task-api` の `job_wait_extend::validate`）約 155 秒が Rust の段にかかった。
`task-core` を変えると 11 crate が再ビルドになる（inventory §3）。

## 決定

### (a) 責任境界

**Rust に残すもの（TS へ移さない・TS から呼べない）**
- store（SQLite・transaction）、events（追記専用）、task・WorkUnit の状態遷移（`transition`・`execution_scheduler`）、DAG の整合（`execution_plan`）。
- scheduler の実行・lease・排他・ready の並び順（`store/tasks.rs` の `ready_tasks_impl`）、capacity・write_set・quota の強制。
- 承認・権限・予算・再開上限（`replan_gate`・`plan_limits`・continuation 上限）、credential、sandbox、process・tool の実行、
  外部 API の transport、deploy の信頼境界、独立した評価器（reviewer・check・grader）。
- 共有状態の版確認と TOCTOU 対策（提案を適用する transaction の中で、提案の元にした snapshot の版を照合する）。

**TS へ移すもの（inventory §2.1・§2.2 の分類どおり）**
| 分類 | 対象（inventory の行） | Rust が TS の後に当てるもの |
|---|---|---|
| TS 移行 | lane 規則表（`model_policy::RULES`・`TaskFeatures::infer`）、route の候補順（`llm_route::route_priority`）、受信箱一次対応の digest（`cos_chat/digest.rs:digest_message`）、定期見直しの入力文（`cos_chat/review.rs:review_text`。store 読み出しを外へ出す Rust 変更が先） | `LaneCeiling::clamp`・人の明示（`TierSource::Human`）の優先、registry に在る route だけを残す検査 |
| 判定のみ切出し | execution gate の得点（`execution_gate::decide_at` の `Scorer`）、source/model の順位付け（`route_select/{policy,lane,balance}`）、dispatcher の `route_select`、受信箱取り込みの分類（`is_noise` 等）、CoS 不在時の退避の文面、`covers`、root 完了の要約文、replan の理由文 | `out_of_scope_rule`・人の明示・深さの閾値、`clamp_to_limits`・override、fail-closed、cursor・transaction・期限判定 |
| Rust 維持 | `direct_route::evaluate`、CoS 定型代答の検査（`basis::validate`・`plan_conforms`・`job_wait_extend`・`review_rework`）、CoS operations、受信箱 API、CoS run の起動、replan の上限、escalation、worker の起動と終了、WorkUnit の状態機械、ready の並び順、review 同期の順位、WU の ready 判定 | — |

**流れ**: Rust が入力 snapshot（DTO）を作る → TS が提案（DTO）を返す → Rust が提案を検証し、上の「Rust が当てるもの」を適用してから実行・記録する。
TS の提案が検証に通らなければ、Rust は (d) の mode に従って Rust の参照実装の結果を使うか、fail-closed で止める。

**安全検査は移さない・緩めない**: `plan_conforms`・`job_wait_extend`・`review_rework` の basis 検査、`human_required_problem`（human check の 403）、
`policy_version` の版違え 422、承認後の objective・acceptance の Edited 422、CoS 承認回数と版の上限は Rust の現在の場所に残す。
TS が CoS の一次判断を出す場合も、その判断は今と同じ API（`/cos/operations`・basis 付きの代答）を通り、同じ検査を受ける。
TS 側に同じ検査の写しを置くことは、Rust 側の検査を外す理由にしない。

### (b) 接続
- 新しい crate `celeris-script`（`crates/script-runtime/`）だけが `deno_core` に依存する。`deno_core` の型は crate の外に出さない。
  外から見えるのは `ScriptRuntimeHandle`（`Clone + Send + Sync`）と DTO だけ。
- `JsRuntime` は `!Send` なので、専用の OS thread（`std::thread`）1 本が生成・所有・破棄する。その thread の中で
  current-thread の tokio runtime が `JsRuntime::run_event_loop` を駆動し続ける。V8 platform の初期化は process で 1 回。
- 要求は bounded `tokio::sync::mpsc`（容量 64）で actor へ送り、応答は要求ごとの `oneshot` で返す。
  要求は `request_id`（ULID）・`domain`・`generation_id`・`deadline`・入力 DTO を持つ。`try_send` が満杯で失敗したら `ScriptError::Busy` を返し、待たない。
- 呼び出し側の cancel は oneshot の受け手を落とすことで伝え、actor は結果を捨てる。V8 の値・lock guard を `await` の向こうや別 thread に持ち越さない。
- 同期の無限ループは tokio の timeout では止まらないので、watchdog thread が `deadline` を過ぎた要求の `v8::IsolateHandle::terminate_execution` を呼ぶ。
  停止後は `cancel_terminate_execution` で isolate を再利用するか、isolate を作り直す（S1 で試験して決める）。
- Rust の非同期の仕事（store の読み出し等）は `#[op2(async)]` の native op で呼び、op は Rust 側の bounded channel を通して既存の tokio サービスへ渡す。
- **入れないもの**: Rust↔TS の通常経路の HTTP・TCP・unix socket・別 process の RPC。Bun・Node・Deno の CLI を daemon の親や子にすること。
  Node（v24、`web/` と同じ）と pnpm は開発と検証の段（型検査・bundle 作成）だけで使い、daemon の実行経路には現れない。
- crate の依存の制約: `deno_core` を推移的に持つ workspace crate は `celeris-script` と `celeris`（binary）の 2 つまで。
  `task-core`・`task-dispatch`・`task-api` は `deno_core` に依存せず、`task-dispatch` が持つ trait（`PolicyScript`、DTO だけを使う）越しに呼ぶ。
  これで `task-dispatch` 等の test binary（cos の test-build で 103 個、link の壁時計 21.2 秒）に V8 を link しない。
  検査: `cargo tree -i deno_core --workspace -e normal --prefix none | awk '{print $1}' | sort -u` が `celeris-script`・`celeris`・`deno_core` だけを出す。

### (c) Host API 契約 `celeris:host`
- module 名は `celeris:host`。版は semver で `1.0.0` から始める。bundle の manifest は `host_api: "^1"` の形で要求する。
  major が違えば load を拒否する。minor の追加は op・欄の追加だけ（既存の意味を変えない）。
- DTO は Rust の型が正本。`serde` と `schemars` 1（workspace の既存依存）で JSON Schema を出し、
  `docs/api/script-host/host-v1.schema.json` に置く。TS の型は `web/scripts/gen-types.mjs` と同じ方式の生成 script で作り、生成物を commit する。
  schema と生成物の食い違いは試験で落とす（生成し直して差分 0）。
- 実行時検証: TS→Rust の値（提案・op の引数）は Rust 側で `serde` の `deny_unknown_fields` で deserialize し、続けて領域ごとの検証関数を通す。
  Rust→TS の入力は schema に沿うことを Rust の試験で固定する。TS 側の検証は補助で、Rust 側の検証の代わりにしない。
- 値の写像:
  | Rust | TS | 規則 |
  |---|---|---|
  | task・run 等の ID（ULID） | `string` | 26 文字の文字列のまま。数値にしない |
  | `i64`・`u64`（token 数・秒・byte） | `number` | 絶対値が 2^53−1 以下に限る。超えうる欄は 10 進の `string` にし、schema に書く |
  | `Option<T>` | `T \| null` | 欄は常に出す（`undefined` を使わない）。TS が欄を省いたら検証で拒否する |
  | enum（unit） | 文字列リテラルの union | serde の `rename_all = "snake_case"`。未知の値は拒否する |
  | enum（データ付き） | `{ kind: "…", … }` の判別 union | serde の `tag = "kind"` |
  | 時刻 | `string`（RFC 3339、UTC） | TS に時計を持たせない。現在時刻は入力 DTO で渡す |
  | エラー | TS は `HostError { code, message }` を throw | Rust は `ScriptError::{Exception, Timeout, Busy, InvalidOutput, Denied, Terminated, GenerationGone}` に写す |
- TS に乱数・時計・環境変数を持たせない。`Math.random` と `Date.now` は bootstrap で入力の seed・時刻に置き換えるか、呼ぶと throw させる（S1 で決める）。
- capability: bundle の manifest は使う op を `capabilities`（例 `read:task_snapshot`）で宣言する。Rust は「manifest の宣言 ∩ 領域ごとの許可表」だけを有効にする。
  op は要求に結び付いた scope（対象 task の id）を Rust 側で照合し、範囲外の読み出しは `Denied`。
  filesystem・network・環境変数・process・native addon の op は作らない。module loader は bundle 自身と `celeris:host` 以外の import を拒否する。
- 上限: 1 要求の入力 1 MiB、V8 heap 64 MiB（`near_heap_limit_callback` で実行を止める）、in-flight 64。
  同じ process の中なので、OOM と native の fault は完全には隔離できない。破壊的な OOM 試験は本番の外の試験 process だけで行う。

### (d) generation と状態モデル
- **bundle**: TS の業務コードは `include_str!`・`build.rs`・業務コード入りの snapshot に焼き込まない。独立した ESM の bundle 1 file と manifest を配る。
  manifest（`celeris.script-manifest/1`）は `bundle_sha256`・`host_api`・`schema_sha256`・`toolchain`（TypeScript・esbuild・Node の版）・
  `source_commit`・`domains`・`capabilities`・`checkpoint_versions` を持つ。静的な bootstrap（polyfill の拒否、`celeris:host` の結線）だけは Rust に入れてよい。
- **generation**: load して検証に通った bundle 1 つ = generation 1 つ。DB に `script_generations`（id・bundle_sha256・manifest・状態）を持ち、
  切り替えは events に `script_generation_*` として追記する。
- **領域ごとの mode**: `rust` → `compare` → `canary` → `ts`。初期値はすべて `rust`。
  | mode | 結果に使う実装 | TS の実行 |
  |---|---|---|
  | `rust` | Rust | しない |
  | `compare` | Rust | 同じ入力で TS も実行し、結果の差だけを記録する（副作用なし、LLM・外部 tool を呼ばない） |
  | `canary` | 対象の一部（領域ごとに決めた割合または task の集合）だけ TS、残りは Rust。TS の分も Rust の参照実装と比べて記録する | する |
  | `ts` | TS | する。Rust の参照実装は S6 の終了条件まで残す |
  - 差分比較の名前は `compare`（event は `script_compare_recorded`）にする。旧 `routing_shadow` の表・event 名・config 欄を使わず、復活させない。
- **atomic な切り替え**: 領域の mode と active generation は `script_domain_modes` の 1 行で持ち、`version` 列の CAS で 1 transaction に更新し、
  同じ transaction で event を追記する。actor は要求を受けた時点の generation を要求に固定する。
- **旧 generation の drain**: 切り替え後に来た要求は新 generation へ行く。旧 generation は in-flight が 0 になるまで残し、
  純粋な判定は 30 秒で打ち切る（打ち切った要求は `GenerationGone` → Rust の参照実装で答える）。
  CoS の非同期経路（S4）は、Promise・closure を新 generation へ移さない。永続 checkpoint から新 generation が再開する。
- **checkpoint の版互換**: checkpoint は `celeris.script-checkpoint/<n>` の版を持つ。新 generation の manifest の `checkpoint_versions` が
  その版を含むときだけ引き継ぐ。含まなければ、その経路は副作用前なら Rust の参照実装でやり直し、副作用後なら止めて受信箱に出す。
- **副作用の扱い**: 副作用の前の純粋判定は Rust へ fallback してよい。副作用の後は無条件に再実行しない。冪等 key と実行記録で照合し、照合できなければ止める。
  認可の error では fail-open しない（許可側へ倒さない）。
- **再起動後の復元**: daemon 起動時に `script_domain_modes` を読み、active generation の bundle の sha256 を disk 上の file と照合する。
  file が無い・hash が違う・`host_api` が合わない場合は、その領域を `rust` に戻して event と受信箱項目を残す（fail-closed）。
  Rust の参照実装を消した後の領域では、その領域の処理を止めて人に知らせる。
- **権限**: `rust`→`compare` は挙動を変えないので、既存の CoS operations の権限で行える。`compare`→`canary`・`canary`→`ts` は本番の挙動を変えるので、
  既存の本番昇格と同じく人の承認を要る。`rust` へ戻す操作（rollback）は誰でも・自動でも行える。

### (e) 版の固定（2026-11-02 に crates.io の index・API と npm registry で確かめた）
| 部品 | 採用版 | 公開日（UTC） | 確かめたこと |
|---|---|---|---|
| `deno_core` | 0.413.0（`=0.413.0` で固定） | 2026-10-21 | crates.io の最新。yanked でない。edition 2024。`.crate` の sha256 `7ad7d5294f640fd99db08c455e2453c9860b1fa728b76fbdacf0eed3e308d48a` |
| `deno_v8`（`deno_core` の依存名は `v8`） | 0.5.0（`deno_core` が `^0.5.0` で要求） | 2026-10-21 | 0.410.0 から `deno_core` は `v8` crate を直接でなく、この facade を通して使う。sha256 `3dfb3151dcae150420c2fe784928b0dbce8897c6d8bc4bac7b54a5565e06b288` |
| `v8`（rusty_v8） | 150.4.0（`deno_v8` が `^150.4.0` で要求） | 2026-07-26 | yanked でない。crates.io の最新は 152.2.0 だが、`deno_v8` 0.5.0 は 150 系だけを許す。150.4.0 の `.crate` は 36,296,166 byte、sha256 `42a978ff11f15b24e5c05a7123cf2b68f41e763546699781a924ef4e2cf43a49` |
| TypeScript（型検査） | 7.0.2 | 2026-07-08 | npm の `latest`。`web/package.json` と同じ版。`engines.node >=16.20.0` |
| esbuild（TS→JS 変換と bundle） | 0.28.2 | 2026-08-08 | npm の `latest`。`engines.node >=18` |
| Node / pnpm（道具の実行だけ） | v24.14.1 / 12.6.0 | — | この host の版。`web/` と同じ |

- `deno_core` の feature は既定（`v8`・`v8_use_custom_libcxx`・`reactor-tokio`）だけを使う。`quickjs`・`unsafe_*` は使わない。
- 型検査（`tsc --noEmit`）と変換（esbuild）を分ける。deno_core は TS の型検査も Node・npm・Web API の互換も持たないので、
  bundle は依存を同梱した単一の ESM にし、Node の組み込み module を import したら bundle 作成で落とす。
- Rust の互換:
  - repo に `rust-toolchain` の file と workspace の `rust-version` は無い。実際の toolchain は rustc 1.98.1（2026-09-01）・cargo 1.98.1。
  - `deno_core` 0.413.0・`deno_v8` 0.5.0・`v8` 150.4.0 は crates.io の index に `rust_version` を宣言していない（MSRV は未宣言）。
    edition 2024 なので rustc 1.85 以上が要る。rustc 1.98.1 で build できるかは未確認（S1 の最初の `cargo check -p celeris-script` で確かめる）。
  - `deno_core` の `tokio` 要求 `^1.47.1` は workspace の `Cargo.lock` の tokio 1.53.1 で満たす。他の依存の衝突は未確認（S1 の `cargo tree -d` で確かめる）。
- `v8` の build は、既定で GitHub の `denoland/rusty_v8` の release から事前 build の static library を download する（`build.rs` の `RUSTY_V8_MIRROR`・`RUSTY_V8_ARCHIVE`）。
  release gate と試験で外部 network に出ないため、S1 で archive を repo の外（`/local/celeris/data/scratch` の下）に一度置き、
  `RUSTY_V8_ARCHIVE` で指し、その sha256 を manifest と release の記録に残す。archive の大きさと sha256 は未確認。
- 版を上げるときは、この表を同じ変更で更新し、S2 の比較 fixture を全部通し直す。

### (f) 数値目標と停止基準
baseline（[baseline.md](../progress/2026-11-02-ts-control-layer-s0/baseline.md)、rustc 1.98.1、Threadripper PRO 3945WX 論理 24、`CARGO_INCREMENTAL=0`、sccache 無効）の値:
| 項目 | policy | cos |
|---|---|---|
| warm の edit→release build（6 段の和、3 試行） | 中央値 221.0 秒 [219.7, 221.0, 224.8] | 中央値 155.7 秒 [155.3, 155.7, 158.9] |
| cold（1 試行） | 283.5 秒 | 377.2 秒 |
| TS 化で編集ごとに消せる段（warm） | 約 208 秒 | 約 137 秒 |
| test-affected（逆依存込み） | 未計測 | 291.3 秒（下限、fail-fast で停止） |
| release build（celeris、cold） | 131.3 秒 | 131.0 秒 |

**改善目標（S3・S6 で測る）**
- TS だけの変更の edit→activate（型検査・bundle・対象の TS 試験・Rust/TS 比較 fixture・事前 build 済みの host での統合試験・generation の load と `compare` での有効化）:
  warm で p50 30 秒以下、p95 60 秒以下。p50 30 秒は cos の warm 中央値 155.7 秒の 19%、policy の 221.0 秒の 14%。
  cold（`node_modules` を offline の store から入れる所から）で p95 120 秒以下（cos の cold 377.2 秒の 32%）。
- この経路で cargo の compile・link は 0 回（計測の記録で `link_count` 0、cargo の起動 0 回）。
- 測定は同じ変更課題・同じ host で 20 試行以上。p95 は最近傍順位（20 試行なら 19 番目）で出す。TS の型検査・bundle の秒も別の欄に出す。
- release gate の全体試験（548〜693 秒）は短縮の対象にしない。

**許容 overhead（S1 で測る。超えたら S2 へ進まず設計を見直す）**
- 判定 1 回の遅延（actor への送信から oneshot の受信まで、warm の isolate、純粋判定）: p50 1 ms 以下、p99 5 ms 以下。
  dispatcher の tick 周期（`tick_ms` 既定 2000 ms）の 0.25% 以下に当たる。1 要求の deadline は 50 ms（CoS の digest など文面組み立ては 200 ms）。
- RSS: daemon の RSS の増分は isolate 1 個で 100 MiB 以下、generation 2 個（active と drain 中）で 200 MiB 以下。daemon の現在の RSS は未計測で、S1 で前後を測る。
- build: `celeris` の cold の release build の増分は 60 秒以下（131.3 秒 → 191 秒以下）。warm の `cargo build -p celeris` の link の増分は 1 回 5 秒以下（現状 1.36 秒）。
  `celeris` の release binary の増分は 80 MiB 以下。`task-dispatch`・`task-api` の test-build の秒は増えない（(b) の依存の制約による。増分 2 秒以下）。

**S5 の停止基準（評価の前に固定する。領域ごとに数える）**
| 基準 | `compare`→`canary` へ進む条件 | `canary`・`ts` で即 `rust` へ戻す条件 |
|---|---|---|
| 比較不一致（ADR に書いた正規化の後） | 純粋判定は連続 1000 呼び出し以上、CoS の経路は 200 件以上で、説明のつかない不一致 0 件 | 説明のつかない不一致 1 件 |
| timeout 率（deadline 超過・`Terminated`） | 直近 1000 呼び出しで 0.1% 以下 | 直近 1000 呼び出しで 0.1% 超、または 10 分間に 3 件 |
| 失敗率（`Exception`・`InvalidOutput`・`Busy`・`GenerationGone`） | 直近 1000 呼び出しで 0.1% 以下 | 直近 1000 呼び出しで 0.1% 超、または連続 3 件 |
| 安全違反 | 0 件 | 1 件で戻し、その generation を無効にし、受信箱に出す |

（2026-11-04 改め: この表の `lane_rules` の「進む条件」は、評価の開始後に人の判断で改めた。末尾の「付記 2026-11-04」を見る。元の行は上のまま残す。）

- 安全違反は次のどれか: Rust の再検証が「安全条件を越える提案」を拒否した（lane の天井越え、人の明示の上書き、`out_of_scope_rule` の対象を通す、fail-closed の候補を採る、許可されない capability の要求、scope 外の読み出し）。
  または Rust の検証を通らずに副作用が実行された。後者は検出したら S5 全体を止める。
- 呼び出しが 1000 件に届かない領域は、件数が届くまで `compare` に留める。
- S6 の打ち切り基準: TS の経路の edit→activate の warm p50 が 78 秒（cos の warm 中央値 155.7 秒の 50%）を超えたら、次の領域の移行を止め、原因を記録して計画を見直す。

### (g) 凍結環境と識別
- IEEE Access の評価・K2・K4 の測定環境（測定用の release・state dir・config・pin した binary）は差し替えない。TS 制御層の導入は、それらの config に `[script]` 欄を足さず、
  全領域 `rust` のまま動く binary でも測定の比較に混ぜない。
- run・trial の記録に TS 制御層の識別を足す: `celeris` の commit、`host_api` の版、領域ごとの mode と active generation の `bundle_sha256`。
  識別が違う試行は同じ arm・同じ集計に入れない。S6 の測定（言語移行の比較）と、移行後の方針改善の性能比較は別の測定として記録する。
- 進行中の測定 daemon で `compare`・`canary`・`ts` へ切り替えない。

## 不採用
- 別 process の JS runtime（Node・Bun・Deno を子 process にして HTTP・stdio で話す）: 依頼の制約で禁止。process 管理と認可の面も増える。
- TS の業務コードを binary に焼き込む（`include_str!`・snapshot）: TS の変更で Rust の再 build が要り、目的に反する。
- TS 側に安全検査を移し、Rust 側の検査を外す: 依頼の制約で禁止。
- `v8` 152 系を直接使う: `deno_core` 0.413.0 の facade が 150 系を要求するため、組み合わせられない。

## 後続（S1 で行うこと）
- `crates/script-runtime/`（`celeris-script`）を作り、`docs/architecture-map.md` に行を足す（この ADR の変更では、実在しない path を地図に書かないので足さない）。
- 本番 config の書き換えは不要（全領域 `rust` が既定で、`[script]` 欄が無ければ TS 制御層は動かない）。

## 付記 2026-11-02: (b) の依存の制約を直す（S1 daemon-wire、task 01M6DJ6QCMCWT80MTYJ7SNE0XK）
- (b) は `celeris` を binary だけの crate と見ていた。実際の `celeris` は lib でもあり、`celerisctl` が `celeris` の lib（主に `celeris::config`）に通常依存している。S0 の棚卸しはこの依存を見落とした。
- そのため `celeris` が `celeris-script` に依存すると、`celerisctl` にも `deno_core` と V8 が推移的に入る。S1 の計画 v4 でこれを受け入れた（案 A）。
  - 実測: `cargo build --release -p celerisctl` の binary は 97,713,712 byte。`v8::`・`deno_core::`・`simdutf` の定義は 58423 個、`nm -S` の大きさの和は 21.0 MiB。V8 を持たない base の release binary との比較は未測定（S1 の overhead unit で測る）。
  - debug build の `celerisctl` にも `v8::`・`deno_core::` の定義が 57044 個残る。feature で分けても `cargo build -p celeris -p celerisctl`（`release.sh`）では feature が統合されるので外せない。
- 直した制約: `deno_core` を推移的に持つ workspace crate は `celeris-script`・`celeris`・`celerisctl` の 3 つまで。`task-core`・`task-dispatch`・`task-api` は 0 件のまま（目的の「これらの test binary に V8 を link しない」は変わらない）。
- 直した検査: `cargo tree -i deno_core --workspace -e normal --prefix none | awk '{print $1}' | sort -u` が `celeris`・`celeris-script`・`celerisctl`・`deno_core` だけを出す。
- 後の段への提案（未決）: V8 を持たない lib の package と `celeris` の binary の package に分け、`celerisctl` を lib だけに依存させる（案 B）。`crates/celerisctl/Cargo.toml`・`scripts/selfdeploy/release.sh`・`-p celeris` を使う検査・`crates/celeris/tests/` に影響し、全量は未確認。

## 付記 2026-11-02: S1 で決めたこと（S1 close、task 01M6DJ6QCMCWT80MTYJ7SNE0XK）
S1 の各 unit の記録は [S1 の進捗](../progress/2026-11-02-ts-control-layer-s1.md) と同じ名前の dir にある。

### TS の置き場と配備先（(d) の補い）
- TS の package は repo 直下の `control-ts/`。`tsc --noEmit`（型検査）と esbuild（変換と bundle）を別の段にし、
  `control-ts/dist/bundle.mjs` と `control-ts/dist/manifest.json` を作る。`dist/` は commit しない（作る物）。
- 配備先は `<state_dir>/script-bundles/<bundle_sha256>/`（`bundle.mjs` と `manifest.json` の 2 file）。`state_dir` は config の `[script] state_dir`。
  配備は一時 dir `.tmp-<ULID>` に書いて fsync し、rename する。同じ hash の dir が在れば検証して再利用し、
  `host_api`・`schema_sha256`・`domains`・`capabilities`・`checkpoint_versions` が違えば上書きせず拒否する。
- daemon は起動時に `active_bundle_sha256` の dir を読み、dir 名・manifest・bundle の hash と `host_api`・`schema_sha256` を照合する。
  合わなければ isolate を使わず、log と通知（束ね key `script_bundle_rejected`）を出す（fail-closed）。`[script]` が無ければ isolate を起こさない。

### `Math.random`・`Date.now`（(c) の「S1 で決める」の答え）
- 置き換えず、呼ぶと `TypeError` を throw させる。引数なしの `Date`（`new Date()`・`Date()`）も同じ。`performance` は消す。
  現在時刻は入力 DTO の `Timestamp`（RFC 3339、UTC、末尾 `Z`）で渡す。
- 理由: 置き換えると全入力 DTO に seed と時刻の欄が要り、渡し忘れても黙って同じ値で動く。throw なら渡し忘れが試験で落ちる。
  S2 の `compare` で Rust と TS を比べるには、TS が隠れた入力を持たないことが要る。試験は `runtime_clock_random_policy`。

### terminate 後の isolate（(b) の「S1 で試験して決める」の答え）
- 再利用しない。watchdog（deadline 超過）か heap 上限で止めた generation は捨て、同じ bundle で isolate を作り直す（generation 番号 +1）。
  止めた要求には `Timeout`（heap なら `HeapLimit`）、同じ generation の他の未完の要求には `GenerationGone` を返す。
- 理由: `cancel_terminate_execution` の後の再利用は技術的にはできるが、止めた時点の途中の状態が残る
  （`globalThis.pairA = 1; for (;;) {} globalThis.pairB = 1;` を止めると再利用した isolate では `[1, null]`。unit 試験
  `terminate_then_reuse_keeps_half_written_state`）。作り直せば global は初期状態に戻る（`runtime_timeout_sync_loop`）。
- `ScriptError` には (c) の 7 種に加えて `InputTooLarge { size, limit }` と `HeapLimit` がある（runtime-guard unit で足した）。

### V8 archive（(e) の「archive の大きさと sha256 は未確認」の答え）
- `deno_core` 0.413.0 は `v8` の `simdutf` feature を必須にする。必要な archive は
  `librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz`（v150.4.0、38,799,023 byte）。simdutf 無しの
  `librusty_v8_release_x86_64-unknown-linux-gnu.a.gz` では link が `undefined symbol simdutf__*` で落ちる。
- 入手元は GitHub の `denoland/rusty_v8` の release `v150.4.0` の asset。人が 2026-11-02 に
  `/local/celeris/data/scratch/rusty-v8/v150.4.0/` に読み取り専用で置き、同じ dir の `SHA256SUMS` に hash を書いた。
  確かめる command: `sha256sum /local/celeris/data/scratch/rusty-v8/v150.4.0/librusty_v8_simdutf_release_x86_64-unknown-linux-gnu.a.gz`
  の出力が `SHA256SUMS` の同じ file の行と一致し、release の asset digest
  （`f48762ca10d1f1fc605a441c5ae430ec8ce1e9e80f14d78fbc42cb878c30b476`）と一致すること。`gzip -t` も通る。
- 設定: host の `~/.cargo/config.toml` の `[env]` に `RUSTY_V8_ARCHIVE = "<上の path>"` を書く。`v8` の build.rs はこの file を展開して使い、
  network に出ない。repo の `.cargo/config.toml`（worktree ごとに生成）と config には書かない。値を替えると `v8` の build script が走り直す。
- debug profile の試験も release の archive で link できる（S1 の全試験はこの形で通った）。

### `deno_core` の写しと serde_json の feature（(b)・(e) の補い）
- crates.io の `deno_core` 0.413.0 は serde_json に `preserve_order`・`float_roundtrip` を付け、cargo の feature 統合で workspace 全体の
  JSON の key 順と f64 の parse を変える。そこで写し `crates/script-runtime/vendor/deno_core/` を root の `[patch.crates-io]` で使い、
  写しの `Cargo.toml` から 2 つの feature だけを外した（`raw_value` は残す）。判断と作り直しの手順は
  [ADR 2026-11-02-deno-core-vendored-serde-json-features](2026-11-02-deno-core-vendored-serde-json-features.md)。
- 確かめる command: `sh crates/script-runtime/vendor/vendor-deno-core.sh --check`（`.crate` の sha256 と Cargo.toml の差分 2 行を照合）、
  `cargo tree --workspace -e features -i serde_json | grep -cE 'preserve_order|float_roundtrip'` が 0。

### `cargo tree -d` の結果（(e) の「他の依存の衝突は未確認」の答え）
- `cargo tree -d -p celeris-script -e normal --depth 0 --locked`: 版の違う重複は `getrandom` 0.3.4/0.4.3 と `syn` 2.0.119/3.0.5 の 2 組。
  `serde`・`serde_core`・`smallvec` は同じ版が host 用と target 用で 2 回出るだけ。
- `cargo tree -d --workspace -e normal --depth 0 --locked`: 版の違う重複は 10 組（`base64`・`getrandom` 3 版・`hashbrown`・`miniz_oxide`・
  `rand_core`・`socket2`・`syn`・`toml_datetime`・`winnow`）。どの版も `deno_core` を持たない crate（`task-api`・`task-core` 等や `celeris` の他の依存）からも使われており、
  `deno_core` だけが持ち込んだ版は無い（`cargo tree --workspace -e normal -i <crate>@<版>` で各版を確かめた）。build を妨げる衝突は無い。
- rustc 1.98.1・cargo 1.98.1 で build できる。写しの `deno_core` に rustc の deprecated の警告が 1 件出る（path 依存なので cap-lints されない。error ではない）。

## 付記 2026-11-02: S2 の compare mode（S2 adr-compare、task 01M6E6HEH7CPZX9KX4Y2SAZKTG）
S2 の作業の前に、(d) の `compare` を実装できる粒度で決める。根拠は [S0 の計画](../progress/2026-11-02-ts-control-layer-s0.md) の §2 S2・§3 と
[S1 の未解決](../progress/2026-11-02-ts-control-layer-s1.md)。記録は [S2 adr-compare](../progress/2026-11-02-ts-control-layer-s2/adr-compare.md)。
この付記はコードを変えない。

### S2-1 対象の領域と順序
| 順 | 領域名（`domain`） | Rust の参照実装 | 本物の呼び出し経路（compare を付ける所） |
|---|---|---|---|
| 1 | `lane_rules` | `model_policy::RULES` を上から当てる部分。policy-split unit が `match_rule(&TaskFeatures) -> RuleMatch` に切り出す（挙動不変、言語移行と別 commit） | `dispatch_run.rs` の `decide_lane` の中の `decide_for_task` の 1 か所。`work_units.rs` の呼び出しは Rust のまま |
| 2 | `route_priority` | `llm_route::route_priority`（候補列の選び方） | `route_select.rs` の `route_select_one` の最初の `route_priority` の 1 か所で、`cos == false` の task の run だけ。harness balance の得点（`order_harnesses` の前）と frontier 列の後ろの列の呼び出しは Rust のまま |

- `direct_route::evaluate` は Rust に残す（(a) の表と ADR-0124）。S2 では TS の写しも作らない。
- 2 つ目は 1 つ目の受け入れ（fixture 全件一致・本物の経路での compare・`rust` への切り戻し）の後に足す。
- 両領域とも時刻・乱数を使わない。入力 DTO に時刻・seed の欄を置かない（TS で `Date.now`・`Math.random` を呼ぶと throw する。S1 の付記）。

### S2-2 入力 DTO・出力 DTO と比べる欄
DTO の正本は Rust の型（(c)）。全 struct に `deny_unknown_fields`。`docs/api/script-host/host-v1.schema.json` に足し、TS の型は生成する。

**`lane_rules`**
- 入力 `LaneRulesInput { features: TaskFeatures }`。`features` は `TaskFeatures::infer_with_hints` の後の 9 軸（`Level` = `low|medium|high`）。
  Rust は `routing.tier_source.policy_decides()` が真（`Hint`・`Default`）のときだけ TS を呼ぶ。`Human`・`System` の task では TS を呼ばず、記録もしない。
- 出力 `LaneRulesOutput { rule_id: string, proposed: Tier, condition: string }`（`Tier` = `frontier|standard|cheap`）。
- `LaneCeiling::clamp`・`source`・`hint`・`reasons`・retry の escalation は Rust が出力の後に当てる。これらは比較しない。
- byte 一致を求める欄: `rule_id`・`proposed`・`condition` の 3 欄すべて。

**`route_priority`**
- 入力 `RoutePriorityInput { harness: string, lane: string | null, cos: bool, execution_class: string | null, table: RouteTable }`。
  `RouteTable { route_defaults: CandidateKey[], lane_route_defaults: { lane: string, candidates: CandidateKey[] }[], class_defaults: { execution_class: string, candidates: CandidateKey[] }[], cos_route_candidates: CandidateKey[] }`、
  `CandidateKey { source: string, model: string }`。`table` は `Registry` のうち、その `harness` の行だけを写す（配列は Rust の `BTreeMap` の key 順）。
- 出力 `RoutePriorityOutput { basis: "cos" | "execution_class" | "lane" | "harness" | "none", candidates: CandidateKey[] }`。
  `basis` はどの列を選んだかの理由コード。route-prio unit が `route_priority` を「列と basis を返す関数」と既存の薄い包みに分ける（挙動不変、別 commit）。
- 空の `candidates`（`basis = "none"`）は正しい出力。その後の `NoDeclaredDefault` の fail-closed は Rust が行う。
- byte 一致を求める欄: `basis` と `candidates`（順序を含む全要素）。

**正規化する非本質差分**: 両領域とも「なし」。比較は、TS の戻り値を出力 DTO に deserialize し直し、Rust の出力と同じ serde_json の直列化（空白なし、struct は宣言順）にした byte 列どうしで行う。
TS の生の JSON の key 順・空白は deserialize で消えるので比較に現れない。これは値の正規化ではない。値の正規化（大小文字・順序の並べ替え・重複除去）は一切しない。
正規化を足すときは、この付記に規則を足す変更と同じ差分で行う。

### S2-3 比較入力の機密除去
- 方針は許可列挙（allowlist）。記録する入力は S2-2 の入力 DTO そのもので、DTO に無い値は記録しない。DTO を広げる変更は、この節の表を同じ差分で直す。
- 保存するもの: `lane_rules` は 9 軸の `Level` だけ。`route_priority` は `harness`・`lane`・`cos`・`execution_class` と、その harness の候補列の `source` id・`model` id。
- 捨てるもの（DTO に入れない）: credential・secret・API token・OAuth の値、account の id と名前、Host API の要求 token（S1 の `ctx.token`）、
  task の title・objective・acceptance・budget の本文、prompt・message・log の本文、`SourceSpec` の全欄（base URL・環境変数名・鍵の参照）、
  `llm_routes`（`via`・API 形式）、他の harness の行、quota・残量（`source_headroom`）、利用者・組織の名前。
- task の id は event を追記する行（`events.task_id`）が持つので、payload には書かない。run の id は書いてよい（機密でない）。
- 記録する入力の直列化が 64 KiB を超えたら、`input` を `null` にし `input_digest` だけを残す（`input_truncated: true`）。
- TS の例外の `message` は 512 byte で切る。入力が機密を持たないので、例外文に機密は入らない。

### S2-4 event と表
**event `script_compare_recorded`**（`Event` の新しい variant。追記先は比較した task の `events`。状態は変えず、`replay` は無視する）
| 欄 | 型 | 中身 |
|---|---|---|
| `domain` | `"lane_rules" \| "route_priority"` | 領域 |
| `generation` | string（ULID） | 使った `script_generations.id` |
| `isolate_epoch` | number | actor が isolate を作り直した回数（S1 の generation 番号 +1）。同じ `generation` の中の区別 |
| `bundle_sha256` | string（64 桁 hex） | 使った bundle |
| `host_api` | string | host の Host API 版（`1.0.0`） |
| `call_site` | string | `dispatch_run.decide_lane`・`route_select.route_select_one` |
| `run_id` | string \| null | 判定した run（作る前なら null） |
| `input_digest` | string | `sha256:` + S2-2 の直列化の sha256（64 桁 hex） |
| `input` | object \| null | S2-3 で除去した後の入力 DTO |
| `input_truncated` | bool | S2-3 の 64 KiB 超え |
| `rust` | object | Rust の出力 DTO |
| `ts` | object \| null | TS の出力 DTO（`ts_error`・`unavailable` は null。`invalid_output` は生の値を 64 KiB まで） |
| `outcome` | `"match" \| "mismatch" \| "ts_error" \| "invalid_output" \| "unavailable"` | 下の定義 |
| `error` | `{ kind, message } \| null` | `ts_error` の `ScriptError` の種類（snake_case）と文。`invalid_output` は deserialize・検証の失敗理由 |
| `normalized` | string[] | 当てた正規化の規則名。S2 では常に `[]` |
| `safety_violations` | string[] | S2-8 の code。無ければ `[]` |
| `elapsed_us` | number | actor への送信から応答（または打ち切り）まで |

`outcome` の定義:
- `match`: TS の出力が DTO として正しく、S2-2 の比較欄がすべて byte 一致。
- `mismatch`: TS の出力は DTO として正しいが、比較欄のどれかが違う。
- `ts_error`: TS を呼んだが `ScriptError`（`Exception`・`Timeout`・`Busy`・`Terminated`・`GenerationGone`・`HeapLimit`・`InputTooLarge`・`Denied`）で終わった。
- `invalid_output`: 戻り値が出力 DTO に deserialize できない（未知の欄・欠けた欄・未知の enum 値）か、領域の検証に通らない
  （`lane_rules` の `rule_id` が空または 128 byte 超、`condition` が 512 byte 超。`route_priority` の候補が 256 件超）。
- `unavailable`: TS を呼ばなかった（runtime が無い、mode が `compare` なのに generation が load されていない、manifest の `domains` にその領域が無い）。

重複の抑え: 同じ task・同じ `domain` の直前の `script_compare_recorded` と `generation`・`input_digest`・`outcome` が同じなら追記しない
（dispatcher が tick ごとに同じ判定を繰り返すため）。S5 の件数はこの抑えの後の event の件数で数える。

**表 `script_generations`**（migration は compare-store unit。番号はその時の空き）
- 列: `id` TEXT PK（ULID）、`bundle_sha256` TEXT NOT NULL UNIQUE、`host_api` TEXT、`schema_sha256` TEXT、`manifest_json` TEXT（manifest の全文）、
  `state` TEXT CHECK IN (`loaded`,`rejected`)、`created_at`、`updated_at`。
- daemon が bundle を load して検証に通ったら `loaded` の行を作る（同じ hash の行が在れば再利用）。検証に落ちたら `rejected`。isolate の作り直しでは行を足さない。
  drain・退役の状態は S5 で migration を足して加える。

**表 `script_domain_modes`**
- 列: `domain` TEXT PK、`mode` TEXT NOT NULL CHECK IN (`rust`,`compare`,`canary`,`ts`)、`generation_id` TEXT NULL（`script_generations.id`）、
  `version` INTEGER NOT NULL、`updated_at`、`updated_by`（actor）。CHECK で `mode = 'rust' OR generation_id IS NOT NULL`。
- migration で `lane_rules`・`route_priority` の行を `rust`・`generation_id` NULL・`version` 0 で入れる。CHECK に `canary`・`ts` を含めるのは S5 で表を作り直さないためで、S2 の API は受け付けない（S2-5）。
- 更新は CAS だけ: `UPDATE … SET mode=?, generation_id=?, version=version+1, updated_at=?, updated_by=? WHERE domain=? AND version=?`。
  更新した行が 0 なら 409（版の競合）。同じ transaction で `script_domain_mode_changed` を追記する。
- **`script_domain_mode_changed`**: 欄は `domain`・`from_mode`・`to_mode`・`from_generation`・`to_generation`・`version`（更新後）・`actor`・`reason`。
  mode の変更は特定の task に属さないので、`events`（`task_id` が NOT NULL）ではなく追記専用の表 `script_domain_events`（`id` INTEGER PK、`ts`、`domain`、`json`）に書く。
  `json` は `Event::ScriptDomainModeChanged` の直列化と同じ形にする。store にこの表の UPDATE・DELETE の経路を作らない。
- dispatcher は tick ごとに `script_domain_modes` を 1 回読み、その tick の判定に使う。

**旧 routing_shadow を使わない**: `routing_shadow` の表・`RoutingShadowRecorded`（`routing_shadow_recorded`）・`routing_shadow_history`・
旧 `[model_routing]` と tier binding の config 欄は、compare の記録・集計・API・config のどれにも使わない。S2 は config 欄を足さない（mode は DB の表が持つ）。

### S2-5 S2 で使う mode と権限
- S2 で受け付ける mode は `rust` と `compare` だけ。API が `canary`・`ts` を受けたら 422（code `script_mode_not_enabled`）を返し、表を変えない。
- `rust`→`compare`: 人の API token か、CoS operations（`/cos/operations` の registry に ALLOWED で足す操作。mode-api unit）で行う。
  要求は `generation`（`bundle_sha256`）と `expected_version` を持つ。対象の generation が `loaded` で、manifest の `domains` にその領域が無ければ 422。
- `compare`→`rust`（戻し）: 認証された誰でも（人・CoS・外部 operator の token）と daemon 自身が行える。daemon は起動時に active generation の bundle が
  無い・hash が違う・`host_api` が合わない場合に、actor `system:script-startup` で戻す（(d) の再起動後の復元）。
- 同じ mode・同じ generation への変更は、表も event も変えずに 200 を返す。
- 本番で `compare` にするのは人の手順（S2 close unit が進捗の「本番への移行」に書く）。進行中の測定 daemon では切り替えない（(g)）。

### S2-6 compare が守ること
- Rust の結果を一切変えない。dispatcher が使う値は常に Rust の参照実装の出力で、TS の出力は `script_compare_recorded` に書くだけ。
  `routing_decided` などの既存 event・run の行・lane・候補列は `rust` のときと byte 一致する（host-adapter unit の試験で、常に違う答えを返す bundle を使って固定する）。
- TS の失敗・timeout・不正な出力・runtime 不在のどれでも、Rust の結果をそのまま使い、dispatch を止めない。
- deadline は 1 呼び出し 50 ms（(f)）。超えたら待つのをやめ、`ts_error`（`error.kind = "timeout"`）を記録する。dispatcher が 1 判定で待つ時間は最大 50 ms。
  （実装では dispatcher は待たない。比較は別の tokio task で 50 ms の deadline 付きで走る。S2-12 の (1)）
- LLM と外部 tool を呼ばない。両領域の TS は op を呼ばない純粋関数で、Rust 側も compare のために store の書き込み（記録の追記を除く）・process・network を使わない。
- 記録の追記に失敗しても dispatch は続ける（warn の log だけ）。記録は判定の後に別の書き込みで行い、判定の transaction に入れない。

### S2-7 要求ごとの V8 context（S1 未解決 4）
- S2 では要求ごとに V8 context を分けない。理由: 両領域は op を呼ばない純粋判定で、領域ごとの許可表を空にする。要求の `scope_task_ids` も空で送る。
  他の要求の token を盗んでも呼べる op が無いので、S1 で見つけた「相手の scope で op を呼ぶ」経路が開かない。
- 許可表が空の領域の manifest が `capabilities` を宣言していたら、その領域を load で拒否する（`script_generations` は `rejected`）。
- 残る危険は、bundle が global に値を残して後の要求の答えを変えること（隠れた入力）。対策は 2 つ:
  fixture の比較 runner は同じ isolate に全件を正順と逆順の 2 回流し、同じ出力を要求する。本番の compare では違いが `mismatch` として出る。
- 要求ごとの context の分離は、op を使う最初の領域（S4 の digest）より前に決める。決めるまで、どの領域にも capability を許可しない。

### S2-8 安全違反の数え方（S5 の停止基準の分母）
- `script_compare_recorded` の `safety_violations` に code を入れる。code:
  - 共通: `denied_capability`・`denied_scope`（呼び出し中に `ScriptError::Denied`。outcome は `ts_error` で、安全違反にも数える）。
  - `lane_rules`: `ceiling_exceeded`（TS の `proposed` に Rust と同じ `clamp` を当てた lane が天井で許されないのに、Rust の lane は許される）。
  - `route_priority`: `undeclared_candidate`（TS の候補が入力 `table` のどの列にも無い。(a) の「registry に在る route だけを残す検査」に当たる）。
  - TS を呼ぶ前提の破れ（`Human`・`System` の task で TS を呼んだ）は Rust の bug なので code にせず、試験で落とす。
- 数え方: 領域と generation ごとに、`safety_violations` が空でない event の件数を数える（1 event に code が複数あっても 1 件）。
  分母は同じ領域・generation の `outcome != "unavailable"` の event の件数（S2-4 の重複の抑えの後）。不一致率・timeout 率・失敗率も同じ分母で数える。
  - timeout: `ts_error` で `error.kind` が `timeout`・`terminated`。
  - 失敗: `ts_error` のそれ以外と `invalid_output`。
  - 説明のつかない不一致: `mismatch` のすべて（S2 は正規化が無いので、説明は S2-2 の規則を足す変更でしか付かない）。
- 「Rust の検証を通らずに副作用が実行された」は compare では構造上起きない。S2-6 の byte 一致の試験で固定し、起きたら S5 全体を止める（(f)）。

### S2-9 host-adapter の実装で決めたこと（2026-11-02）
記録は [S2 host-adapter](../progress/2026-11-02-ts-control-layer-s2/host-adapter.md)。
- S2-4 の「検証に落ちたら `rejected`」は S2 では行を作らない。`script_generation_record` は同じ hash の行を再利用し、`rejected` から `loaded` へ戻す経路が store に無い。
  ファイルが一時的に無いだけで `rejected` の行を作ると、その hash を二度と `compare` にできなくなる。拒否の理由は既存の通知（`script_bundle_rejected`）に残る。
- S2-5 の起動時の戻しは、generation の bundle と読み込めた bundle の hash の比較に一本化する。`[script]` が無い・bundle を読めない場合は「読み込めた bundle なし」として戻す。
  `host_api` の不一致は manifest の検証で load が失敗するので、この場合に含まれる。
- `Denied` の S2-8 の code は host の op の理由文で分ける。`outside the request scope` を含めば `denied_scope`、それ以外は `denied_capability`。

### S2-10 route_priority の basis の規則（2026-11-02）
記録は [S2 route-prio core](../progress/2026-11-02-ts-control-layer-s2/route-prio/core.md)。
- `llm_route::route_priority_decide(registry, harness, lane, cos, execution_class) -> (RoutePriorityBasis, &[CandidateKey])` が列と basis を返す。`route_priority` は同じ signature の薄い包みで、列だけを返す（挙動不変）。
- 列の選び方は分ける前と同じ。上から最初に当たった列を使う。
  1. `cos == true` → `cos_route_candidates`（basis `cos`）。
  2. `execution_class` があり、その harness に同じ class の行が宣言されている → その行（basis `execution_class`）。
  3. harness×lane の行が宣言されている → その行（basis `lane`）。lane が `null` のときは空文字 `""` の lane として引く（`unwrap_or_default`）。したがって `""` の行があれば lane `null` でもその行が当たる。
  4. harness 全体の `route_defaults` が宣言されている → その行（basis `harness`）。
  5. どれも無い → 空（basis `none`）。
- 選んだ列が空なら、列の種類によらず basis は `none` にする。空の列は宣言されていても下の列へ落ちない（空の class 行・空の lane 行・空の `cos_route_candidates` は、そのまま空の出力になる）。
- 未知の class（その harness に行が無い class）は 2 に当たらず 3 以降へ進む。未知の lane も同じく 4 へ進む。
- 候補の重複は除かない。列の要素はそのままの順で返す。

### S2-11 route_priority の compare の結線（2026-11-02）
記録は [S2 route-prio hook](../progress/2026-11-02-ts-control-layer-s2/route-prio/hook.md)。S2-1 の「`route_select_one` の最初の `route_priority` の 1 か所」を次のように実装する。
- `route_select`（`route_select.rs` の純粋関数）は変えない。結線は `dispatch_run.rs` の `select_route_with` の直後の 1 か所だけで、
  `Dispatcher::compare_route_priority(task_id, run_id, route_input, route_trace)`（本体は `dispatcher/script_compare.rs`）を呼ぶ。
  `review_spawn` の reviewer の route 選択、`lane_assist` の試走（lane ごとの `select_route`）、CoS chat の route 選択、
  `order_by_headroom` の得点の `route_priority`、frontier 列の後ろの列の `route_priority` では呼ばない。
- `route_input.cos == true` なら比べない（dispatch_run の CoS task の run も含む）。
- 1 判定で 1 件だけ記録する。比べる harness は `RouteTrace.harness`（選んだ harness、全部が断ったときは最後に試した harness）。
  `None`（harness の選択そのものが失敗）なら記録しない。failover で試した harness を全部記録しないのは、毎回 2 件になり
  S2-4 の「同じ task・同じ domain の直前の記録」との重複の抑えが効かなくなるため。
- 入力 DTO と Rust の出力は、その harness・`route_input.lane`・`cos = false`・`route_input.harness.execution_class` で
  `RoutePriorityInput::redacted` と `route_priority_decide` を当て直して作る（`route_select_one` の最初の `route_priority` と同じ引数）。
  比べるのはこの列で、frontier 列を前に足した後の `RouteTrace.candidates` ではない。
- `call_site` は `route_select.route_select_one`、`run_id` は dispatch_run が作った run の id。`input_digest` は run id を含まないので、
  同じ入力の判定が run ごとに繰り返されても重複の抑えが効く。
- mode が `rust` なら DTO を作らず TS を呼ばない。mode は tick ごとに 1 回、両領域の行を一度に読む。generation の確認・`unavailable`・
  deadline・in-flight の上限（両領域の合計で `MAX_IN_FLIGHT_COMPARES`）・記録は `lane_rules` と同じ経路を通る。dispatcher は TS を待たない。
- 空の列（basis `none`）も記録する。route はそのまま `NoDeclaredDefault` で断る（TS が候補を返しても変わらない）。
- `classify_route_priority` の判定: `basis` と `candidates` の byte 一致で `match`／`mismatch`、候補 256 件超・未知の basis・
  未知の欄は `invalid_output`。TS の候補のどれかが入力 `table` のどの列（`route_defaults`・`lane_route_defaults`・`class_defaults`・
  `cos_route_candidates`）にも無ければ、outcome は `mismatch` のまま `safety_violations` に `undeclared_candidate` を入れる。

### S2-12 実装との突き合わせ（S2 close、2026-11-02）
記録は [S2 の進捗](../progress/2026-11-02-ts-control-layer-s2.md)。S2-1〜S2-11 を実装と突き合わせた結果。違いは 4 つで、どれも安全側か範囲の明確化。

| 節 | 実装 | 付記との違い |
|---|---|---|
| S2-1 | `lane_rules`（`dispatch_run.rs` の `decide_lane`）、`route_priority`（`dispatch_run.rs` の `select_route_with` の直後）。`direct_route` は Rust のまま | `route_priority` の結線の場所は S2-11 で `route_select.rs` の中から `dispatch_run.rs` へ決め直した（`route_select` は不変） |
| S2-2 | `LaneRulesOutput`（`task_dispatch::script_policy`）、`RoutePriorityOutput`（`task_core::script_compare`）。全 struct `deny_unknown_fields`、正規化なし（`normalized` は常に `[]`） | TS の型は手書きで、`host-v1.schema.json` からの生成は未実施（S2 進捗の未解決 1） |
| S2-3 | `LaneRulesInput::redacted`・`RoutePriorityInput::redacted`・`recorded_input`（64 KiB）・`truncate_error_message`（512 byte） | なし |
| S2-4 | migration `0103_script_domains.sql`、`Event::ScriptCompareRecorded`・`ScriptDomainModeChanged`、`script_compare_append` の重複の抑え | `rejected` の行は作らない（S2-9） |
| S2-5 | `GET /api/v1/script/domains`・`PUT /api/v1/script/domains/{domain}/mode`、CoS 操作 `script_domain.mode_set`、`celerisctl script domains`・`script mode`、起動時の戻し（`system:script-startup`） | なし |
| S2-6 | `call_lane_rules`・`call_route_priority` の `tokio::time::timeout(50 ms)`、in-flight 上限 32（両領域の合計） | (1) dispatcher は TS を待たない（0 ms）。`tick()` は同期の関数で、待つには `block_in_place` が要り `current_thread` の runtime では panic するため。記録は判定より少し遅れて追記される。Rust の結果と dispatch の速さは compare の有無で変わらない |
| S2-7 | 要求の scope は空。両領域の capability は空（`bundle.config.json`） | なし |
| S2-8 | `denied_capability`・`denied_scope`（S2-9 の理由文で分ける）・`ceiling_exceeded`・`undeclared_candidate` | (2) `ceiling_exceeded` は今の `LaneCeiling::clamp` では起きない（許される lane が 1 つでもあれば clamp はそれを返す）。試験は「違反なし」だけを確かめる |
| S2-9〜S2-11 | 付記のとおり | (3) compare の記録の `run_id` は `lane_rules` では `null`（`decide_lane` は run の id を作る前に走る）、`route_priority` では run の id |
| 試験の dist | 本物の host の試験は `control-ts/dist` の manifest の domains が `bundle.config.json` と違えば SKIP、`CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で失敗 | (4) `release.sh`・`test-affected.sh` は dist を作らないので、dist の無い環境では 5 件が SKIP で通る。release gate への組み込みは別 task の候補 |

## 付記 2026-11-02: S3 の高速検証経路との違い（S3 close、task 01M6EVP95S9VM3DBMZFRD8FS35）
根拠は [TS だけの変更の高速検証の ADR](2026-11-02-ts-fast-verify.md) と [S3 の進捗](../progress/2026-11-02-ts-control-layer-s3.md)。

- **(f) の edit→activate の定義**: S3 の計測（`policy-ts`）では「generation の load と `compare` での有効化」を、`script-host-runner activate` による一時 state dir への配備と load で代用した。
  DB の `script_generations` の行と `script_domain_modes` の CAS は含まない。これらは Rust のコードで、TS だけの変更では変わらず、`crates/celeris/tests/script_compare_*` が受け持つ。
  (f) の目標の判定はこの代用で行った（warm p50 5.031 秒・p95 7.406 秒、cold p95 11.525 秒。全 40 試行で cargo の起動 0 回・`link_count` 0）。
- **(f) の cold**: S3 の cold は `control-ts/node_modules` を消した状態で、runner の cache は当たる。runner の作り直し（約 60 秒）を含む cold は測っていない。
- **S2-7 の「許可表が空の領域の manifest が `capabilities` を宣言していたら load で拒否する」は実装されていない**（S2-12 の表は S2-7 の違いを「なし」としたが、この点が違う）。
  `crates/celeris/src/daemon/script*.rs`・`crates/script-runtime/src/manifest.rs` にこの拒否は無い。manifest の `capabilities` は全領域の和で、領域ごとの宣言を持たないので、この形のままでは検査できない。
  今の守りは次の 2 つで、どちらも S2-7 の拒否とは別の物である。
  1. 実効の権限は `Grant::new` が「manifest の宣言 ∩ 領域の許可表」に絞る。`lane_rules`・`route_priority` の許可表は空なので、宣言があっても op を呼べない。
  2. 検証の経路では、`script-host-runner` の安全シナリオが「和 ⊆ 各領域の許可表の和」を検査し、外れると exit 3 にする。daemon の経路にこの検査は無い。
  S2-7 の末尾「要求ごとの context の分離を決めるまで、どの領域にも capability を許可しない」は変えない。拒否を実装するか S2-7 からこの文を消すかは、op を使う最初の領域（S4 の digest）の前に決める（S3 の進捗の提案）。

## 付記 2026-11-02: S4 の CoS digest 経路（S4 adr-s4、task 01M6EVP9GT07K2G01EMW0E6HNC）
S4 の作業の前に、CoS の非同期経路 1 つを TS へ同等移行する仕様を決める。根拠は [S0 の計画](../progress/2026-11-02-ts-control-layer-s0.md) §2 S4、
[S0 inventory §4](../progress/2026-11-02-ts-control-layer-s0/inventory.md)、S2 の付記（S2-1〜S2-12）、`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`、
`crates/task-core/src/chat/triage.rs`（`cos_triage_runs_without_digest`・`cos_triage_requeue_run`）、`crates/task-core/src/chat/store.rs`（`chat_assistant_message_add_once`）。
記録は [S4 adr-s4](../progress/2026-11-02-ts-control-layer-s4/adr-s4.md)。この付記はコードを変えない。
用語: この付記の「compare」は S2 の `compare` mode のこと。旧 `routing_shadow`（表・`RoutingShadowRecorded`・`routing_shadow_history`・旧 `[model_routing]`・tier binding）は使わない。

### S4-1 移す経路と理由
- 移すのは、受信箱一次対応の **digest pass 全体**: `CosChatLaunch::triage_digest`（`digest_due` の立った tick で走る）→ `digest_one`。
  triage run の終了 → Rust が `cos_triage_runs_without_digest` で未 digest の run を読む → `cos_triage_run_report`・`chat_run_get` で snapshot を作る →
  **TS が判定する（text・cards の組み立てと、stopped/interrupted の requeue をするかの判断）** → Rust が検証して apply（message 投入、requeue）。
- S0 の計画は `digest_message` の文面だけを移す案だった。それでは「単なる rule 関数」になるので、requeue をするかの判断も出力 command に含める（S0 から範囲を広げた点）。
  requeue の SQL・message の書き込み・`digest_due`・`dirty` の更新・走査・20 件の上限は Rust のまま。
- 選んだ理由:
  1. 境界が既にある。`digest_message` は I/O と時計を持たず、`digest_one` の副作用は 2 つ（message 投入、requeue）だけ。前段の Rust の変更は「判定を値にして返す」分割で、挙動は変えない。
  2. 実運用の非同期の流れ（run の終了、tick をまたぐ再配送、write の失敗の retry、人の割り込み、daemon の再起動後の回収）を全部通る。試験が既に 11 件ある（`cos_chat_triage_inbox_thread.rs`）。
  3. 安全条件に触れない。認可・予算・human_required・`policy_version` の検査は digest の外にある。
  4. 変更頻度が文面側に偏る（日本語の文面と card の調整）。S3 の高速経路の効果が出る場所である。
- B（定期見直しの入力文）を後に回す理由: 冪等 key の確認・`project_brief_list` の読み出しが判定の前に混じり、先に store 読み出しを外へ出す Rust の変更が要る。
- C（取り込み分類）を後に回す理由: cursor・挿入 transaction・`policy_version` に近く、入力が `EventRow`・`InboxItem`・`Notice` と大きい。変更頻度は最大だが、S4 の最初には重い。
- D（CoS 不在時の退避）は Rust のまま。期限判定が安全網（ADR 2026-10-27 系の fallback）で、文面だけを切り出す価値が小さい。
- B・C・D への広げ方は S4 の対象外。

### S4-2 領域 `cos_digest`・DTO・fixture
DTO の正本は Rust の型。全 struct に `deny_unknown_fields`、`docs/api/script-host/host-v1.schema.json` に足す。
- 領域名 `cos_digest`。manifest の `domains` に載せる。`capabilities` は空（S4-6）。
- 入力 `CosDigestInput { thread: string, run_id: string, run_state: "completed"|"failed"|"stopped"|"interrupted", reason: string | null, items: CosTriageItemReport[] }`。
  `items` は `cos_triage_run_report(run_id)` の並びのまま。`CosTriageItemReport`（`task_core::chat::triage`）に `Serialize`・`Deserialize`・`JsonSchema`・`deny_unknown_fields` を足す（前段、別 commit）。
  `reason` は `chat_run_get(thread, run_id)` の `reason`（読めなければ `null`）。時刻・乱数・`policy_version` の欄は置かない。
- 出力 `CosDigestOutput { commands: DigestCommand[] }`。`DigestCommand` は `kind` で分かれる:
  - `{ "kind": "post_digest", "thread": string, "idempotency_key": string, "text": string, "cards": ChatCard[], "run_id": string }`
  - `{ "kind": "requeue_run", "run_id": string }`
- Rust の参照実装の出力: 常に `post_digest` を 1 件、`run_state` が `stopped`・`interrupted` ならその後ろに `requeue_run` を 1 件。順序は今の `digest_one` と同じ（投入が先）。
- fixture の 1 行は `{ "name": string, "input": CosDigestInput, "commands": DigestCommand[] }`（`control-ts/fixtures/cos_digest/*.jsonl`）。`commands` は Rust の参照実装が書き出した期待値で、TS の手書きにしない。
- 比べる欄: `commands` の全要素（`text` と `cards` を含む byte 一致、順序込み）。正規化は S2-2 と同じく「なし」。

### S4-3 Rust の検証規則と安全違反
Rust は TS の output を apply の前に次で検証する。1 つでも落ちたら、その run の TS の output 全体を使わない（apply しない）。
1. `post_digest.idempotency_key == COS_TRIAGE_DIGEST_KEY_PREFIX + run_id`（`cos-triage-digest:<run_id>`）。
2. `post_digest.thread` と `run_id`、`requeue_run.run_id` が入力 snapshot と一致する。
3. 全 `ChatCard.id` が `items[].source_key` の部分集合。card の数は `items` の数以下。
4. `requeue_run` は `run_state` が `stopped`・`interrupted` のときだけ。
5. run ごとに `post_digest` と `requeue_run` はそれぞれ高々 1 件。
6. `post_digest` が無い output は不正（digest は run ごとに必ず 1 件）。`text` は `CHAT_MESSAGE_TEXT_MAX_BYTES` 以下、`idempotency_key` は `CHAT_CLIENT_KEY_MAX_BYTES` 以下。
- 数え方（S2-8 と同じ枠）: 検証落ちは outcome を `invalid_output` にし、`safety_violations` に code を入れる。
  code: `key_mismatch`（1）・`scope_mismatch`（2）・`card_outside_items`（3）・`requeue_state_invalid`（4）・`duplicate_command`（5）。6 は code を持たず `invalid_output` の理由文だけ（安全でなく形式の違反）。
  TS が呼んだ op の拒否は S2-8 の `denied_capability`・`denied_scope`。空でない event の件数で数え、分母は `outcome != unavailable` の件数（S2-8 と同じ）。
- 検証は TS の output にだけ当てる。Rust の参照実装の output も同じ検証関数を試験で通す（参照実装が検証に落ちたら Rust の bug。code にせず試験で落とす）。
- 既存の安全検査は動かさない: `cos_triage_requeue_run` の `WHERE state='running'`、message の冪等 key、`digest_due` を空の走査で初めて下ろす規則。

### S4-4 compare と apply
- compare は Rust の command を **常に apply し、TS を待たない**（S2-12 の (1) と同じ）。`digest_one` は snapshot → Rust の参照実装 → apply の順で今と同じに動き、その後で別の tokio task が TS を 50 ms の deadline で呼ぶ。
- 結果は S4-5 の log に追記する。追記の失敗・TS の失敗・timeout・runtime 不在・generation 不在・mode `rust` のどれでも、digest pass は止めず Rust の結果のまま。
- mode は S2 の表 `script_domain_modes` の行（`cos_digest` を migration で `rust`・version 0 で足す）。tick ごとに 1 回読む。S2-5 と同じく S4 で受け付けるのは `rust` と `compare` だけ。
- in-flight の上限は S2 の `MAX_IN_FLIGHT_COMPARES`（全領域の合計 32）を共用する。digest pass は 1 回に最大 20 run なので、上限を超えた run は記録を落とす（件数を `unavailable` として数えない。log の欠けとして warn する）。S5 の分母は log の行数。
- `canary`・`ts` は S5。S4 の TS の output は apply されない。

### S4-5 比較入力の保存規則
- 追記先: `events` ではなく新しい追記専用の表 `script_compare_log`（migration は hook unit。番号はその時の空き）。理由は、`events.task_id` が NOT NULL で digest が task でなく thread・run に属すること（S2-4）。
  列は `id` INTEGER PK、`ts`、`domain`、`thread_id`、`run_id`、`json`。UPDATE・DELETE の経路を作らない。`script_compare_recorded` の event は使わない。
- `json` の欄は S2-4 の `script_compare_recorded` と同じ（`domain`・`generation`・`isolate_epoch`・`bundle_sha256`・`host_api`・`call_site`＝`cos_digest.digest_one`・`run_id`・`outcome`・`error`・`normalized`・`safety_violations`・`elapsed_us`）に、次の違いを持つ。
  - `input` は常に `null`。`input_digest`（`sha256:` + S4-2 の入力の直列化の sha256）と `thread_id`・`run_id` だけを残す。`input_truncated` は欄ごと無い。
  - `rust`・`ts` は `commands` を直接ではなく `commands_digest`（`sha256:` + 直列化の sha256）と `command_kinds`（`["post_digest","requeue_run"]` の並び）で残す。本文は残さない。`mismatch` のときだけ `ts_commands_head`（TS の `text` の先頭 512 byte）を足す。
  - `applied: { message_id: string, requeued: number }`（Rust が実際に apply した結果）。
- 本文を残さない理由: items の `summary`・`detail`・`reason` は task・案件の本文で、S2-3 の「本文を捨てる」に当たる。同じ本文は chat の message と `cos_inbox_items` に既にあり、log に写すと二重に持つ。
- replay: 本番では、`thread_id`・`run_id` から store で入力を組み直し（`cos_triage_run_report`・`chat_run_get`）、`input_digest` が log と同じ run だけを再実行する。違う run（digest 後に item の state が変わった）は `drifted` として数え、不一致に入れない。
  試験用の記録済み trace は、trace-capture unit が dispatcher の本物の経路（fake adapter で triage run を終えさせる）から書き出し、`control-ts/fixtures/cos_digest/` に本文つきで置く。これは repo の試験 fixture で、本番の log ではない。
- 重複の抑え: 同じ `thread_id`・`run_id` に、同じ `generation`・`input_digest`・`outcome` の行は足さない。digest pass の再配送（tick の繰り返し）で行が増えない。

### S4-6 async の読み出し・LLM を bridge しない理由
- digest が使う入力は、Rust が TS を呼ぶ前に作る snapshot で全部そろう。TS は `CosDigestInput` だけを読み、store・chat・process・network・LLM を呼ばない。
  非同期なのは経路（run の終了 → tick → 読み出し → 判定 → 書き込み、tick をまたぐ retry・再配送・再起動）で、TS の中の判定は同期の純粋関数である。経路の主体（`digest_due`・走査・apply）は Rust のまま。
- したがって S4 では op と capability を足さない。`cos_digest` の許可表は空、要求の `scope_task_ids` も空。manifest が `cos_digest` に `capabilities` を宣言したら load で拒否する（S2-7 と同じ。`script_generations` に `rejected` の行は作らない。S2-9）。
- S2-7 の「要求ごとの V8 context の分離は op を使う最初の領域より前に決める」は、S4 では op を使わないので **不要のまま**。分離を決めるのは、最初に op を使う領域（B・C の拡張、または S6）の前に延ばす。
- 隠れた入力（global に値を残す）の対策は S2-7 と同じ。fixture runner が全件を正順・逆順の 2 回流し、同じ出力を要求する。本番の compare の差は `mismatch` に出る。
- LLM は digest 経路に無い（triage run は終わっていて、digest は記録済みの run の結果を読むだけ）。記録済みの LLM 応答の replay は、triage run の結果が `cos_inbox_items` に記録されている形で行う。live で LLM を二重に呼ばない。

### S4-7 行為の追跡
- 新しい event kind は足さない（`Event` の variant を変えず、task-api の query・web/api の生成型に波及させない）。
- post_digest の実行記録は chat の message（`chat_messages` の `client_message_id = cos-triage-digest:<run_id>`、`run_id` 付き）。冪等 key が thread 内で一意なので、再配送・retry・再起動で 2 件目は作られない。
- requeue の実行記録は `cos_inbox_items` の行（`state` が `pending`、`run_id` が `NULL`）と S4-5 の `applied.requeued`。`WHERE run_id=? AND state='running'` なので繰り返しても 0 件で、冪等。
- compare の記録は S4-5 の `script_compare_log`。以上で足りる。S5 の `ts` mode で TS の command を apply するようになったら、`applied` と同じ欄で「TS の command を apply した」ことを `script_compare_log` に残す（S5 で決める。S4 では行わない）。

### S4-8 試験の表
旧 `routing_shadow` の名前は試験名・fixture 名・表名のどれにも使わない。試験は `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`（robustness unit）と `control-ts/test/cos_digest.test.ts`。
| 観点 | 場面 | 期待 |
|---|---|---|
| 順序 | 1 pass に 2 run 以上。stopped の run | `post_digest` の後に `requeue_run`。投入が失敗したら requeue しない（今の `?` と同じ）。run の並びは `started_at`・`run_id` 順 |
| 再配送 | `digest_due` が複数回立つ。同じ run が 2 pass に現れる | message は 1 件。`script_compare_log` は同じ `input_digest`・`outcome` で 1 行 |
| retry | message の書き込み失敗、走査の失敗、20 件超の drain | 次の tick で再試行。`digest_due` は空の走査まで残る。compare の有無で結果が同じ |
| cancel | stopped・interrupted の run、人の割り込み | items が `pending` に戻り、人の message が先に走る。completed・failed の run は requeue しない（`requeue_state_invalid` を TS が出したら検証で落ちる） |
| 再起動 | daemon 再起動後の reconcile、起動後の orphan 回収 | `digest_due` が立ち、message は 1 件のまま。mode `compare` の復元（`script_domain_modes`）後に log が続く |
| policy_version・session freshness | `[cos.triage] policy_version` の変更で session が fresh になった後の run | digest の入力に `policy_version` は無く、run_id が key。版が変わる前の run の digest も 1 件で、文面が変わらない。版の整合の 422 は動かさない |
| generation 切替 | pass の途中で mode が `rust` ↔ `compare`、generation の差し替え、`GenerationGone` | Rust の message・requeue は byte 一致。log の `generation` が呼んだ版を指す。TS 失敗は `ts_error`、digest pass は止まらない |
| 同等性 | 11 件の既存入力の replay | `commands` が byte 一致。fixture を正順・逆順の 2 回流して一致 |
| 安全 | 検証規則 1〜6 を 1 件ずつ破る bundle | `invalid_output` と対応する code。apply されない |
| 結果不変 | 常に違う答えを返す bundle（compare） | `chat_messages`・`cos_inbox_items` が `rust` のときと byte 一致 |

### S4-9 Rust の分割（rust-split unit が行う。挙動不変）
- `digest_one` を 4 つに分ける: snapshot の組み立て（`cos_triage_run_report`・`chat_run_get`）→ `digest_decide(&CosDigestInput) -> CosDigestOutput`（純粋、`digest_message` を使う）→ `validate_digest_output(&CosDigestInput, &CosDigestOutput) -> Result<(), DigestViolation>` → `apply_digest_commands`。
- 分割の commit では出力 message・requeue・log の無い今の挙動を変えない。11 件の既存試験がそのまま通ることを確かめる。

### S4-10 実装との突き合わせ（S4 close、2026-11-02）
記録は [S4 の進捗](../progress/2026-11-02-ts-control-layer-s4.md)。S4-1〜S4-9 を実装と突き合わせた結果。挙動に関わる違いは無く、名前・置き場所・試験の範囲の違いである。

| 節 | 実装 | 付記との違い |
|---|---|---|
| S4-1 | `digest_one` は `digest_snapshot`→`cos_digest_decide`→`validate_cos_digest_commands`→`apply_digest_commands`。TS は `control-ts/src/domains/cos_digest.ts` | なし |
| S4-2 | DTO は `crates/task-core/src/chat/digest.rs`。出力の包み `CosDigestOutput` は `task_dispatch::script_policy`。command の型は `CosDigestCommand` | (1) `run_state` は 4 値の enum でなく文字列。今の文面が未知の値を `run は <state> で終わった` と書くので、enum にすると挙動が変わる。TS も文字列で受ける。(2) `host-v1.schema.json` に DTO を足していない。S2 の領域と揃え、host の呼び出しの包みだけを載せる。(3) fixture は `control-ts/fixtures/cos_digest/{cases,traces}.jsonl` |
| S4-3 | `validate_cos_digest_commands`、`CosDigestViolation::code()`。`classify_cos_digest` が検証落ちを `invalid_output` にして code を `safety_violations` に入れる | (4) 規則 6 の違反は `MissingPostDigest`・`TextTooLarge`・`KeyTooLarge` の 3 つに分けた（どれも code なし） |
| S4-4 | `Dispatcher::compare_cos_digest`。`SCRIPT_CALL_DEADLINE`（50 ms）、`MAX_IN_FLIGHT_COMPARES`（合計 32）。上限を超えた run は warn の log だけで記録を落とす | (5) 記録は apply が全部成功した run だけに作る。message の後で requeue が失敗した run は digest 済みになるので、その run の記録は作られない |
| S4-5 | migration `0104_script_cos_digest.sql`、`CosDigestCompareRecord`（`script_compare/cos_digest_record.rs`）、`script_compare_log_append` の重複の抑え | (6) `applied.message_id` は、冪等 key で既にあった message の id にもなる。(7) replay は試験の中だけにあり、本番の log の replay の道具は無い。stopped・interrupted の run は requeue で item の `run_id` が消え、組み直した入力の `input_digest` が必ず合わないので常に `drifted` になる（`script_compare_cos_digest_dispatch_match_replay_and_rollback`）。本番の log の replay で確かめられるのは requeue の無い run だけで、requeue の判断の同等性は fixture の replay で確かめる |
| S4-6 | `bundle.config.json` の `cos_digest` は `[]`。scope は空 | なし |
| S4-7 | 新しい event の variant なし。行為の追跡は `chat_messages`・`cos_inbox_items`・`script_compare_log` | なし |
| S4-8 | `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`（11 件）、`crates/task-dispatch/src/dispatcher/tests/cos_chat_triage_digest_script_compare.rs`（10 件）、`crates/celeris/tests/script_compare_cos_digest.rs`（5 件）、`control-ts/src/domains/cos_digest.test.ts` | (8) TS の試験の置き場所は `control-ts/test/` でなく `control-ts/src/domains/`（他の領域と同じ）。(9) 同等性の行は既存 11 件の入力でなく、cases 2,066 行と traces 17 行で確かめた。(10) 安全の行は「規則を 1 件ずつ破る bundle」でなく、検証関数を 14 通りの command で試し（`script_fixtures_cos_digest_validation_rejects_each_rule`）、dispatcher の経路では偽の `PolicyScript` で 3 つの code（`card_outside_items`・`key_mismatch`・`requeue_state_invalid`）を試す。`scope_mismatch`・`duplicate_command`・digest の場面の `generation_gone` は dispatcher の経路で試していない。(11) item state の `handled` は `cos_inbox_items` の CHECK の外にある未知の state で、試験だけで作る |
| S4-9 | (S4-1 の行のとおり) | (12) 名前は `digest_decide`・`validate_digest_output`・`DigestViolation` でなく `cos_digest_decide(&CosDigestInput) -> Vec<CosDigestCommand>`・`validate_cos_digest_commands`・`CosDigestViolation`。置き場所は dispatcher でなく task-core |

## 付記 2026-11-02: S4 の digest pass 全体を TS の async 経路にする（S4 adr-async、task 01M6EVP9GT07K2G01EMW0E6HNC）
前回の final review は S4 の受け入れ条件 0 を不合格にした。理由は次の 3 点である。TS に移ったのは同期の `decide()` だけである。async の読み出しも apply も Rust に残っている。TS の command はどの mode でも Rust の検証→apply に渡らない。
この付記は、その 3 点を解消する仕様を決める。コードは変えない。記録は [S4 adr-async](../progress/2026-11-02-ts-control-layer-s4/adr-async.md)。
S4-1〜S4-10 のうち、この付記が改める点は S4-11 の表にまとめる。改めない点は元の節のまま有効である。用語は S4 の付記と同じ。旧 `routing_shadow` は使わない。

### S4-11 S4-1〜S4-10 のうち改める点
| 元の節 | 元の決定 | 改めた決定 | 節 |
|---|---|---|---|
| S4-1 | TS は run ごとの判定だけを行う。走査・読み出し・apply は Rust が行う | TS の `cosDigestPass(ctx)` が pass 全体を動かす。走査と読み出しは op を await して行う。DB の更新は今までどおり Rust の検証→apply だけが行う | S4-12 |
| S4-2 | 入口は run ごとの `cos_digest(CosDigestInput) -> { commands }` | 入口は pass ごとの `cosDigestPass(ctx) -> CosDigestPassOutput`。`CosDigestInput`・`CosDigestCommand` は op の応答と run ごとの command としてそのまま使う | S4-12 |
| S4-3 | 検証規則は 1〜6 | 規則 1〜6 を run ごとに当てる。pass には規則 7〜9 を足す | S4-14 |
| S4-4 | compare だけを行う。TS は run ごとに別の tokio task で呼ぶ。`canary`・`ts` は S5 で扱う | mode `compare` は pass 1 回を TS へ 1 回送る。TS の op は Rust が読んだ snapshot から答える。`cos_digest` に限って mode `ts` を S4 で受ける。`canary` は S5 のまま | S4-14 |
| S4-5 | `script_compare_log` は 1 run 1 行。欄は S4-5 の表のとおり | 1 run 1 行は変えない。`applied_by`・`fallback`・`mode`・`pass_id`・`ops` の欄を足す | S4-15 |
| S4-6 | op・capability を足さない。S2-7 の context 分離は決めない | op 2 つと capability 2 つを足す。scope と要求ごとの分離をここで決める。S4-6 の本文は全部置き換える | S4-13 |
| S4-7 | TS の command を apply した記録は S5 で決める | `applied_by` で記録する。新しい event の variant は足さない（この点は S4-7 のまま） | S4-15 |
| S4-8 | `cases.jsonl`・`traces.jsonl` を run ごとに replay する | pass の trace `pass_traces.jsonl` を fake host で replay する。`traces.jsonl` と run ごとの trace の書き出しは消す | S4-16・S4-17 |
| S4-9 | `digest_one` を 4 つに分ける | 分けた 4 つは残す。読み出しを pass の memo（S4-12）へ移し、apply を pass の最後にまとめる | S4-12 |

### S4-12 pass の流れ（目的の (1)）
- **Rust に残るもの（S4-6 を改める）**: `digest_due`・`dirty`・pass の起点（`triage_launch` の tick で `digest_due` が立っていること）・次の tick での retry・20 件の上限・空の走査で `digest_due` を下ろす規則・message の書き込みと requeue の SQL。scheduler の実行責任はこれまでどおり Rust が持つ。
- **TS に移るもの**: pass の中の手順。TS が list を読み、run ごとに read し、run ごとの command 列を決めて返す。読む順と、read が失敗した run を飛ばす判断も TS が持つ。
- TS の入口: `export async function cosDigestPass(ctx: RequestContext): Promise<CosDigestPassOutput>`（`control-ts/src/domains/cos_digest.ts`）。run ごとの判定は既存の純粋関数 `decide()` を使う。
- op（Host API 1.1.0。op の追加なので minor を上げる。(c) の規則による）:
  | op（JS の関数 / native op） | 要求 | 応答 | Rust が行う読み出し |
  |---|---|---|---|
  | `cos_digest:list_runs`（`cosDigestListRuns` / `op_cos_digest_list_runs`） | `CosDigestListRunsRequest {}`（欄なし） | `CosDigestListRunsReply { runs: { thread, run_id, run_state }[] }` | `cos_triage_runs_without_digest(20)`。上限は Rust の定数で、TS は変えられない |
  | `cos_digest:read_run`（`cosDigestReadRun` / `op_cos_digest_read_run`） | `CosDigestReadRunRequest { thread, run_id }` | `CosDigestInput`（S4-2 の型のまま） | `cos_triage_run_report(run_id)` と `chat_run_get(thread, run_id)`。後者の失敗は今と同じく `reason: null`。前者の失敗は op の error（code `exception`） |
- 出力 `CosDigestPassOutput { runs: { run_id: string, commands: CosDigestCommand[] }[] }`。全 struct に `deny_unknown_fields` を付ける。DTO の正本は task-core の Rust の型で、`docs/api/script-host/host-v1.schema.json` に足す。
- **pass の memo**: Rust は pass 1 回ごとに `CosDigestPassReads` を 1 つ作る。読み出しはすべて memo を通す。同じ要求の 2 回目は store を読まず、memo の答えを返す。1 pass で store を読むのは、list が 1 回、run ごとの read が 1 回ずつまでである。
  memo は要求と応答の組を最初に読んだ順に持つ。これが S4-15 の `ops` と S4-16 の trace の元になる。
- **op の運び方**（(b) の「op は Rust 側の bounded channel を通して既存の tokio サービスへ渡す」の具体化）: native op は引数を JSON のまま要求の host sink（容量 64 の bounded `mpsc`、応答は要求ごとの `oneshot`）へ送る。
  dispatcher は TS の呼び出しと op の server（memo と store を持つ future）を同じ async fn の中で並べて await する。store を V8 の thread に渡さない。`celeris-script` は task-core・store の型を知らない。op の要求と応答は dispatcher 側で DTO に deserialize して検証する。
- `PolicyScript::cos_digest(input, deadline)` を `PolicyScript::cos_digest_pass(call: CosDigestPassCall, deadline) -> PolicyFuture` に置き換える。`CosDigestPassCall` は `pass_id`（ULID）と host sink の送り口を持つ。
- pass の deadline は 1 pass で 200 ms（`COS_DIGEST_PASS_DEADLINE`）。op の待ちも含む。純粋判定の 1 呼び出し 50 ms（(f)・S2-6）は変えない。dispatcher が 1 tick で TS を待つのは mode `ts` のときだけで、待つ時間は最大 200 ms。
- 1 pass の op 呼び出しは 64 回まで。65 回目以降の op は `busy` を返す。TS は時計・乱数を持たない（S1 の付記）。checkpoint は使わない。pass は 1 tick の中で終わり、op の間に永続の状態を作らないので、`checkpoint_versions` は `[]` のままにする。

### S4-13 capability・scope・要求ごとの分離（目的の (2)。S4-6・S2-7 を改める）
- capability は `cos_digest:list_runs` と `cos_digest:read_run` の 2 つ。`crates/script-runtime/src/ops.rs` の許可表に `("cos_digest", [CosDigestListRuns, CosDigestReadRun])` を足す。
  manifest（`control-ts/bundle.config.json`）の `cos_digest` もこの 2 つを宣言する。有効になるのは (c) の「宣言 ∩ 許可表」だけである。S4-6 の「`cos_digest` が capability を宣言したら load で拒否する」は削除する。他の領域（`lane_rules`・`route_priority`）の許可表は空のまま。
- grant は要求の token と `domain` に結び付ける。op は自分の領域（`cos_digest:*` なら `cos_digest`）の grant の token でだけ通る。他の領域の要求の token では `denied`（`denied_capability`）になる。
- scope は pass ごとに Rust の op server が持つ。`scope_task_ids` は空のまま使わない（digest は task でなく thread・run に属する）。
  - `list_runs` は pass の中で何回呼んでも、memo の同じ答えを返す。
  - `read_run` は、その pass の `list_runs` の答えにある `(thread, run_id)` の組だけを読める。`list_runs` の前の呼び出しや、答えに無い組の呼び出しは `denied` になり、`safety_violations` に `denied_scope` を入れる（S2-8）。組の照合は thread と run_id の両方で行う。
- **要求ごとの V8 context の分離（S2-7 の宿題の答え）**: V8 context は要求ごとに分けない。代わりに次の 4 つで、S1 の未解決 4（並行する要求の token を盗み、相手の scope で op を呼ぶ経路）を塞ぐ。
  1. **排他**: capability を 1 つ以上持つ要求は、1 つの isolate の中で同時に 1 件までしか走らせない。actor は、その要求の走っている間は capability を持つ別の要求を始めず、待ち行列に置く。capability の無い要求（純粋判定）は並行してよい。
     盗む相手の token（別の scope）が同時に存在しないので、token を盗んでも scope は広がらない。digest pass は 1 tick に高々 1 回なので、排他の待ちは実運用では生じない。
  2. **domain への結び付け**: 上の grant の規則。純粋判定の領域のコードが `cos_digest` の token を global から読んでも、許可表が空の領域では op が通らない。token と grant の両方が `cos_digest` のものでなければ op は通らない。
  3. **失効**: 要求が終わる（返事・deadline・cancel・generation の作り直し）と、`GrantGuard` が token を消す。終わった要求の token は、後の要求からは使えない（S4-18）。
  4. **隠れた入力**: fixture の runner と pass の replay runner は、全件を正順と逆順に 2 回流し、同じ出力を要求する（S2-7 と同じ）。
  - 要求ごとに context を分けない理由: vendor の `deno_core` に、追加の context（realm）を作る公開 API が無い。`crates/script-runtime/vendor/deno_core` を `grep` したところ、`create_realm` が無かった。`v8::Context::new` は runtime の初期化の中だけにある。
    要求ごとに isolate を作り直す方式は、pass ごとに bundle の評価が要り、200 ms の deadline に収まるかが未確認である。この方式は S5 以降で、op を使う領域が 2 つ以上になったときに見直す。

### S4-14 mode（目的の (3)。S4-3・S4-4 を改める）
mode は pass の始めに `script_domain_modes` から 1 回読み、その pass の間は固定する。actor も、要求を受けた時点の generation を要求に固定する（(d)）。
| mode | 読み出し | apply する command | TS の呼び出し |
|---|---|---|---|
| `rust` | Rust が memo を通して読む（今と同じ回数） | Rust | しない |
| `compare` | Rust が memo を通して読み、Rust の command を apply する。memo は apply の前に凍結する | Rust | apply の後、別の tokio task で 1 pass に 1 回送る。op は凍結した memo からだけ答え、store を読まない。memo に無い要求（例: list に無い run の read）は `denied`（`denied_scope`）を返す。二重の読み出しも、apply との競合も起きない |
| `ts` | TS の op が呼んだときに memo を通して store を読む。TS が返った後、Rust の参照実装が同じ memo で判定する。TS が読んでいない run だけを Rust が読み足す | 検証に通れば TS、通らなければ Rust（下の fallback） | dispatcher が 200 ms を上限に待つ |
- `compare` の in-flight は、1 pass で `MAX_IN_FLIGHT_COMPARES`（全領域の合計 32）の 1 枠を使う。S4-4 の「20 run が 20 枠を使い、上限を超えた run の記録を落とす」は無くなる。
- **`ts` の検証**: TS の出力を、run ごとに S4-3 の規則 1〜6 で検証する（入力は memo のその run の `CosDigestInput`）。それに加えて、pass に次の規則を当てる。
  7. 出力のどの `run_id` も、その pass の `list_runs` の答えにある（code `run_outside_list`。安全違反に数える）。
  8. `read_run` が成功した run は、出力にちょうど 1 回ずつ現れる。`read_run` が失敗した run は出力に現れない（code なし。理由文 `run_missing`・`run_unreadable`。形式の違反）。
     Rust の参照実装は、読み出しに失敗した run を digest しない（今の `digest_one` の `Err`）。規則 8 はこれと同じにするためのものである。
  9. 出力の `runs` の並びは `list_runs` の答えの並び（`started_at`・`run_id` 順）と同じ（code なし。理由文 `run_order`。形式の違反）。apply の順序を Rust と揃える。
- **fallback**: 次のどれかが起きたら、その pass の TS の出力は 1 件も apply しない。pass 全体を Rust の command で apply する。
  - TS の失敗（`ScriptError` の全種類。`timeout` を含む）
  - 規則 1〜9 のどれかに落ちる
  - runtime・generation が使えない
  - apply の直前に読み直した `script_domain_modes` の行が、pass の始めと違う（mode・generation・`version` のどれか。code `mode_changed`）
  検証は apply の前に pass 全体に当てる。一部の run だけ TS の command を apply することはしない。
- **apply**: run ごとに、`post_digest`→`requeue_run` の順に今の `apply_digest_commands` で書く。ある run の書き込みが失敗したら、その run の残りは書かずに次の run へ進む。`digest_due` は空の走査まで下ろさない（S4-3 の既存の規則を動かさない）。
  apply の失敗は TS の責任ではないので fallback にしない。その run は次の pass で再び list に現れ、冪等 key が 2 件目の message を作らない。
- **`ts` を受ける範囲と権限**（S2-5・(d) の権限の具体化）:
  - API が `ts` を受けるのは domain `cos_digest` だけ。`lane_rules`・`route_priority` の `ts`、どの domain の `canary` も、今までどおり 422（`script_mode_not_enabled`）で拒否し、表を変えない。
  - `→ts` は、人の API token の actor だけが行える。CoS operations・外部 operator の token・daemon（`system:*`）の要求は 403 にする（code `script_mode_requires_human`）。CoS operations の registry では、`to_mode = ts` を EXCLUDED の理由付きで拒否する。
  - `→ts` の元は、同じ generation の `compare` に限る。`rust` から直接 `ts` にする要求と、generation を同時に変える要求は 422（`script_mode_ts_requires_compare`）にする。
  - `ts→rust`・`ts→compare`（戻し）は、S2-5 の戻しと同じく、認証された誰でも・daemon 自身が行える。起動時の復元（bundle が無い・hash が違う・`host_api` が合わない・manifest の `cos_digest` に capability が足りない）では、actor `system:script-startup` が `rust` へ戻す。
- **本番**: 本番の `cos_digest` は `rust` のまま変えない。S4 は `ts` を試験と非本番の daemon でだけ使う。本番で `compare`・`ts` にする手順と、canary・停止基準・他の領域への拡大は S5 で決める。進行中の測定 daemon では切り替えない（(g)）。

### S4-15 記録（目的の (4)。S4-5・S4-7 を改める）
- `script_compare_log` は今までどおり 1 run 1 行で書く。`mode` が `compare`・`ts` の pass で、出力（Rust・TS のどちらか）に現れた run ごとに 1 行。mode `rust` では書かない。表の列と migration は変えず、`json` に次の欄を足す。
  | 欄 | 型 | 中身 |
  |---|---|---|
  | `mode` | `"compare" \| "ts"` | pass の始めに読んだ mode |
  | `pass_id` | string（ULID） | 同じ pass の行どうしで同じ値 |
  | `applied_by` | `"rust" \| "ts"` | その run に apply した command を出した側。`compare` では常に `rust` |
  | `fallback` | `{ code, message } \| null` | `ts` で Rust の command を apply した理由。code は `ts_error`・`invalid_output`・`run_outside_list`・`run_missing`・`run_unreadable`・`run_order`・`mode_changed`・`unavailable`。`message` は 512 byte で切る。`compare` と、TS を apply した行では `null` |
  | `ops` | `{ op, request_digest, reply_digest, outcome, elapsed_us }[]` | その run に関わる op 呼び出し（その pass の `list_runs` と、その run の `read_run`）。`request_digest`・`reply_digest` は `sha256:` + 直列化の sha256。`outcome` は `ok`・`error`・`denied`・`memo`（memo から答えた 2 回目以降）。本文は残さない（S4-5 の理由のまま） |
- `call_site` は `cos_digest.pass` に変える。`outcome`（`match` ほか）と `rust`・`ts` の `commands_digest`・`command_kinds` は run ごとの比較のまま残す。`ts` mode でも、Rust の参照実装の command と比べた結果を入れる。
- 重複の抑え（S4-5）の比べる欄に `applied_by` を足す。同じ run・同じ `generation`・`input_digest`・`outcome`・`applied_by` の行は足さない。
- message の冪等 key `cos-triage-digest:<run_id>` は変えない。TS が出した `post_digest` も、規則 1 で同じ key に限る。
  どちらの側が apply しても、再配送・retry・再起動・fallback の後に 2 件目の message は作られない。requeue の `WHERE state='running'` も変えない。
- 新しい `Event` の variant は足さない（S4-7 のまま）。task-api の query・`web/api` の生成型は変わらない。

### S4-16 trace の形と replay（目的の (5)。S4-5 の replay・S4-8 を改める）
- 置き場は `control-ts/fixtures/cos_digest/pass_traces.jsonl`。1 行が 1 pass で、形は次のとおり。
  `{ "name": string, "ops": { "op": "cos_digest:list_runs" | "cos_digest:read_run", "request": object, "reply": { "kind": "ok", "value": object } | { "kind": "error", "error": HostError } }[], "output": CosDigestPassOutput }`
  - `ops` は Rust の pass の memo の中身で、最初に読んだ順に並べる。
  - `output` は Rust の参照実装の出力。TS の手書きにしない（S4-2 と同じ）。
- 書き出し: 本物の dispatcher の経路から書き出す。triage run は FakeAdapter で終えさせる。`CELERIS_UPDATE_FIXTURES=1` で書き直し、無ければ byte 一致を確かめる（`cos_digest_trace.rs` と同じ方式）。
  場面は、1 run・20 run を超える drain・stopped / interrupted / completed / failed の混在・`read_run` の失敗・`chat_run_get` の失敗（`reason: null`）を含める。
- replay: fake host（`control-ts/src/testing/` の pass replay runner）が、TS の op 呼び出しに trace の `ops` から答える。照合は `op` と `request` の直列化の一致で行う。
  trace に無い要求には `denied` を返し、その pass を不一致にする。合格は次の 2 つがそろうことである。
  - TS の `output` が trace の `output` と byte 一致する。
  - TS が呼んだ op の集合が trace の `ops` に含まれる。
  全 pass を正順と逆順に 2 回流す（S4-13 の 4）。`node control-ts/scripts/compare.mjs cos_digest` は pass の replay を行い、合否を exit code で返す。
- `cases.jsonl` は純粋関数 `decide()` の単体の fixture として残す。compare.mjs は各行を「run 1 つの pass」に機械的に組み直して replay する（list の答え = その行の `thread`・`run_id`・`run_state`、read の答え = `input`、`output` = `commands`）。手書きの期待値は足さない。
- LLM は呼ばない。digest の pass は終わった triage run の結果を読むだけである。LLM の応答（CoS の判断）は `cos_inbox_items` の `decision` として記録済みで、trace の `read_run` の応答に入っている。live で LLM を二重に呼ばない。
- 本番の log の replay の道具は作らない（S4-10 の (7) のまま）。log は本文を持たないので、本番の log からは trace を作れない。

### S4-17 旧い経路を消す（目的の (6)。破壊的変更、ADR 2026-10-11-breaking-changes-by-default）
pass-wire・ts-pass・traces-async の各 unit が、新しい経路を入れるのと同じ差分で次を消す。互換の分岐・新旧の切替 flag は残さない。
- Rust: `PolicyScript::cos_digest`（run ごと）とその実装（`crates/celeris/src/daemon/script_policy.rs`）、`Dispatcher::compare_cos_digest` の run ごとの spawn、`CosDigestDraft` の run ごとの呼び出し、`CosDigestApplication` を run ごとに compare へ渡す経路。
- TS: `cosDigest`（run ごとの入口）の export と、`domains/index.ts` の登録。`compare.mjs` の `cos_digest` の `entry: "cosDigest"`。
- fixture と試験: `control-ts/fixtures/cos_digest/traces.jsonl`、`crates/task-dispatch/src/dispatcher/tests/cos_digest_trace.rs`。run ごとの compare を前提にした試験は、pass の試験に書き換えるか消す（`cos_chat_triage_digest_script_compare.rs`、`crates/celeris/tests/script_compare_cos_digest.rs`）。
- 文書: S4-6 は S4-13 で置き換えた。architecture-map の cos_digest の行は close-async unit が直す。

### S4-18 await 中の op が返す error（目的の (7)）
| 起きたこと | await 中・後の op が返す error（`HostError.code`） | 呼び出しの結果 | mode `ts` の pass |
|---|---|---|---|
| pass の deadline（200 ms）を過ぎた | `timeout`。op server は deadline の時点で受け付けを止め、その後は store を読まない | `ScriptError::Timeout`。同期のループは watchdog が止める（(b)） | Rust で apply（`fallback.code = ts_error`） |
| 呼び出し側の cancel（dispatcher の停止・tick の future の破棄） | `terminated`。開始時に有効だった token の grant が await の後に消えていたら、この code を返す。開始時から知らない token の場合だけ `denied` を返す | 結果は捨てる | 何も apply しない。apply は TS が返った後の 1 か所だけなので、cancel で副作用は起きない。次の tick（再起動後は reconcile で立つ `digest_due`）で pass をやり直す |
| generation の作り直し（isolate の再生成・heap 上限・terminate 後の作り直し） | `generation_gone` | `ScriptError::GenerationGone` | Rust で apply（`ts_error`） |
| pass 中の mode・generation の切り替え（API の CAS） | op は続けて答える。pass は始めの generation で最後まで走る（Promise を新 generation へ移さない。(d)） | 通常どおり | apply の直前の読み直しで違いを見つけたら Rust で apply（`mode_changed`） |
| op server の store の読み出しの失敗 | `exception`（op の error。memo に error として残す） | TS が扱う（その run を飛ばす） | 規則 8 で照合する（Rust も同じ run を飛ばす） |
- `terminated`・`timeout`・`generation_gone` は安全違反に数えない（S2-8 の timeout・失敗の数え方のとおり）。`denied` だけを `denied_capability`・`denied_scope` として数える。

### S4-19 後続の unit の作業範囲と試験名
試験名は unit が実装時に変えてよい。変えたら進捗ファイルに対応を書き、close-async unit がこの表と突き合わせる。
| unit | 作業範囲（path） | 足す・直す試験 |
|---|---|---|
| host-ops | `crates/script-runtime/src/{ops.rs,dto.rs,actor.rs,handle.rs,host.js}`、`crates/script-runtime/tests/`、`docs/api/script-host/host-v1.schema.json`、`control-ts/src/host.d.ts`（生成物）、`crates/celeris/src/daemon/script_policy.rs`（`ScriptCall` の欄が増えた分の追従だけ） | `crates/script-runtime/tests/cos_digest_ops.rs`: `cos_digest_ops_denied_without_manifest_declaration`、`cos_digest_ops_forward_to_host_sink_and_return_reply`、`cos_digest_ops_grant_bound_to_domain`、`cos_digest_ops_terminated_after_request_end`、`cos_digest_ops_timeout_at_deadline`、`cos_digest_ops_generation_gone_on_rebuild`、`capability_requests_are_exclusive_per_isolate`、`cos_digest_ops_busy_after_64_calls`。schema と生成型の再生成で差分 0 |
| mode-ts | `crates/task-api/src/script_domains.rs`、`crates/task-api/tests/script_domains.rs`、`crates/task-api/src/cos/operations.rs`・`cos/ops/`、`agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`、`config/skills/cos-operator/operations.md`、`crates/task-core/src/script_compare.rs`（mode の検証だけ） | `script_domain_mode_ts_only_for_cos_digest`、`script_domain_mode_ts_requires_human_actor`（CoS・外部 operator・`system:*` を拒否）、`script_domain_mode_ts_requires_compare_same_generation`、`script_domain_mode_ts_rollback_by_anyone`、`script_domain_mode_canary_still_rejected`、CoS registry の既存試験の追従。dispatcher は pass-wire が入るまで `ts` を `rust` と同じに扱う（`compare_target` は `compare` 以外で何もしない） |
| pass-wire | `crates/task-dispatch/src/dispatcher/cos_chat/{digest.rs,triage.rs}`、`crates/task-dispatch/src/dispatcher/script_compare.rs`、`crates/task-dispatch/src/script_policy.rs`、`crates/task-core/src/chat/digest.rs`（pass の DTO・規則 7〜9）、`crates/task-core/src/script_compare.rs`・`script_compare/cos_digest_record.rs`（S4-15 の欄）、`crates/celeris/src/daemon/script_policy.rs`（`cos_digest_pass` の実装。trait を変える unit が同じ差分で直す）、S4-17 の Rust の削除 | `crates/task-dispatch/src/dispatcher/tests/cos_digest_pass.rs`: `cos_digest_pass_rust_reads_each_once_and_applies_rust`、`cos_digest_pass_compare_answers_ops_from_frozen_snapshot`、`cos_digest_pass_compare_denies_unlisted_read`、`cos_digest_pass_ts_applies_validated_ts_commands`、`cos_digest_pass_ts_falls_back_on_error_timeout_invalid`、`cos_digest_pass_ts_rejects_run_outside_list`、`cos_digest_pass_ts_mode_changed_before_apply_uses_rust`、`cos_digest_pass_log_records_applied_by_and_ops`。task-core: `cos_digest_pass_validation_rejects_each_rule`（規則 1〜9）。既存 11 件（`cos_digest_script.rs`）を通す |
| ts-pass | `control-ts/src/domains/{cos_digest.ts,cos_digest.test.ts,index.ts}`、`control-ts/src/testing/`（pass replay runner）、`control-ts/scripts/{compare.mjs,compare.test.mjs}`、`control-ts/bundle.config.json`（`cos_digest` の capability）、S4-17 の TS の削除 | `cos_digest.test.ts`: `cosDigestPass reads list then each listed run`、`skips a run whose read_run throws`、`never reads a run outside the list`、`replay forward and reverse give the same output`。`node control-ts/scripts/compare.mjs cos_digest` が `cases.jsonl` を組み直した pass で全件一致し exit 0。TS の規則を 1 か所壊した写しで exit 1 |
| traces-async | `crates/task-dispatch/src/dispatcher/tests/cos_digest_pass_trace.rs`（新）、`control-ts/fixtures/cos_digest/pass_traces.jsonl`（新）、`traces.jsonl` と `cos_digest_trace.rs` の削除 | `cos_digest_pass_trace_fixture_is_current`（`CELERIS_UPDATE_FIXTURES` 無しで byte 一致）。`node control-ts/scripts/compare.mjs cos_digest` が `pass_traces.jsonl` を含めて全件一致 |
| real-host | `crates/celeris/tests/script_cos_digest_pass.rs`（`script_compare_cos_digest.rs` を置き換え） | `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` で: `script_cos_digest_pass_compare_real_host_match`、`script_cos_digest_pass_ts_applies_ts_commands`、`script_cos_digest_pass_ts_throwing_bundle_falls_back_to_rust`、`script_cos_digest_pass_ts_daemon_restart_without_bundle_restores_rust`、`script_cos_digest_pass_ts_restart_keeps_one_message`。mode `ts` で apply した `chat_messages`・`cos_inbox_items` が、mode `rust` の基準と byte 一致する |
| robust-ts | `crates/task-dispatch/src/dispatcher/tests/cos_digest_script.rs`（mode `ts` の場面を足す） | `cos_digest_script_ts_order_item_update_before_and_after_run_end`、`…_ts_redelivery_same_run_twice`、`…_ts_retry_message_write_failure`、`…_ts_retry_scan_failure`、`…_ts_retry_requeue_write_failure`、`…_ts_cancel_deadline_during_op_await`、`…_ts_cancel_dispatcher_stop_during_op_await`、`…_ts_restart_mid_pass_applies_once`、`…_ts_policy_version_bump_session_fresh`、`…_ts_generation_gone_during_await_falls_back`。共通の不変条件は 3 つ: message は冪等 key ごとに 1 件、requeue は item ごとに 1 回まで、記録は run・`applied_by` ごとに 1 件まで |
| close-async | 進捗のまとめ、`docs/architecture-map.md` の cos_digest の行、この付記の実装との突き合わせ（S4-20 として足す） | 文書の検査（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`） |

### S4-20 実装との突き合わせ（S4 close-async、2026-11-03）
記録は [S4 の進捗](../progress/2026-11-02-ts-control-layer-s4.md) と各 unit の記録。S4-11〜S4-19 を実装と突き合わせた結果である。安全・apply の規則に関わる違いは無い。違いは置き場所・名前・記録の細部・試験の範囲である。S4-6 の本文は S4-13 で置き換えたまま有効でない。S4-10 の表のうち S4-4・S4-6・S4-8 の行は、この表の S4-14・S4-13・S4-16 の行が改める。

| 節 | 実装 | 付記との違い |
|---|---|---|
| S4-12 | TS の入口は `control-ts/src/domains/cos_digest.ts` の `cosDigestPass(ctx)`。op は `crates/script-runtime/src/ops.rs` の `op_cos_digest_list_runs`・`op_cos_digest_read_run`（Host API 1.1.0、`HOST_API_VERSION`）。dispatcher の pass は `crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` の `Dispatcher::cos_digest_pass` と memo `CosDigestPassReads`。trait は `PolicyScript::cos_digest_pass(CosDigestPassCall, deadline)`、deadline は `COS_DIGEST_PASS_DEADLINE`（200 ms）。1 要求の op は `MAX_OPS_PER_REQUEST`（64）まで | (1) host sink は `ScriptCall` の欄でなく `ScriptRuntimeHandle::call_with_host_sink(call, Some(sink))` の引数。`HostSink = mpsc::Sender<HostOpCall>`。(2) `celeris-script` は task-core に依存しないので、op の DTO の写しを `crates/script-runtime/src/dto.rs` に持ち、schema はここから出す。task-core の型との一致は `crates/script-runtime/tests/dto.rs` の `cos_digest_input_matches_task_core` が `cases.jsonl` の全行の往復で確かめる。pass の DTO（`CosDigestPassOutput` ほか）の正本は `crates/task-core/src/chat/digest.rs`。(3) schema の `Option` の欄は `string \| null` の required になった（`require_all_properties`）。(4) mode `ts` の待ちは、tick が同期の関数なので、TS の呼び出しと op server を別 thread の current-thread runtime で 1 つの future として走らせる。store は dispatcher 側に残る。(5) daemon（`crates/celeris/src/daemon/script_policy.rs`）は pass 1 回を入力 `{}` の要求 1 件として送る |
| S4-13 | 許可表 `DOMAIN_CAPABILITIES` の `cos_digest` に 2 つ。`control-ts/bundle.config.json` の `cos_digest` も 2 つを宣言する。排他は actor の `held`、失効は `GrantGuard` | (6) capability の無い要求（純粋判定）と並行して走る `cos_digest` の要求の token を global から読めば、その要求の scope で op を呼べる。scope は同じ pass の分から広がらない（S4-13 の 2 のとおり、他の領域の許可表が空なので op は通らない） |
| S4-14 | 規則 7〜9 は `validate_cos_digest_pass`・`CosDigestPassViolation`（task-core）。mode の権限は `crates/task-core/src/store/script_domains.rs` の `script_domain_set_mode` と `crates/task-api/src/script_domains.rs`。起動時の復元は `crates/celeris/src/daemon/script_layer.rs` の `restore_modes` が `needs_generation()` の行（`compare`・`canary`・`ts`）を `rust` へ戻す | (7) CoS operations の `script_domain.mode_set` は registry の分類を ALLOWED のまま残し、本文の `mode` が `ts` なら effect の前に 403（理由 `human_only`）で拒み、`cos_operations` に rejected を記録する。CoS からの戻し（`ts→rust`・`ts→compare`）は通る（`script_domain_mode_ts_rollback_by_anyone`）。(8) 起動時の復元の条件のうち「manifest の `cos_digest` に capability が足りない」は扱っていない。`restore_modes` は bundle の hash で判定する。hash が同じなら manifest も同じなので、配備済みの bundle では起きない想定である（未確認）。(9) 規則 1〜6 の違反はその run の行を `invalid_output` にする。規則 7〜9 の違反のうち、run を名指さないもの・Rust の出力に無い run を名指すものは、全行を `invalid_output` にする |
| S4-15 | `CosDigestCompareRecord`（`crates/task-core/src/script_compare/cos_digest_record.rs`）に `mode`・`pass_id`・`applied_by`・`fallback`・`ops`。`COS_DIGEST_CALL_SITE` は `cos_digest.pass`。重複の抑えに `applied_by` を足した | (10) mode `compare` の `ops` の `outcome` は全部 `memo`（凍結した memo から答える。拒否は `denied`）。(11) `denied` の op が 1 回でもあった pass では、全行の `safety_violations` に `denied_scope` を入れる。list に無い run の read は、どの行の `ops` にも入らない。(12) 記録は apply が全部通った run だけに書く（S4-10 の (5) のまま）。(13) 新しい binary は欄の無い旧い形の行を読めない。本番は mode `rust` で行を書いていない |
| S4-16 | `control-ts/fixtures/cos_digest/pass_traces.jsonl`（20 行）。書き出しと byte 一致は `crates/task-dispatch/src/dispatcher/tests/cos_digest_trace.rs`（module `cos_chat_triage::pass_trace`）。replay は `control-ts/src/testing/pass-replay.ts` の fake host と `control-ts/scripts/compare.mjs` | (14) 1 行に `writes`（実際に書かれた message と requeue）の欄を足した。形は `{ name, ops, writes, output }`。`compare.mjs` は行末の `output` だけを比べるので replay は変わらない。`writes` は Rust の試験が `output` の commands と照合する。(15) 同じ場面を mode `rust` と mode `ts` で走らせ、`script_compare_log` の行が `applied_by = ts`・`match` であることも確かめる。(16) `read_run` が失敗する pass は fixture に入れず、`cos_digest_pass_trace_read_failure_skips_the_run_in_both_modes` で確かめる。(17) `compare.mjs` の冒頭の注釈は `traces.jsonl` の名前を残している（動作には関わらない） |
| S4-17 | 消えたもの: `PolicyScript::cos_digest`、`Dispatcher::compare_cos_digest`、`CosDigestDraft`・`CosDigestApplication` の run ごとの経路、TS の `cosDigest` の export、`compare.mjs` の `entry: "cosDigest"`、`control-ts/fixtures/cos_digest/traces.jsonl` | (18) `cos_digest_trace.rs` は消さず、同じ path で pass の trace の試験に書き換えた。(19) `crates/celeris/tests/script_compare_cos_digest.rs` は `script_cos_digest_pass.rs` に置き換えず、同じ file を pass の形に直して mode `ts` の 4 件を足した（9 件）。`cases.jsonl` だけを読む |
| S4-18 | `crates/script-runtime/tests/cos_digest_ops.rs` と `crates/task-dispatch/src/dispatcher/tests/cos_digest_script_ts.rs` のとおり | (20) deadline と要求の終わりが同時に起きたら `timeout` を優先して返す |
| S4-19 | 下の対応のとおり | (21) 試験名の違い: real-host の 5 件は `script_cos_digest_pass_*` でなく `script_compare_cos_digest_*`（`…_ts_mode_applies_ts_commands`・`…_compare_answers_ops_from_rust_snapshot`・`…_ts_mode_falls_back_to_rust`・`…_daemon_ts_restart_keeps_one_message`。throw の fallback と restart の rust への戻しはこの 2 件に含む）。traces-async は `cos_digest_pass_trace.rs` でなく `cos_digest_trace.rs`。robust-ts は `…_ts_retry_scan_failure` を tick 全体の失敗と `…_ts_retry_list_runs_failure_during_op` に分け、`…_ts_retry_read_run_failure_during_op`・`…_ts_item_updated_during_op_await`・`…_ts_restart_after_failed_requeue`・`…_ts_mode_and_generation_change_during_op_await` を足した（15 件）。ts-pass は `skips a run whose read_run throws` を `skips a run whose read_run throws exception` にした |

## 付記 2026-11-03: S5 の generation 管理・段階切替・rollback と S6 の測定の定義（S5 adr-s5、task 01M6FPPJN7A8H427MXFVYFY7H4）
記録は [S5 の進捗](../progress/2026-11-02-ts-control-layer-s5.md) と [adr-s5](../progress/2026-11-02-ts-control-layer-s5/adr-s5.md)。
(d)・(f)・(g)、付記 S2-4・S2-5・S4-14・S4-16 と S4 の進捗の未解決 7・8 を具体化する。**(f) の数値は 1 つも変えない。** 下の節が S2-5・S4-14 の mode の権限と S2-4 の表の定義を改めるときは、そう書く。
言語移行と方針改善を混ぜない。S5 は TS の規則・Rust の参照実装の判定を変えない。

### S5-1 実行中の daemon に新 bundle を足す経路と generation の状態（目的の (1)）
**登録の経路**
1. bundle の配備は今の `script-deploy`（`crates/script-runtime/src/bin/script-deploy.rs`）が行う。`<state_dir>/script-bundles/<bundle_sha256>/` に `bundle.mjs` と `manifest.json` を置く。API は bundle の本文を受け取らない（upload の経路を作らない）。
2. `POST /api/v1/script/generations`（本文 `{ "bundle_sha256": "<64 桁 hex>" }`）で登録を頼む。task-api は trait `ScriptGenerationControl`（`crates/task-api/src/script_generations.rs`。DTO だけを使い `deno_core` に依存しない。(b) の依存の制約）を呼ぶ。daemon（`crates/celeris/src/daemon/script_layer.rs`）がこれを実装し、`celeris-script` の `GenerationRegistry`（`crates/script-runtime/src/generation.rs`）に渡す。
3. registry は**新しい isolate（新しい actor の thread）**で bundle を load し、次を検証する: 配備先の名前と hash の照合（`manifest.rs` の今の検証）、`host_api` の major、`schema_sha256`、`domains` が既知の領域だけ、`capabilities` ⊆ 許可表 `DOMAIN_CAPABILITIES`、`checkpoint_versions` の形（S5-5）、module の評価と selftest、manifest の各領域の入口が export されていること。既存の isolate は使わない。
4. 通れば `script_generations` に `loaded` の行を書き、同じ transaction で `script_generation_loaded` を追記する。落ちれば `invalid`（理由付き）を書き、isolate を捨て、422 `script_generation_invalid` を返す。
5. trait が未接続（`[script]` の無い daemon、task-api の試験の state）なら 503 `script_runtime_unavailable` を返し、表を変えない。
6. 同じ hash がすでに `loaded`・`active`・`draining` なら、何も変えず 200 で今の行を返す。`retired` の hash は新しい isolate で load し直して `loaded` に戻す（known-good への戻り道。S5-3）。`invalid` の hash は 422 `script_generation_invalidated` で拒む（直した bundle は別の hash になる）。
- 一覧は `GET /api/v1/script/generations`、退役は `POST /api/v1/script/generations/{id}/retire`（`active` なら 409 `script_generation_active`）。
- 登録と退役は挙動を変えないので、人の token・外部 operator の token・CoS operations（registry に ALLOWED で `script_generation.register`・`script_generation.retire` を足す）で行える。

**generation の状態**（S2-4 の `state IN ('loaded','rejected')` を改める。migration は store-gen unit が足し、既存の `rejected` の行は `invalid` に移す。`rejected` の語は残さない）
| 状態 | 意味 | isolate | 入る条件 |
|---|---|---|---|
| `loaded` | 検証に通り、どの領域の mode の行からも指されていない | 持つ | 登録の成功、`retired` の再登録 |
| `active` | 1 つ以上の領域の `script_domain_modes.generation_id` がこれを指す | 持つ | mode の CAS（同じ transaction で状態も変える） |
| `draining` | どの領域からも指されなくなったが、in-flight が残る | 持つ（新しい要求は受けない） | 最後の領域が別の generation か `rust` に移った CAS |
| `retired` | isolate を捨てた。行と bundle の file は残す | 持たない | drain の終わり（S5-2）、`loaded` の退役 |
| `invalid` | 検証に落ちた、または安全違反（S5-7）で無効にした | 持たない | 登録の検証失敗、`script-guard` の安全違反 |
- 遷移は store の 1 か所（`script_generation_transition`）だけが書き、毎回 `script_generation_{loaded,activated,draining,retired,invalidated}` を追記する。generation は特定の task に属さないので、event は新しい追記専用の表 `script_generation_events`（`id` INTEGER PK、`ts`、`generation_id`、`json`）に書く。UPDATE・DELETE の経路は作らない（S2-4 の `script_domain_events` と同じ扱い）。
- **生きた generation（`loaded`・`active`・`draining`）は最大 2 つ**（(f) の RSS の上限が generation 2 個で 200 MiB）。3 つ目の登録は 409 `script_generation_limit` で拒む。daemon は自動で `loaded` を退役させない（人か CoS が退役させる）。
- Celeris は `script-bundles/` の下の file を消さない（`retired`・`invalid` の file も残す）。消すのは人の手順だけ。
- `[script] active_bundle_sha256` は消す（読み込みエラー）。起動時に load する generation は DB の `script_domain_modes` が決める（S5-2 の起動時の復元）。`[script]` に残すのは `state_dir` と上限だけ。本番の config に欄があれば人が消す（進捗の「本番への移行」）。

### S5-2 要求ごとの generation の固定・in-flight・drain（目的の (3)）
- **固定**: 呼び出し側（dispatcher の tick・digest の pass）は mode の行を 1 回読み、その行の `generation_id` を要求に入れる（S4-14 のまま）。`GenerationRegistry::call(generation_id, …)` はその generation の actor にだけ送る。registry に無い・`retired` の id への要求は即 `GenerationGone` を返す。呼び出し側は Rust の参照実装で答える。
- **in-flight**: generation ごとに 64（今の bounded mpsc の容量。超えたら `Busy`）。生きた generation は 2 つまでなので process 全体で 128 まで。compare の別 tokio task は今の `MAX_IN_FLIGHT_COMPARES`（全領域で 32）のまま。registry は generation ごとに in-flight の数を RAII の guard で数える。
- **切替**: mode の CAS が `generation_id` を A から B に変えたら、同じ transaction で B を `active` にし、A を指す行が残らなければ A を `draining` にする。CAS の後に行を読んだ要求は B へ行く。切替の前に A を読んだ要求は A で最後まで走る。
- **drain**: `draining` の generation は新しい要求を受けない。in-flight が 0 になったら `retired` にして isolate を捨てる。純粋判定は CAS の時刻から **30 秒**で打ち切る（(d)）。30 秒の時点で残る要求は watchdog が `terminate_execution` で止め、呼び出し側には `GenerationGone` を返す。呼び出し側は Rust の参照実装で答える。打ち切った件数を `script_generation_retired` の `cancelled_in_flight` に書く。
  - 要求ごとの deadline（純粋判定 50 ms、digest の pass 200 ms）が 30 秒より短いので、ふつうは deadline のうちに drain が終わる。30 秒は退役の上限である。
- **CoS の digest pass**: pass は始めの generation で最後まで走る（S4-18）。pass の途中で A が `draining` になっても、op は A の要求の token で答え続ける。apply の直前の読み直しで違いを見つけたら Rust で apply する（`mode_changed`）。Promise・closure を B へ移さない。
- **起動時の復元**（(d)・S4-14 を具体化する）: daemon は起動時に `script_domain_modes` から `generation_id` を集め（最大 2 つ）、それぞれを新しい isolate で S5-1 の 3 と同じ検証で load する。file が無い・hash が違う・`host_api` が合わない・検証に落ちる・manifest にその領域が無い・その領域の capability が足りない、のどれかなら、その generation を指す領域を actor `system:script-startup` で `rust` に戻し、受信箱に項目を出す。load できた generation の mode（`compare`・`canary`・`ts`）は保つ（S4 の `restore_modes` が `needs_generation()` の行を全部 `rust` へ戻すのを改める）。`invalid` を指す行は必ず `rust` に戻す。script-guard の停止（S5-7 の `halted`）が立っていれば、全領域を `rust` で起動する。

### S5-3 canary の選び方（目的の (2) の前半）
- `script_domain_modes` に `canary_bps` INTEGER を足す（store-gen unit）。CHECK: `mode = 'canary'` なら 1〜9999、それ以外は NULL。
- 選択は決定的な hash で行う: `h = sha256("celeris.canary/1" || 0x00 || domain || 0x00 || generation_id || 0x00 || unit_key)` の先頭 8 byte を big-endian の u64 にし、`h % 10000 < canary_bps` なら TS の側。generation を key に入れるので、generation ごとに対象の集合が変わり、同じ generation の中では変わらない。乱数・時計を使わない。
- 単位（`unit_key`）:
  | 領域 | 単位 | 理由 |
  |---|---|---|
  | `lane_rules` | task id | 同じ task の判定が run ごとに TS と Rust を行き来しないようにする |
  | `route_priority` | task id | 同じ。`route_select` の run id は判定の時点で無いことがある（S2-4 の `run_id` null） |
  | `cos_digest` | pass（`pass_id`） | thread 単位にしない。1 pass は複数の thread の run を含み（S4 の trace の「2 thread」）、規則 7〜9 の検証と apply は pass 全体で行う（S4-14）。thread で分けると 1 pass の中で TS と Rust の command が混ざる。`pass_id` は Rust が作る ULID で、記録に残るので後から選択を再現できる |
- canary の Rust 側の単位も compare と同じく TS を影で走らせて記録する。TS 側の単位は TS の結果を Rust が検証してから使い、Rust の参照実装の結果と比べて記録する（(d) の表）。記録の json に `canary_selected`（bool）と `canary_bps` を足す（`script_compare_recorded` と `script_compare_log` の両方）。
- 戻し先は常に `rust`（または `compare`）にする。以前の TS の generation へ自動で戻さない。known-good の bundle は file と `retired` の行で残り、使うときは再登録して `compare` から昇格し直す。Rust の参照実装は S6 の終了条件まで残す（(d)）。

### S5-4 mode の遷移と権限（目的の (2) の後半。S2-5・S4-14 を改める）
| 遷移 | 誰が | 前提（満たさなければ 422） |
|---|---|---|
| `rust`→`compare`、または任意の mode から別の generation の `compare` へ | 人・CoS operations・外部 operator の token（挙動を変えない） | generation が `loaded`・`active`、manifest にその領域がある、checkpoint の版（S5-5）が合う |
| `compare`→`canary` | **人の token だけ**（CoS・外部 operator・`system:*` は 403 `script_mode_requires_human`。CoS registry は本文の `mode` が `canary`・`ts` なら effect の前に 403 で拒み、rejected を記録する。S4-20 の (7) と同じ形） | 同じ generation の `compare`。script-guard の計数（S5-7）が (f) の「`compare`→`canary` へ進む条件」を満たす（不足は 422 `script_canary_not_ready`、本文に計数）。`canary_bps` が 1〜9999 |
| `canary` の `canary_bps` の変更 | 人の token だけ | 同じ generation |
| `canary`→`ts` | **人の token だけ** | 同じ generation の `canary`。canary の間の計数が (f) の「進む条件」と同じ閾値を満たす（TS 側の単位で数える） |
| `canary`・`ts`・`compare`→`rust`、`canary`・`ts`→`compare`（戻し） | 認証された誰でも・daemon（`system:script-startup`・`system:script-guard`）。自動でもよい | なし |
- S4 で許した `compare`→`ts`（`cos_digest` だけ）は消し、全領域で `canary` を経る。`canary`・`ts` を全 3 領域（`lane_rules`・`route_priority`・`cos_digest`）で受ける。`script_mode_not_enabled`（422）は使わなくなるので消す。`script_mode_ts_requires_compare` は `script_mode_requires_previous_stage` に改める（`canary` は同じ generation の `compare` から、`ts` は同じ generation の `canary` から）。
- script-guard の停止（S5-7 の `halted`）が立っている間は、`rust` 以外への遷移を 409 `script_guard_halted` で拒む。停止を解くのは人の token だけ（`POST /api/v1/script/guard/clear`）。
- 認可の error（401・403）では表も event も変えない。CAS の版違いは今の 409 のまま。
- 本番の mode を変えるのは人の手順だけ。worker と CoS は本番で `canary`・`ts` にしない（進捗の「本番への移行」に書く）。進行中の測定 daemon では切り替えない（(g)）。

### S5-5 checkpoint と版の照合（目的の (4)）
- 形: `celeris.script-checkpoint/1` は JSON の object で、欄は `schema`（`"celeris.script-checkpoint/1"`）・`domain`・`workflow_id`（ULID）・`generation_id`（書いた generation）・`step`（TS が決める文字列、128 byte まで）・`state`（plain な JSON の値だけ。関数・Promise・class instance・`undefined` を入れない。直列化して 64 KiB まで）・`effects`（`{ idempotency_key, kind, applied }` の配列）・`written_at`（RFC 3339、Rust の時計）。
- 書くのは Rust だけ。TS は「次の checkpoint の `step` と `state`」を提案として返し、Rust が検証して保存する。Promise・closure・V8 の値は checkpoint に入れず、generation をまたいで移さない。新しい generation は保存された checkpoint を入力 DTO として受け、最初から関数を呼ぶ。
- manifest の `checkpoint_versions`（今の `Vec<u32>`、版の番号 `n` の並び）との照合: ある領域に保存済みの checkpoint の版の集合を V とする。その領域を新しい generation へ移す CAS（S5-4 の 1 行目）は、新しい manifest の `checkpoint_versions` が V を全部含むときだけ通す。含まなければ 422 `script_checkpoint_incompatible`。起動時の復元で含まなければ、その領域を `rust` に戻す（S5-2）。登録（S5-1）では、`checkpoint_versions` が重複なしの昇順で、host が知る版（S5 では `1` だけ）に限ることを検証する。
- S5 の 3 領域には長期 workflow が無い: `lane_rules`・`route_priority` は純粋判定、`cos_digest` の pass は 200 ms で終わり、続きは Rust の永続状態（`digest_due`・`dirty`・message の冪等 key・`WHERE state='running'`）から次の pass が作り直す。だから S5 では checkpoint の表を作らない（V は常に空で、今の bundle の `checkpoint_versions: []` で通る）。S5 で実装するのは照合の関数・登録の検証・CAS の拒否と、その試験（作り物の V で照合を確かめる）までである。表 `script_checkpoints` は、checkpoint を使う最初の領域を足す変更が、その ADR の付記と一緒に作る。

### S5-6 副作用の前後の扱いと認可の error（目的の (5)）
- **副作用の前**（`lane_rules`・`route_priority` の判定全部、`cos_digest` の apply より前）: TS の失敗（`ScriptError` の全種類）・検証落ち・`GenerationGone`・tick の予算切れ（S5-8）では、Rust の参照実装の結果を使う。記録に `fallback` の code を残す（S4-15 の形を `lane_rules`・`route_priority` の記録にも足す）。
- **副作用の後**: 副作用を起こすのは Rust の apply だけである（S4-14）。apply が途中で止まった後（再起動・cancel・generation の切替）の再実行は、command を無条件に繰り返さない。Rust が command ごとに冪等 key と実行記録で照合してから書く。
  - `post_digest`: 冪等 key `cos-triage-digest:<run_id>` の message が在れば書かない（今のまま）。
  - `requeue_run`: `WHERE state='running'` の条件付き更新（今のまま）。
  - 冪等 key を持たない副作用の command の種類は検証で拒む（`invalid_output`。今後 command の種類を足す変更は、冪等 key と照合の方法を同じ差分で決める）。
- **照合できないとき**（冪等 key の読み出しが失敗した、記録と状態が食い違う）: その run の残りの command を書かずに止める。`digest_due` を下ろさない。actor `system:script-guard` で受信箱に `script_effect_unverified` の項目を出し、その領域を `rust` に戻す（S5-7 の戻しと同じ経路）。次の pass は Rust の経路で照合し直す。照合できるまで同じ run を書かない。
- **認可の error で fail-open しない**: op の `Denied`、API の 401・403、capability・scope の拒否は、どれも「許可した」とみなす既定値にしない。TS の出力が拒否された読み出しに拠るなら、その pass・判定は Rust の参照実装で答える（Rust は自分の認可を通して読む）。拒否を理由に検査を飛ばす経路、拒否された op の代わりに空の結果を「成功」として返す経路を作らない。
- **戻しの書き込みが失敗したら**: script-guard や起動時の復元が mode の行を `rust` に戻す書き込みに失敗したら、dispatcher はその領域を process の中で `rust` として扱い（fail-closed）、書けるまで tick ごとに書き直す。

### S5-7 停止基準の監視 `script-guard`（目的の (6)。(f) の数値のまま）
- **計数の置き場**: 新しい表 `script_guard_state`（store-gen unit が migration を足す）に、(domain, generation_id) ごとに 1 行を持つ。欄: `mode`（数え始めた mode）・`window`（直近 1000 呼び出しの結果の環。各要素は `ok`・`mismatch`・`timeout`・`failure`）・`consecutive_failures`・`timeouts_10min`（直近 10 分の timeout の時刻）・`clean_streak`（説明のつかない不一致の無い連続の呼び出し数。`compare`→`canary` の前提に使う）・`calls`・`safety_violations`・`updated_at`。**再起動の後も残す**: 消すと、再起動のたびに 1000 件の窓と昇格の前提が 0 からになり、率の超過を隠し、`canary` の前提を数え直すことになるからである。generation が変われば別の行で数え直す。領域 `*` の行は全体の停止 `halted`（bool と理由）を持つ。
- **分母と数え方**: S2-8 のとおり（`outcome != "unavailable"`、重複の抑えの後）。`canary`・`ts` では TS 側の単位と、Rust 側の単位で影に走らせた TS の両方を数える。S5 は正規化の規則を足さないので、`mismatch` は全部「説明のつかない不一致」に数える。tick の予算切れ（S5-8）は `unavailable` で、分母に入れない。
- **更新**: dispatcher が記録（`script_compare_recorded`・`script_compare_log`）を追記するたびに、store の `script_guard_observe` が同じ transaction で行を更新し、純粋関数 `script_guard::evaluate`（task-core）で (f) の表を当てる。
- **戻す条件**（(f) の表のまま）: 説明のつかない不一致 1 件、直近 1000 呼び出しの timeout 率 0.1% 超または 10 分間に timeout 3 件、直近 1000 呼び出しの失敗率 0.1% 超または連続 3 件、安全違反 1 件。率は 1000 件に届いた窓で判定する（届く前は件数の条件だけ当てる）。
- **戻しの動作**: 条件に当たったら、同じ transaction で mode の行を CAS で `rust` に戻し、`script_domain_mode_changed` を actor **`system:script-guard`**、`reason` に code（`guard_mismatch`・`guard_timeout_rate`・`guard_timeout_burst`・`guard_failure_rate`・`guard_failure_streak`・`guard_safety_violation`）と計数を入れて追記する。対象は mode が `canary`・`ts` の領域。`compare` の領域は挙動が Rust のままなので mode を変えず、計数だけを残す（`clean_streak` が 0 になり昇格の前提が崩れる）。
- **安全違反**: mode にかかわらず、その generation を `invalid` にし、その generation を指す全領域を `rust` に戻し、受信箱に項目を出す。「Rust の検証を通らずに副作用が実行された」を検出したら、全領域を `rust` に戻し、領域 `*` の `halted` を立てる（S5 全体を止める。(f)）。解くのは人だけ（S5-4）。
- **受信箱**: 自動の戻しは毎回受信箱に項目を出す。daemon の `crates/celeris/src/daemon/script_guard.rs` が `script_domain_events`・`script_generation_events` の actor `system:script-guard` の行を cursor で読み、項目を作る（冪等 key は event の id）。dispatcher は LLM を呼ばない。
  - **種類 `script_control`**（script-inbox unit が足した。`InboxKind::ScriptControl`）: 受信箱では答えない種類（`project_stalled` と同じく answer は 409 `native_action_required`）。actor `system:script-startup`（S5-2 の起動時の戻し）・`system:script-guard`（S5-6 の `script_effect_unverified`、この節の戻し・無効化・停止）の記録 1 件に項目 1 件。項目にする generation の記録は `script_generation_invalidated` と `script_guard_halted` だけ（mode の CAS が同じ actor で書く `draining`・`retired` 等は同じ出来事の mode の変更の項目と重なるので作らない）。本文には領域・generation（mode の変更は戻した元）・変更（`canary → rust` 等）・`reason` の先頭の code（`guard_*`・`script_effect_unverified`・起動時の失敗の理由）と `reason` の全文を出す。
  - **永続の置き場**（migration 0106）: 表 `script_inbox_items`（id `<domain|generation>-<event id>`、`UNIQUE (source, event_id)`、`INSERT OR IGNORE`）と `script_inbox_cursors`（記録の表ごとの読み終えた id。前へだけ進む）。項目の作成・閉じる・cursor の更新は 1 つの transaction。cursor を失っても冪等 key で 2 件目を作らない。記録の表は追記専用のまま。
  - **閉じる規則**: 人の actor（`human:<name>`）の後の記録で閉じる。その領域の mode の変更はその領域の項目と領域を持たない generation の項目（停止を除く）を、その generation の `loaded`・`retired`・`invalid` への遷移はその generation の項目を、`script_guard_cleared` は停止の項目を閉じる。閉じるのはその人の記録より前の項目だけ（同じ表は id、別の表は ms の時刻で比べる）。CoS・外部 operator・`system:*` の記録は項目を作らず、閉じもしない。
  - daemon の `script_guard::spawn` が active の instance で 30 秒ごと（起動直後に 1 回）に取り込む（`tick_loop` から起動）。
- 判定の時計は store に渡す `now`（試験では注入した時計）を使う。10 分の窓も同じ時計で数える。

### S5-8 mode ts の 200 ms 待ちと tick の遅れ（目的の (7)。S4 未解決 7）
- 同期で TS を待つのは `canary` の TS 側と `ts` だけ（純粋判定 1 回 50 ms まで、digest の pass 200 ms まで）。`compare` は待たない（S2-6）。
- **1 tick で TS を待つ時間の合計を 200 ms までにする**（tick 周期 `tick_ms` 既定 2000 ms の 10%。digest の pass の deadline と同じ値で、新しい数値の目標ではない）。予算を使い切った後のその tick の判定は TS を呼ばず Rust で答え、`fallback.code = tick_budget`・outcome `unavailable` を記録する。timeout・失敗に数えない（TS の誤りでないため）。予算は tick ごとに戻る。
- **数え方**: dispatcher は tick ごとに `script_wait_us`（その tick で TS を同期で待った合計）を測り、`tick_budget` の件数と一緒に、tick の終わりに process の中の集計（直近 1000 tick の p50・p95・最大）に入れる。daemon は 10 分ごとに `script_tick_wait` の log 行を出す。S6 の overhead の報告はこの値を使う。停止基準には入れない（(f) を変えない）。
- 許容: tick の遅れの p95 が 200 ms 以下。超えたら（予算の実装の誤り）試験で落とす。`canary_dispatch` の試験で、時計を止めて 50 ms の判定を 5 回待たせ、5 回目が `tick_budget` になることを確かめる。

### S5-9 bundle の無い・古い環境での試験（目的の (8)。S4 未解決 8）
- 問題: `control-ts/dist` は git の管理外で、task の根に古い物が残り、統合の `test-affected.sh` は dist を作り直す前に走る。今は無い・古い dist で bundle を使う試験が SKIP で黙って通り、古い dist の中身が source とずれると誤って落ちうる。
- 方式:
  1. 試験の support（`crates/celeris/tests/support/script_compare.rs` と `crates/script-runtime/tests/script_host_runner.rs`）は、環境変数 `CELERIS_SCRIPT_BUNDLE_DIR` があればそこを、無ければ `control-ts/dist` を読む。
  2. `control-ts/dist` を読むときは、今の古さの判定（manifest の `domains`・`capabilities` と `bundle.config.json` の照合）に加えて、`build-bundle.mjs` が dist に書く `inputs.sha256`（`control-ts/src/`・`bundle.config.json`・`package.json`・`pnpm-lock.yaml` の内容 hash）を作業ツリーから計算した値と比べる。合わなければ古い dist として SKIP する（落とさない）。manifest の形（`celeris.script-manifest/1`）は変えない。
  3. `CELERIS_SCRIPT_REQUIRE_BUNDLE=1`、または `CELERIS_SCRIPT_BUNDLE_DIR` がある場合は SKIP しない。使えない・古い bundle は失敗にする。
  4. `scripts/dev/test-affected.sh` は、選んだ試験に `celeris` か `celeris-script` が入るとき、`ts-verify.sh` と同じ方法（`build-bundle.mjs`、offline の pnpm install）で `$TMPDIR` の使い捨ての dir に bundle を作り、`CELERIS_SCRIPT_BUNDLE_DIR` を渡す。task の根の `control-ts/dist` を読まず、書き換えもしない。作れないとき（node_modules が offline で入らない等）は渡さず、理由を 1 行出す。
  5. SKIP を数える: 試験は SKIP のとき、`CELERIS_SCRIPT_SKIP_LOG` があればその file に 1 行（試験名と理由）を足す。`test-affected.sh` はこの file を `$TMPDIR` に置いて渡し、終わりに `script bundle SKIP: N 件` を出す。N > 0 で bundle を作れていた場合は exit 1 にする（作った bundle で SKIP するのは誤り）。
- release gate（`release.sh`）に同じ方式を入れるかは S5 で決めない（進捗の提案）。

### S5-10 S6 の測定の定義（目的の (9)）
- **対の scenario**（`scripts/dev/edit-activate-baseline.sh`。cos-ts-scenario unit が足す）:
  | 対 | Rust の経路（旧） | TS の経路 |
  |---|---|---|
  | 規則表 | `policy`（`crates/task-core/src/model_policy.rs` の `RULES` に、当たらない規則を 1 つ足す。今のまま） | `policy-ts`（`control-ts/src/domains/lane_rules.ts` に同じ論理の規則を足す。今のまま） |
  | CoS digest | `cos-digest`（新。Rust の digest の参照実装 `crates/task-core/src/chat/digest.rs` の `cos_digest_decide` に、当たらない条件を 1 つ足す） | `cos-ts`（新。`control-ts/src/domains/cos_digest.ts` の判定に同じ論理の条件を足す） |
  - `cos-digest` と `cos-ts` の編集は同じ論理にする: 「`reason` が nonce を含む run には digest を出さない」のような、実際には当たらない分岐を 1 つ足す。anchor と挿入の本文は unit が決め、両方の差分を進捗に載せる。
  - S0 の `cos`（`job_wait_extend::validate`）は TS へ移していない経路なので、対に使わない。S0 の baseline として残す。
- **同じ検証責任**: 「その領域の判定を変えたとき、出荷できる成果物を作り、その領域の単体試験と Rust/TS の比較を通す」ところまでを両方の経路の 1 試行とする。
  - Rust の経路: 今の 6 段（`check`・`build`・`clippy`・`test-build`・`test-run`（対象 crate `task-core`）・`release-build`）。`test-run` は `crates/task-core/tests/script_fixtures.rs`（比較 fixture と Rust の出力の一致）を含む。
  - TS の経路: `ts-verify.sh`（型検査・bundle・領域の TS 試験・比較 fixture の replay・事前 build 済みの host での golden・generation の load と `compare` での有効化）。ADR 2026-11-02-ts-fast-verify (7) の `policy-ts` と同じ手順で、`cos-ts` は領域 `cos_digest` を対象にする。
  - Rust の経路は crate の全試験を走らせるので、TS の経路より検証が広い。この差を報告に書き、Rust 側を狭めて揃えることはしない（測定のために手順を変えない）。
- **時間**: 1 試行は 300 秒以内に収まる設計にする（warm の Rust は S0 で約 155〜225 秒、TS は目標 p95 60 秒）。300 秒を超えた試行も捨てずに記録し、`over_300s` の印を付けて p50・p95 に含める。cold の Rust（S0 で 283.5・377.2 秒）は 300 秒を超えるので S6 で測らない（S0 の値を引用する）。
- **leaf ごとの試行数**（1 leaf の壁時計 3600 秒に収める）:
  | leaf | scenario・mode | 試行数 | 生データ |
  |---|---|---|---|
  | m-ts-warm | `policy-ts` warm、`cos-ts` warm | 各 20 | `policy-ts.jsonl`・`cos-ts.jsonl` |
  | m-ts-cold | `cos-ts` cold | 20 | `cos-ts-cold.jsonl` |
  | m-policy-a / m-policy-b | `policy` warm | 10 ずつ（rep 1〜10 / 11〜20） | `policy-a.jsonl`・`policy-b.jsonl` |
  | m-cosd-a / m-cosd-b | `cos-digest` warm | 10 ずつ（rep 1〜10 / 11〜20） | `cos-digest-a.jsonl`・`cos-digest-b.jsonl` |
  - 置き場は `agent-docs/progress/2026-11-02-ts-control-layer-s6/`。2 つに分けた scenario は `--rep-start 11`（cos-ts-scenario unit が足す）で rep の番号を続ける。s6-report unit は行を変えずに a・b を順に連結して `policy.jsonl`・`cos.jsonl`（中身の `scenario` は `cos-digest`）を作り、検査（`--validate`）を通す。
- **識別の記録**（(g)）: 各行の `env` に、`commit`（`git rev-parse HEAD`）、作業ツリーが clean であること、対象の dir の tree hash（Rust は `git rev-parse HEAD:crates/task-core`、TS は `HEAD:control-ts`）、rustc・node・pnpm の版、host 名、`CARGO_TARGET_DIR` の種別（warm の共有か cold の使い捨てか）、TS の経路では候補 bundle の `bundle_sha256` と `host_api`・runner の sha256、測定の系列名（leaf の key）を入れる。tree hash・toolchain・host のどれかが違う行は同じ集計に入れない（a・b を連結するときも照合する）。S6 の言語移行の比較と、移行後の方針改善の性能比較は別の測定として記録する。

### S5-11 S5 の測定規則（S6 の測定 leaf が共通に守る）
1. **直列**: 測定 leaf は depends_on で 1 本の鎖にし、同時に 2 つ走らせない。
2. **commit 済みの作業ツリーで測る**: 測る前に `git status --porcelain` が空であること。script の編集は試行ごとに戻し、戻せなければ止める（今の `revert` のまま）。
3. **重い作業と並べない**: 自分の run の中で、測定と並べて cargo・test-affected・他の build を走らせない。試行の前後に load average（1 分）と、自分以外の `cargo`・`rustc` の process の数を記録し、0 でなかった試行に印を付ける（捨てない。報告で分けて示す）。CPU を焼く負荷をかけない。
4. **IEEE Access・K2・K4 の環境と state dir に触れない**: 測定用の release・state dir・config・pin した binary を読まず書かない。測定の daemon・state は `$TMPDIR` か run の作業場所の下に作る（(g)）。
5. **測定のためにコードを変えない**: 測定 leaf はコードと script を変えない（範囲は進捗の file と生データの jsonl だけ）。script の不具合を見つけたら測らずに plan_issue で返す。
6. **時間切れ**: 残りの壁時計が「1 試行の最大（300 秒）＋後始末 120 秒」を切ったら新しい試行を始めない。測れた試行数を進捗に書き、目標に届かなければ「未達（N/目標）」と書く。届かない分を推定で埋めない。
7. **混ぜない**: 識別（S5-10）が違う試行、prime の行、skipped の行を p50・p95 に入れない。p95 は最近傍順位（(f)）で出す。

### S5-12 後続の unit の作業範囲と試験名
試験名は unit が実装時に変えてよい。変えたら進捗ファイルに対応を書き、s5-close unit がこの表と突き合わせる。範囲 check には下の path を最初から入れる（ADR 文書・進捗ファイル・`docs/architecture-map.md` は各 unit の範囲に含める）。
| unit | 作業範囲（path） | 足す・直す試験 |
|---|---|---|
| bundle-skip | `crates/celeris/tests/support/script_compare.rs`、`crates/script-runtime/tests/script_host_runner.rs`、`control-ts/scripts/{build-bundle.mjs,build-bundle.test.mjs}`（`inputs.sha256`）、`scripts/dev/test-affected.sh`、`scripts/dev/tests/`、`control-ts/scripts/test-affected.test.mjs` | `usable_bundle_skips_stale_inputs_hash`、`usable_bundle_fails_when_bundle_dir_env_set`、`usable_bundle_require_fails_on_stale`、`skip_log_records_test_and_reason`、`build-bundle writes inputs.sha256`（node --test）、shell 試験 `scripts/dev/tests/test-affected-script-bundle.sh`（偽の build で `CELERIS_SCRIPT_BUNDLE_DIR` を渡す・作れないとき理由を出す・SKIP 1 件で exit 1） |
| runtime-gen | `crates/script-runtime/src/{generation.rs,actor.rs,handle.rs,watchdog.rs,manifest.rs,lib.rs,error.rs}`、`crates/script-runtime/tests/generation.rs`、`crates/celeris/src/daemon/script_layer.rs`・`script_policy.rs`（handle の型が変わる分の追従だけ） | `generation_load_uses_new_isolate`、`generation_load_rejects_unknown_domain_capability_or_checkpoint`、`generation_call_pins_requested_generation`、`generation_limit_two_live`、`generation_drain_cuts_pure_call_at_30s`（時計を止める）、`generation_call_to_retired_is_generation_gone`、`generation_in_flight_64_then_busy`、`checkpoint_versions_must_cover_stored`（作り物の V） |
| store-gen | `crates/task-core/migrations/`（新 migration）、`crates/task-core/src/store/{script_domains.rs,script_domains_tests.rs,script_guard.rs}`、`crates/task-core/src/{script_compare.rs,script_guard.rs}`、`crates/task-core/src/script_compare/` | `script_generation_state_transitions`、`script_generation_rejected_rows_migrate_to_invalid`、`script_generation_events_append_only`、`script_domain_canary_bps_check`、`script_domain_mode_cas_moves_generation_states`、`script_guard_observe_counts_per_generation`、`script_guard_evaluate_each_stop_condition`（(f) の 4 行と 10 分・連続 3 件）、`script_guard_state_survives_reopen` |
| api-canary | `crates/task-api/src/{script_domains.rs,script_generations.rs,lib.rs}`、`crates/task-api/tests/{script_domains.rs,script_generations.rs}`、`crates/task-api/src/cos/operations.rs`・`cos/ops/`、`agent-docs/adr/2026-10-09-cos-operations-all-mutations.md`、`config/skills/cos-operator/operations.md`、`docs/api/v1/api-v1.schema.json`、`web/api/generated/`（再生成） | `script_generation_register_503_when_unwired`、`script_generation_register_invalid_422`、`script_generation_limit_409`、`script_domain_mode_canary_requires_human`、`script_domain_mode_canary_requires_guard_ready`、`script_domain_mode_ts_requires_same_generation_canary`、`script_domain_mode_rollback_by_anyone`、`script_domain_mode_refused_while_halted`、`script_guard_clear_human_only`、CoS registry の試験の追従 |
| script-inbox | `crates/task-ops/src/{human_inbox.rs,inbox.rs}`・`human_inbox/tests.rs`、`crates/task-core/src/script_inbox.rs`・`migrations/0106_script_inbox.sql`、`crates/celeris/src/daemon/{script_guard.rs,script_guard_tests.rs,tick_loop.rs}`、`crates/task-api/src/inbox_notifications.rs`、`web/features/inbox/`、`docs/api/v1/`・`web/api/generated/`（再生成） | `script_inbox_item_once_per_event_survives_restart`（S5-7 の `script_guard_inbox_item_once_per_event` に当たる）、`script_inbox_human_actor_creates_no_item_and_closes_open_ones`、`script_inbox_generation_invalidated_and_halt_items`、`script_inbox_reason_code_takes_leading_code`、`script_inbox_item_shows_domain_generation_and_code`（task-ops） |
| daemon-gen | `crates/celeris/src/daemon/{script_layer.rs,script_policy.rs}`、`crates/celeris/src/config/script.rs`（`active_bundle_sha256` を消す）、`crates/celeris/tests/script_compare_*.rs`（config の追従）、`config/celeris.example.toml` の `[script]` の例（あれば） | `daemon_registers_generation_in_new_isolate`、`daemon_switch_drains_old_generation`、`daemon_restore_keeps_loaded_modes`、`daemon_restore_rolls_back_missing_or_invalid_generation`、`config_active_bundle_sha256_is_rejected` |
| canary-dispatch | `crates/task-dispatch/src/dispatcher/{script_compare.rs,work_units.rs,route_select.rs}`、`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs`、`crates/task-dispatch/src/script_policy.rs`、`crates/task-dispatch/src/dispatcher/tests/`、`crates/celeris/src/daemon/script_policy.rs`（trait を変える分） | `canary_selection_is_deterministic_per_unit`（3 領域）、`canary_ts_side_uses_validated_ts_result`、`canary_rust_side_still_compares`、`canary_fallback_before_effect_uses_rust`、`canary_effect_unverified_stops_and_rolls_back`、`canary_denied_never_fails_open`、`tick_budget_200ms_falls_back_after_budget`、`rollback_write_failure_treated_as_rust` |
| guard | `crates/celeris/src/daemon/script_guard.rs`、`crates/celeris/src/daemon.rs`・`crates/celeris/src/daemon/tick_loop.rs`（起動）、`crates/celeris/tests/script_guard.rs` | `script_guard_rolls_back_on_one_mismatch`、`script_guard_rolls_back_on_timeout_rate_and_burst`、`script_guard_rolls_back_on_failure_rate_and_streak`、`script_guard_safety_violation_invalidates_generation`、`script_guard_unvalidated_effect_halts_all`、`script_guard_inbox_item_once_per_event`、`script_guard_counts_survive_restart` |
| gen-e2e | `crates/celeris/tests/script_generation_e2e.rs`（新）、`crates/celeris/tests/support/` | `CELERIS_SCRIPT_REQUIRE_BUNDLE=1`（または `CELERIS_SCRIPT_BUNDLE_DIR`）で本物の bundle 2 つ（A と印付きの B）を使う `script_generation_e2e_coexist_switch_error_rollback_restart`: A を active にした daemon に B を登録→併存→B に切替（A は drain）→B の途中 error→script-guard が `rust` に戻す→再起動で active generation を復元。event の欠落 0（`script_domain_events`・`script_generation_events` の連番に穴が無い）、message は冪等 key ごとに 1 件 |
| cos-ts-scenario | `scripts/dev/edit-activate-baseline.sh`、`scripts/dev/tests/`（偽の cargo・runner の試験） | `cos-digest`・`cos-ts` の `--dry-run`、`--rep-start`、`env` の識別の欄（S5-10）、試行の前後の load の記録。shell 試験 `scripts/dev/tests/edit-activate-cos-ts.sh`（anchor が 1 回だけ在る・編集が戻る・`--validate` が新しい欄を検査する） |
| s5-close | 進捗のまとめ、`docs/SPEC.md`（運用の節）、`docs/architecture-map.md`、この付記の実装との突き合わせ（S5-13 として足す）、「本番への移行」 | 文書の検査（`check-doc-links.sh`・`check-adr-numbers.sh`・`progress-index.sh --check`・`check-architecture-map.py`） |

### S5-13 実装との突き合わせ（S5 s5-close、2026-11-03）
記録は [S5 の進捗](../progress/2026-11-02-ts-control-layer-s5.md) と各 unit の記録（`../progress/2026-11-02-ts-control-layer-s5/`）。S5-1〜S5-9・S5-12 を実装と突き合わせた結果である。(f) の数値は変わっていない（定数は `crates/task-core/src/script_guard.rs`）。権限・安全・停止基準の規則に関わる違いは無い。違いは置き場所・名前・起点の時刻・試験の範囲である。S5-10・S5-11 は S6 の測定 leaf が使うので、ここでは突き合わせない。

| 節 | 実装 | 付記との違い |
|---|---|---|
| S5-1 | 登録は `crates/task-api/src/script_generations.rs`（trait `ScriptGenerationControl`、`GET`・`POST /api/v1/script/generations`、`POST …/{id}/retire`）→ daemon の `crates/celeris/src/daemon/script_layer.rs` の `ScriptGenerations` → `crates/script-runtime/src/generation.rs` の `GenerationRegistry::register_deployed`。状態と遷移は `crates/task-core/migrations/0105_script_generations_s5.sql` と `store/script_domains.rs` の `transition_tx`（状態を書く唯一の場所）、記録は追記専用の `script_generation_events`。`[script] active_bundle_sha256` は読み込みエラー | (1) 登録簿の key は DB の id でなく `bundle_sha256`（検証してから記録する順なので、検証の時点で DB の id が無い）。(2) migration は指されない旧 `loaded` を `retired` にした（再起動で isolate は残らない。再登録で `loaded` に戻る）。(3) 起動時の復元は isolate の無い `loaded` を `retired`（reason `startup_no_isolate`）にする。(4) bundle の file が無い hash の登録も `invalid` になり、同じ hash では登録し直せない（進捗の未解決） |
| S5-2 | 固定は `GenerationRegistry::call`（future を作った時点で generation の `Arc` と in-flight の枠を取る）、in-flight は generation ごとに 64、drain の打ち切りは `DRAIN_CUTOFF`（30 秒、trait `GenerationClock` の時計）。daemon の見回りは `RECONCILE_INTERVAL`（1 秒）ごとに表の `draining`・`retired`・`invalid` を登録簿に当てる | (5) 30 秒は CAS の時刻でなく、見回りが drain を始めた時刻から数える（最大 1 秒遅れる）。(6) 30 秒の時点の打ち切りは `terminate_execution` でなく、呼び出し側へ `GenerationGone` を返して actor が後から来た結果を捨てる形。同期の実行は要求の deadline（50 ms）で watchdog が止める。(7) 起動時の復元は行から指されない `draining` を `retired`（`cancelled_in_flight` 0、reason `drained`）にする（core-fix の提案を daemon-gen が入れた）。(8) standby の daemon は isolate を作らず、active になってから最初の見回りで復元する |
| S5-3 | `task_core::script_compare::canary_selected`（付記の sha256 の式のまま）、単位の選択は `crates/task-dispatch/src/dispatcher/script_canary.rs`。記録の `canary_selected`・`canary_bps` は `ScriptCompareRecord`・`CosDigestCompareRecord` の serde default の欄 | 違いなし。`route_priority` の TS 側で候補列が Rust と違うときは `RouteContext::priority_override` で選び直す（付記に無い実装の細部）。その dispatcher の試験は無い（進捗の未解決） |
| S5-4 | `crates/task-api/src/script_domains.rs`（検査の順: 領域名 404 → `canary`・`ts` の主体 403 → generation の解決 422 → store の CAS）、`POST /api/v1/script/guard/clear`。CoS registry（`crates/task-api/src/cos/ops/admin.rs`）は本文の `mode` が `canary`・`ts` なら effect の前に 403 `human_only` | 違いなし。`script_mode_not_enabled`・`ts_may_skip_canary`・`ScriptMode::accepted_for` は消した。(9) `celerisctl script mode` は `rust`・`compare`・`ts` だけで、`canary`・`canary_bps`・generation の登録・退役・停止の解除の subcommand は無い。help の文面は S4 の「`ts` は `cos_digest` だけ・`compare` から」のまま（`crates/celerisctl/src/commands/script.rs`。進捗の未解決） |
| S5-5 | 照合は `checkpoint_versions_cover`（runtime と task-core の両方）、保存済みの版は `script_checkpoint_versions_stored`（S5 では常に空）。登録の検証は昇順・重複なし・host の版 `[1]` の中 | 違いなし。表 `script_checkpoints` は作っていない |
| S5-6 | 副作用の前の fallback の code（`ts_error`・`invalid_output`・`safety_violation`・`denied`・`unavailable`・`tick_budget`）は `script_canary.rs`。副作用の後は `chat_assistant_message_add_once` の `Conflict` と保存済み message の thread・run で照合し、照合できなければ pass の残りを書かず `digest_due` を下ろさず `system:script-guard` の `script_effect_unverified: …` で `rust` に戻す。戻しの書き込みに失敗した領域は process の中で `rust` として扱い、tick ごとに書き直す | (10) op の `denied` は、TS の出力がその読み出しに拠るときだけ fallback にする（拒否された読み出しを使わない出力は検証に回す）。fail-open の経路は無い（`canary_denied_never_fails_open`） |
| S5-7 | 計数・判定・戻しは store の `script_guard_observe`（`crates/task-core/src/store/script_guard.rs`。`script_compare_append_observed`・`script_compare_log_append_observed` が追記と同じ transaction で呼ぶ）と純粋関数 `crates/task-core/src/script_guard.rs`。daemon の `crates/celeris/src/daemon/script_guard.rs` は 30 秒ごとの見回りで、検証の外の副作用の検出（`guard_unvalidated_effect` で `script_guard_halt`）・進む条件の log・受信箱の取り込みを行う | (11) 計数の「側」は記録の `canary_selected` でなく append 関数の `ts_side` 引数で受ける。`evaluate` は側を見ず、Rust 側の影の不一致・安全違反でも戻す。(12) 検証の外の副作用は `cos_digest` の記録の自己矛盾（`applied_by = ts` と mode・出力・安全違反・fallback の組）で見つける。apply と記録は同じ transaction にない。`crates/task-dispatch/src/dispatcher/cos_chat/digest.rs` は `apply_digest_pass` の後に `append_pass_record` を別に呼ぶ（同じ transaction なのは記録の追記と計数）。その間で process が落ちると TS の apply の記録が残らず計数もされず、この検出でも見つけられない。追記の失敗も warn だけなので、同じく記録と計数が残らない。(13) 見回りは `script_compare_log` を毎回全件読む（進捗の未解決）。(14) 受信箱の `script_control` の項目は CoS の triage の対象にも入る（`InboxKind::ALL`）。項目は受信箱では答えられず（409 `native_action_required`）、CoS は `canary`・`ts` へ上げられない（403）ので、CoS が行えるのは人への知らせと `rust`・`compare` への戻しだけである。gen-e2e で、daemon がこの項目から triage run を起こすことを確かめた |
| S5-8 | `ScriptTickWait`（1 回 50 ms、1 tick 合計 200 ms）、tick の始めの `begin_tick`、集計 `Dispatcher::script_tick_wait_summary`（直近 1000 tick の p50・p95・最大と `tick_budget` の件数） | (15) daemon が 10 分ごとに出す `script_tick_wait` の log 行は無い（集計の関数だけがある）。(16) 試験 `tick_budget_200ms_falls_back_after_budget` は待ち時間の値の差し替えで確かめる。dispatcher の中で 50 ms の判定を 5 回待たせる試験は無い（TS の待ちは別の current-thread runtime で走り、試験の `tokio::time::pause` が届かない） |
| S5-9 | 試験の support（`crates/celeris/tests/support/script_compare.rs`）は `control-ts/dist` を読まず、`CELERIS_SCRIPT_BUNDLE_DIR` が無ければ control-ts の source の内容 hash を名前にした `$TMPDIR/celeris-script-bundle/<hash>` に bundle を作って読む。`test-affected.sh` は bundle を作って `CELERIS_SCRIPT_BUNDLE_DIR` と `CELERIS_SCRIPT_REQUIRE_BUNDLE=1` を渡し、SKIP の件数を出す | **S5-9 の (1)(2) を改める**: 試験は `control-ts/dist` を読まない。dist の `inputs.sha256` は使わない（`build-bundle.mjs` は変えていない）。(3)〜(5) はそのまま。release gate で REQUIRE を立てるかは決めていない |
| S5-12 | 試験名の対応 | runtime-gen・store-gen・canary-dispatch・guard は付記の名前のまま。api-canary は `script_generation_register_503_when_unwired`・`…_invalid_422`・`script_generation_limit_409` を `script_domain_generation_*` の名前で、`script_guard_clear_human_only` を `script_domain_guard_clear_checks`（`script_domains.rs` の中の検査）で作った。script-inbox は `script_guard_inbox_item_once_per_event` を `script_inbox_item_once_per_event_survives_restart` で作った。daemon-gen は各名前に `script_` の接頭辞を付けた（`script_daemon_*`・`script_runtime_daemon_restore_*`）。gen-e2e は `script_generation_e2e_switch_error_rollback_restart`。bundle-skip は (1)(2) を改めたので `usable_bundle_*` の 3 件と node の `inputs.sha256` の試験は無く、`script_compare_fresh_bundle_follows_source`・`script_compare_unbuildable_bundle_is_skipped_and_logged` と shell 試験 `scripts/dev/tests/script-bundle-fresh.sh` に替えた |

## 付記 2026-11-03: S6 の結果と二重実装を整理する終了条件（S6 s6-report、task 01M6FPPJN7A8H427MXFVYFY7H4）
記録は [S6 の進捗](../progress/2026-11-02-ts-control-layer-s6.md)。(f) の数値は変えていない。

### S6-1 生データの file 名（S5-10 との違い）
連結 file と TS の file は S5-10 の表の名前のとおりで、部分 file だけ名前が違う。連結は測定 unit が済ませ、改名は [raw-names](../progress/2026-11-02-ts-control-layer-s6/raw-names.md) が `git mv` で行った（中身は 1 byte も変えていない）。中身の `scenario` は S5-10 のとおりで、`cos.jsonl` の scenario は `cos-digest`（S0 の `cos`〈`job_wait_extend`〉とは別）である。

| S5-10 の名前 | 今の file（`../progress/2026-11-02-ts-control-layer-s6/`） | 中身 |
|---|---|---|
| `policy-ts.jsonl`・`cos-ts.jsonl` | 同じ | 各 20 試行 |
| `cos-ts-cold.jsonl` | 同じ | 20 試行 |
| `policy-a.jsonl`・`policy-b.jsonl` | `policy-warm-a.jsonl`・`policy-warm-b.jsonl` | 各 10 試行 |
| `policy.jsonl` | 同じ | a+b の連結、20 試行 |
| `cos-digest-a.jsonl`・`cos-digest-b.jsonl` | `cos-digest-warm-a.jsonl`・`cos-digest-warm-b.jsonl`・`cos-digest-warm-c.jsonl` | 7＋7＋6 試行 |
| `cos.jsonl` | 同じ | a+b+c の連結、20 試行 |

**検証の範囲（S5-10 との違い）**: S5-10 は Rust の経路の `test-run` を「対象 crate `task-core`」と書く。`policy` はそのとおりだが、`cos-digest` の実測は `check`・`clippy`・`test-build`・`test-run` を `-p task-core -p task-dispatch` の 2 crate で走らせた（`cos.jsonl` の `cmd`。`build`・`release-build` は `policy` と同じ `-p celeris`）。`task-dispatch` は digest の pass と Rust/TS の比較の試験を持つので cos-ts-scenario unit が足した（[cos-ts-scenario](../progress/2026-11-02-ts-control-layer-s6/cos-ts-scenario.md)）。このため `cos-digest` の p50 は `policy` より 110.0 秒長く（344.491 と 234.481）、TS の経路との比の約 46 倍はこの 2 crate の範囲の値である。`task-core` だけの `cos-digest` は測っていない。

- 集計は `python3 -I scripts/dev/edit-activate-summary.py <jsonl>...`。file をまたいで同じ記録を 1 回だけ数え、20 試行未満の scenario・mode があれば exit 1 にする。
- `cos-digest` warm は 1 試行約 345 秒で、prime と合わせて 1 leaf に 7 試行しか入らない。S5-10 の「10 ずつ」は守れず、3 leaf（7＋7＋6）で 20 試行を取った（推定で埋めていない）。

### S6-2 打ち切り基準の判定
TS の経路の warm p50 は `policy-ts` 7.519 秒・`cos-ts` 7.470 秒で、(f) の 78 秒を超えない。移行は打ち切らない。

### S6-3 二重実装を整理する終了条件（領域ごと）
Rust の参照実装（`lane_rules` は `model_policy::match_rule`、`route_priority` は `llm_route::route_priority_decide`、`cos_digest` は `chat::digest::cos_digest_decide`）を消してよいのは、その領域で次の全部が成り立ち、人が承認したときだけである。1 つでも欠けたら残す。
1. **本番で ts の観測**: 本番でその領域が同じ generation 系列の `ts` のまま連続 30 日続き、その間に script-guard の自動の戻し・説明のつかない不一致・安全違反・検証の外の副作用がどれも 0 件。呼び出しは純粋判定で 1 万件以上、CoS の経路で 2,000 件以上（(f) の進む条件の 10 倍）。
2. **golden の凍結**: 比較 fixture の期待値を、Rust の出力から生成する形から、repo に commit した凍結 file に移す。ts-verify・runner の golden が Rust の参照実装を呼ばずに採点でき、その状態で全件一致する。
3. **fallback の置き換え**: Rust 参照へ戻す fallback（副作用の前の `ts_error`・`invalid_output`・`timeout` など。S5-6）の代わりの fail-closed の既定を、領域ごとにこの ADR に書き、試験で固定する（例: `lane_rules` は lane の天井の側、`cos_digest` は digest を出さず受信箱に知らせる）。既定は今の Rust の結果より安全条件を緩めない。
4. **戻し先の確保**: known-good の TS bundle を 2 世代以上 `<state_dir>/script-bundles/` に残し、その hash を進捗に書く。Rust 参照の削除は 1 commit にまとめ、git revert で戻せるようにする。
5. **方針改善の手順**: 二重実装をやめた後の方針改善は、凍結した golden の更新を人が review する手順で行う（S6 pilot の「分かったこと」。TS だけの変更は今の compare では不一致として戻される）。

消さないもの: Rust の再検証と安全条件、script-guard、generation の登録・切替・drain、認可・永続状態・scheduler（(a)）。`rust` mode の意味は「Rust 参照で判定する」から「fail-closed の既定で判定する」に変わるので、mode 表と API の説明を同じ変更で直す（ADR 2026-10-11-breaking-changes-by-default）。

### S6-4 今の状態
- 本番は 3 領域とも `rust` で、S6-3 の 1 を満たす領域は無い。Rust の参照実装はすべて残す。
- 次に移す領域の選定は、本番で 1 領域が `ts` に上がり S6-3 の 1 の観測が始まってからにする。

### 付記 P0-B（2026-11-03）: `cos_digest` の apply と記録の原子化
S5-13 の S5-7 の行の (12) のうち「apply と記録は同じ transaction にない」「追記の失敗も warn だけ」は、
[ADR 2026-11-03-cos-digest-persist](2026-11-03-cos-digest-persist.md) で改めた。digest pass の apply は 1 つの transaction で、
run ごとに message・requeue・記録・script-guard の計数（mode `compare` と canary の Rust の側は影の outbox の行）を同じ SAVEPOINT に入れる。
S4-14 の「apply の直前の mode の読み直し」は、TS の出力を apply する pass では transaction の中で行う。
`append_pass_record`・`script_compare_log_append_observed` は消した。記録の自己矛盾による検出（(12) の前半）は変えない。

## 付記 P1-C（2026-11-03、task 01M6HGFR1TR6PJ8822DHW7NNNT）
- S5-1 の 6 を狭めた: 配備先の dir が無い hash の登録は `invalid` にせず 409 `script_bundle_not_deployed`（配備の後に登録し直せる）。
  配備された bundle が検証に落ちたときだけ `invalid`。S5-8 の tick の待ちは `DaemonSnapshot.script_tick_wait` と 10 分ごとの log 行で
  読める。S5-7 の見回りは `script_compare_log` を id の cursor から差分で読む。CLI の追加を含め、詳細は
  [2026-11-03-ts-control-ops-cli-observe](2026-11-03-ts-control-ops-cli-observe.md)。

## 付記 P1-E（2026-11-03、task 01M6HGFR1TMY7PTGQ553FQGM7H）
本番の `compare` の件数は読み取り専用の道具で数える。mode・計数・(f) の数値・権限は変えない。
- **道具**: `scripts/dev/script-compare.py`（`preflight`・`report`・`sample`）。DB は `sqlite3 -readonly` と `PRAGMA query_only=ON` で開き、1 つの read transaction で読む。測定環境の DB は開かない。
- **件数の定義**: S2-8 のまま。`events` の `script_compare_recorded` と `script_compare_log` の行を (domain, generation) ごとに数え、`unavailable` は分母に入れない。ready は `script_guard::readiness` と同じ条件で、説明のつかない不一致の無い連続が純粋判定 1000・CoS 200 以上。
- **照合**: `script_guard_state` の計数は記録と照合するだけに使う。合わないとき（記録の無い計数を含む）は problem で exit 1 にし ready にしない。canary・ts を通った generation は guard が mode の変わり目で数え直すので、記録より多いときだけ破れとする。これで計数だけを書いた synthetic の DB を本番の件数と取り違えない。
- **差分を隠さない**: `normalized` が空でない記録、`match` なのに `rust` と `ts` が違う記録、未知の outcome は数えずに problem にする。意図した差分の outcome が増えたら、その変更でこの道具の `OUTCOMES` と定義を直す。
- **手順の順序（S5「本番への移行」の改め）**: 今の本番 release（S5 より前）は `[script]` を未知の欄として拒む。`[script]` は S5 以後の release を promote した後に足し、daemon を再起動する。release を戻すときは先に `[script]` を消す。手順は [P1-E の進捗](../progress/2026-11-03-ts-control-ops/p1e-shadow-preflight.md) の「本番への移行」。

## 付記 P2-F（2026-11-04、task 01M6HGFR1TCHVSDDNH0GR9SNBK）
- 4 つ目の領域 `execution_gate` を足した。S2-3（本文を TS に渡す例外と記録の除去）・S2-8（安全違反の code `gate_rule_not_allowed`・`gate_threshold_violated`）・S6-4（選定の前提）の改めと詳細は [ADR 2026-11-04-ts-execution-gate-domain](2026-11-04-ts-execution-gate-domain.md)。
- S6-3 の終了条件はこの領域にも同じく当てる。Rust の参照実装は `execution_gate::score`。今は条件を満たさないので消さない。

## 付記 2026-11-04: (f) の `lane_rules` の進む条件を評価の開始後に改める（task 01M6J81DZ7C8SMY7HNZTW8GT0Z）
(f) は「評価の前に固定する」と書いた基準である。本番の評価は 2026-11-04T02:41:21Z に `compare` で始まっており、この付記は**評価の開始後の変更**である。元の行は消さず、ここで改める。
- **決めた主体と日**: 人が 2026-11-04 に「本番でも網羅を流す＋100 件」と決めた。
- **理由**: `lane_rules` の入力は 9 軸×3 段階の 19,683 通りで有限であり、全件を本番の isolate で確かめられる。本番の呼び出しは約 48 件/日で、1000 件まで約 21 日かかる。残る確認は本物の task からの入力の組み立てと、本番での timeout・失敗である。
- **改める条件**: `lane_rules` の `compare→canary`・`canary→ts` は、全入力の網羅の証拠が合格なら、説明のつかない不一致の無い連続 100 件で、窓の中の timeout・失敗が 0 件、安全違反 0 件。証拠が無い・不合格なら (f) の 1000 件と 0.1% のまま。
- **変えないもの**: 「即 `rust` へ戻す条件」の列、`route_priority`・`cos_digest` の条件、付記 S5-4 の権限、付記 S6-3 の終了条件（純粋判定 1 万件以上は 1000 件のときの値のまま固定）。付記 S5-7 の計数に網羅の呼び出しを混ぜない。
- **網羅の不一致・安全違反**: live と同じく 1 件で不合格にし、付記 S5-7 と同じ戻しを行う。
- 操作・証拠の置き場・tick を止めない方法・権限の詳細は [ADR 2026-11-04-lane-rules-exhaustive-evidence](2026-11-04-lane-rules-exhaustive-evidence.md)。
