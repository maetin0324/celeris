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
