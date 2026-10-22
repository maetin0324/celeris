//! アカウントの管理（ADR-0017）。`create`/`patch`/`delete` は account directory への
//! ファイル読み書きだけで完結する（LLM もワーカーも起動しない、DESIGN §5.10 の境界を守る）。`reload`/`check` は
//! 稼働中の `Dispatcher` の差し替え・実際のワーカー起動が要るので、celeris（task-worker/task-dispatch に依存する側）
//! へ `AdminRequest` で委譲する（ADR-0017 M2）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::AccountAdapter;
use tokio::sync::oneshot;

/// task-api → celeris（`ApiSettings.admin_tx` 経由）。稼働中の `Dispatcher`・ワーカーの起動が要る操作だけを運ぶ。
pub enum AdminRequest {
    /// 設定ファイルを読み直し、稼働中の source・harness の選定と実行アダプタ一式を差し替える。
    Reload {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// ADR-0024 D6 / ADR-0025 D4: プールのアカウントを 1 つ確認する（claude-code は `[accounts].check_model`
    /// を使う。codex はモデルを指定しない）。
    AccountCheck {
        adapter: AccountAdapter,
        id: String,
        reply: oneshot::Sender<Result<AccountCheckOutcome, AccountAdminError>>,
    },
    /// ADR-0024 D7 / ADR-0025 D5: ログインを開始する（claude-code は `claude auth login`、codex は
    /// `codex login --device-auth`）。
    AccountLoginStart {
        adapter: AccountAdapter,
        id: String,
        reply: oneshot::Sender<Result<AccountLoginStartOutcome, AccountAdminError>>,
    },
    /// ADR-0024 D7: 認可コードを渡して待つ（claude-code のみ。codex は 409 `login_code_not_supported`）。
    AccountLoginCode {
        id: String,
        code: String,
        reply: oneshot::Sender<Result<AccountLoginCodeOutcome, AccountAdminError>>,
    },
    /// ADR-0024 D7 / ADR-0025 D5: 進行中のログインを止める（無ければ何もしない）。
    AccountLoginCancel {
        adapter: AccountAdapter,
        id: String,
        reply: oneshot::Sender<Result<(), AccountAdminError>>,
    },
    /// S2+S8: `DELETE /accounts/{id}`。celeris 側（ディスパッチャの権威ある `account_in_use`）で行う
    /// （task-api のスナップショット由来の `in_use` はレースしうるため。ADR-0024 D5 の実装をここへ寄せる）。
    AccountRemove {
        adapter: AccountAdapter,
        id: String,
        reply: oneshot::Sender<Result<(), AccountAdminError>>,
    },
    /// ADR 2026-10-08 account-concurrency-limits §(d): `PATCH /accounts/{adapter}/{id}`。celeris 側で存在を
    /// 確かめ（実: 根の下のディレクトリ、暗黙: 非プール行）、DB に書いてからディスパッチャの表を更新する
    /// （次の tick から効く）。`adapter` は `AccountAdapter::as_str()` か `"implicit"`。`None` は保存値を消す。
    AccountLimitSet {
        adapter: String,
        id: String,
        max_runs: Option<usize>,
        reply: oneshot::Sender<Result<(), AccountAdminError>>,
    },
    /// ADR 2026-10-22-dynamic-config D2: `PATCH /config/fields`。検証してから config.toml へ書き戻し、
    /// 稼働中の daemon に入れ、変更を `config_events` へ追記する。失敗したら何も変えない。
    ConfigUpdate {
        changes: Vec<crate::types::ConfigPatchChange>,
        actor: String,
        reply: oneshot::Sender<Result<crate::types::ConfigPatchResult, ConfigAdminError>>,
    },
    /// ADR 2026-10-22-dynamic-config D2: `GET /config/fields`（config.toml の欄と分類、稼働中の値）。
    ConfigFields {
        reply: oneshot::Sender<Result<crate::types::ConfigFieldsView, ConfigAdminError>>,
    },
    /// ADR-0032 D5: `POST /clusters/{id}/connect`。コード無しで張れれば `kind = "connected"`、TOTP 等の
    /// プロンプトが要れば `kind = "needs_code"`。
    ClusterConnectStart {
        id: String,
        reply: oneshot::Sender<Result<ClusterConnectStartOutcome, ClusterAdminError>>,
    },
    /// ADR-0032 D5: `POST /clusters/{id}/connect/code`。コードはこの要求の中だけを通り、ログにも応答にも
    /// 残らない（ハンドラ側の規律。ここでは受け渡すだけ）。
    ClusterConnectCode {
        id: String,
        code: String,
        reply: oneshot::Sender<Result<ClusterConnectCodeOutcome, ClusterAdminError>>,
    },
    /// ADR-0032 D5: `DELETE /clusters/{id}/connect`。進行中の接続セッションを取り消す、または張った接続を切る。
    ClusterConnectCancel {
        id: String,
        reply: oneshot::Sender<Result<(), ClusterAdminError>>,
    },
    /// ADR-0037 D4（Phase 39）: `POST /notify/test`。celeris が `[secrets]` から webhook の URL を読み、
    /// その場で 1 通送る（HTTP クライアントを持つのは celeris 側だけ）。
    NotifyTest {
        reply: oneshot::Sender<Result<NotifyTestOutcome, NotifyAdminError>>,
    },
    /// ADR 2026-10-14-local-machine-settings D4: `PUT /clusters/local/settings`（と CoS の
    /// `local.settings_put`）が store の上書きを書いた後の知らせ。dispatcher は store から読み直し、
    /// `max_concurrency` を次の tick から効かせる（返事は待たない）。
    LocalSettingsChanged,
}

/// ADR-0037 D4: `POST /notify/test` の結果。**URL・ホスト名は含まない**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyTestOutcome {
    pub ok: bool,
    /// 人が読む一行（失敗の種別だけ）。
    pub detail: Option<String>,
}

