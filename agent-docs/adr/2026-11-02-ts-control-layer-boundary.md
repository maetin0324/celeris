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