/// ADR-0037 D4: テスト送信ができない理由（ハンドラが 409 `notify_unavailable` へ写す）。
#[derive(Debug, Clone)]
pub enum NotifyAdminError {
    /// 秘密が無い・HTTP クライアントが無い等。文面に URL は入れない。
    Unavailable(String),
}

/// ADR-0024 D6: `POST …/accounts/{account_id}/check` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountCheckOutcome {
    pub result: AccountCheckResult,
    pub detail: Option<String>,
    /// 観測できた `rate_limit_event`（あれば）。`AccountUsageView` に写す（`source = "check"`）。
    pub observation: Option<task_core::RateLimitObservation>,
}

/// ADR-0024 D7 / ADR-0025 D5: `POST …/accounts/{account_id}/login` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountLoginStartOutcome {
    pub url: String,
    /// Unix 秒（claude-code は 10 分後、codex は 15 分後）。
    pub expires_at_unix: i64,
    /// codex の一回限りのコード（claude-code は `None`。ログには出さない）。
    pub user_code: Option<String>,
}

/// ADR-0024 D7: `POST …/accounts/{account_id}/login/code` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLoginCodeOutcome {
    pub ok: bool,
    /// 認可コード・URL は含まない。
    pub detail: Option<String>,
}

/// アカウント管理系の要求が完了できなかった理由（ハンドラが HTTP へ写す）。
#[derive(Debug, Clone)]
pub enum AccountAdminError {
    /// 指定した id のアカウントディレクトリが無い。
    NotFound,
    /// `[accounts]` が設定されていない、または celeris 側に届かなかった。
    Unavailable(String),
    /// `login/code` を呼んだが進行中のログインが無い。
    LoginNotStarted,
    /// `login` の開始自体に失敗した（15 秒以内に URL が出ない等）。
    LoginFailed(String),
    /// S2+S8: `DELETE /accounts/{id}` で `account_in_use > 0`（ディスパッチャの権威ある値）。
    InUse,
    /// ADR-0025 D5: codex は `login/code` を使わない（device フローで完結する）。
    LoginCodeNotSupported,
}

/// Account connectivity check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountCheckResult {
    Ok,
    AuthFailed,
    Throttled,
    SpawnFailed,
}

/// ADR-0032 D5: `POST /clusters/{id}/connect` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterConnectStartOutcome {
    /// `"connected"` | `"needs_code"`。
    pub kind: String,
    /// `kind = "needs_code"` のときだけ。ssh が出したプロンプト文字列（ログには出さない）。
    pub prompt: Option<String>,
    /// `kind = "needs_code"` のときだけ（Unix 秒）。
    pub expires_at_unix: Option<i64>,
}

/// ADR-0032 D5: `POST /clusters/{id}/connect/code` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterConnectCodeOutcome {
    pub ok: bool,
    /// コード・URL は含まない。
    pub detail: Option<String>,
}

/// ADR-0032 D5: クラスタ接続の管理系が完了できなかった理由（ハンドラがこれを HTTP へ写す）。
#[derive(Debug, Clone)]
pub enum ClusterAdminError {
    /// 指定した id が `[[clusters]]` に無い。
    NotFound,
    /// `auth = "manual"` のクラスタに `connect` した（人の操作で接続する運用のまま）。
    NotSupported,
    /// 進行中のセッションが無いのに `connect/code` を呼んだ。
    NotStarted,
    /// コードが空・制御文字を含む（ssh には渡していない）。
    InvalidCode,
    /// 接続そのものの失敗（ssh の失敗、タイムアウト等）。
    Failed(String),
}

/// `ConfigUpdate` / `ConfigFields` の失敗（ADR 2026-10-22-dynamic-config）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAdminError {
    /// 値・欄の検証に落ちた（422。何も変えていない）。
    Invalid(String),
    /// 起動時にしか決められない欄、または secret の欄（422。再起動が要る旨を `detail` に持つ）。
    RestartRequired(String),
    /// 設定ファイルを読めない・書けない等（500）。
    Internal(String),
}
