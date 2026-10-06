//! プロバイダ（アカウント）の管理（ADR-0017）。`create`/`patch`/`delete` は `providers.d/<id>.toml` への
//! ファイル読み書きだけで完結する（LLM もワーカーも起動しない、DESIGN §5.10 の境界を守る）。`reload`/`check` は
//! 稼働中の `Dispatcher` の差し替え・実際のワーカー起動が要るので、celeris（task-worker/task-dispatch に依存する側）
//! へ `AdminRequest` で委譲する（ADR-0017 M2）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::{
    AccountAdapter, LlmSourceRef, ProviderKind, ResolvedLlmSource, SourceOrigin, Tier,
};
use tokio::sync::oneshot;

use crate::types::ProviderConfigView;

/// task-api → celeris（`ApiSettings.admin_tx` 経由）。稼働中の `Dispatcher`・ワーカーの起動が要る操作だけを運ぶ。
pub enum AdminRequest {
    /// 設定ファイルと `providers.d/` を読み直し、稼働中のプロバイダ選定・アダプタ一式を差し替える。
    Reload {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// 1 アカウントだけ短い疎通確認を行う（タスク・イベントには残さない。ADR-0017 D2）。
    Check {
        provider_id: String,
        reply: oneshot::Sender<Result<ProviderCheckOutcome, CheckError>>,
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

/// ADR-0024 D6: `POST /accounts/{id}/check` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountCheckOutcome {
    pub result: ProviderCheckResult,
    pub detail: Option<String>,
    /// 観測できた `rate_limit_event`（あれば）。`AccountUsageView` に写す（`source = "check"`）。
    pub observation: Option<task_core::RateLimitObservation>,
}

/// ADR-0024 D7 / ADR-0025 D5: `POST /accounts/{id}/login` の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountLoginStartOutcome {
    pub url: String,
    /// Unix 秒（claude-code は 10 分後、codex は 15 分後）。
    pub expires_at_unix: i64,
    /// codex の一回限りのコード（claude-code は `None`。ログには出さない）。
    pub user_code: Option<String>,
}

/// ADR-0024 D7: `POST /accounts/{id}/login/code` の結果。
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

/// `check` が終わったときの結果（ADR-0022 M1 で `detail` を追加）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCheckOutcome {
    pub result: ProviderCheckResult,
    /// 人が読むための一行の手がかり（ワーカーの返答や失敗の理由）。
    pub detail: Option<String>,
}

/// `POST /api/v1/providers/{id}/check` の結果（ADR-0017 D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCheckResult {
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

/// `check` が完了できなかった理由（celeris 側の都合。ハンドラがこれを HTTP へ写す）。
#[derive(Debug, Clone)]
pub enum CheckError {
    /// 指定した id が現在の設定（`[[providers]]` + `providers.d/`）に無い。
    NotFound,
    /// 設定の再読込に失敗した（`Config::load` のエラー文言）。
    ConfigInvalid(String),
    /// 受け取り側（celeris）に届かなかった（チャネルが閉じている・タイムアウト）。
    Unavailable(String),
}

/// `providers.d/<id>.toml` の中身（`celeris::config::ProviderConfig` と同じ形。ADR-0017 M1）。
/// task-api は celeris に依存できない（循環依存になる）ので、独立に同じ形の型を持つ。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_source: Option<LlmSourceRef>,
    #[serde(default)]
    pub tier_models: task_core::model_routing::TierModels,
    #[serde(default)]
    pub account_id: Option<String>,
    pub id: String,
    pub adapter: String,
    #[serde(default = "default_tiers")]
    pub tiers: Vec<Tier>,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0030 D2: 環境変数名 → `[secrets]` の秘密 id。**管理 API はこのフィールドを読み書きしない**
    /// （`command`/`args`/`settings` と同じ理由・同じ扱い: `create`/`patch` の本文に来たら 422 で拒否する。
    /// 素通り用フィールドで、人が直接編集した `providers.d/<id>.toml` の値を PATCH の往復で消さない）。
    #[serde(default)]
    pub env_from_secrets: HashMap<String, String>,
    /// ADR-0024 D2: `[accounts]` のプールから選ぶ（`adapter = "claude-code"` かつ `[accounts]` があるときだけ有効）。
    ///
    /// ADR 2026-10-06 D2: `"opencode-go"` のように pool の adapter 名を文字列で書ける（人が書いた行を往復で保つ）。
    #[serde(default)]
    pub account_pool: task_core::AccountPoolSetting,
    /// ADR-0026 D7: `adapter = "acp"` のときだけ意味を持つ、ACP エージェントの実行ファイルの上書き。
    /// **管理 API はこのフィールドを読み書きしない**（`create`/`patch` の本文に来たら 422 で拒否する。
    /// `handlers::providers::reject_provider_command_and_args` を参照）。人が直接編集した `providers.d/<id>.toml` の値を
    /// `read_provider_file` → `write_provider_file` の往復（PATCH）で消さないための素通り用フィールド。
    #[serde(default)]
    pub command: Option<String>,
    /// 上と同じ（引数）。
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// ADR-0027 D3: `adapter = "paperqa"` のときだけ意味を持つ、PaperQA 設定ファイルの上書き。
    /// **管理 API はこのフィールドを読み書きしない**（`command`/`args` と同じ理由・同じ扱い）。
    #[serde(default)]
    pub settings: Option<String>,
}

impl ProviderConfigFile {
    pub fn to_view(&self) -> ProviderConfigView {
        let mut env_keys: Vec<String> = self.env.keys().cloned().collect();
        env_keys.sort();
        ProviderConfigView {
            kind: self.kind.unwrap_or_default(),
            llm_source: Some(self.resolved_llm_source()),
            credential_refs: task_core::model_routing::credential_refs(&self.env_from_secrets),
            tier_models: self.tier_models.clone(),
            account_id: self.account_id.clone(),
            id: self.id.clone(),
            adapter: self.adapter.clone(),
            tiers: self.tiers.clone(),
            concurrency: self.concurrency,
            model: (!self.model.is_empty()).then(|| self.model.clone()),
            env_keys,
            account_pool: self.account_pool.is_on(),
        }
    }

    pub(crate) fn resolved_llm_source(&self) -> ResolvedLlmSource {
        let derived = match self.adapter.as_str() {
            "fake" => LlmSourceRef::None,
            "claude-code" => LlmSourceRef::ClaudeOauth,
            "codex" => LlmSourceRef::CodexOauth,
            "acp"
                if self.model.starts_with("opencode-go/")
                    || self.account_pool
                        == task_core::AccountPoolSetting::Adapter(
                            task_core::AccountAdapter::OpencodeGo,
                        ) =>
            {
                LlmSourceRef::OpencodeGo
            }
            _ if matches!(
                self.model.as_str(),
                "celeris/frontier"
                    | "celeris/standard"
                    | "celeris/cheap"
                    | "openai/celeris/frontier"
                    | "openai/celeris/standard"
                    | "openai/celeris/cheap"
            ) =>
            {
                LlmSourceRef::Celeris
            }
            _ => LlmSourceRef::Unknown,
        };
        ResolvedLlmSource {
            source: self.llm_source.clone().unwrap_or(derived),
            origin: if self.llm_source.is_some() {
                SourceOrigin::Explicit
            } else {
                SourceOrigin::Derived
            },
        }
    }
}

fn default_tiers() -> Vec<Tier> {
    vec![Tier::Frontier, Tier::Standard, Tier::Cheap]
}

fn default_concurrency() -> usize {
    1
}

/// `POST /api/v1/providers` の本文。
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCreateBody {
    #[serde(default)]
    pub kind: Option<ProviderKind>,
    #[serde(default)]
    pub llm_source: Option<LlmSourceRef>,
    #[serde(default)]
    pub credential_refs: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub tier_models: task_core::model_routing::TierModels,
    #[serde(default)]
    pub account_id: Option<String>,
    pub id: String,
    pub adapter: String,
    #[serde(default)]
    pub tiers: Option<Vec<Tier>>,
    #[serde(default)]
    pub concurrency: Option<usize>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// ADR-0024 D2: 既定 `false`。`true` は `adapter = "claude-code"` かつ `[accounts]` があるときだけ有効。
    #[serde(default)]
    pub account_pool: bool,
}

impl ProviderCreateBody {
    pub fn into_file(self) -> ProviderConfigFile {
        ProviderConfigFile {
            kind: self.kind,
            llm_source: self.llm_source,
            tier_models: self.tier_models,
            account_id: self.account_id.clone(),
            id: self.id,
            adapter: self.adapter,
            tiers: self.tiers.unwrap_or_else(default_tiers),
            concurrency: self.concurrency.unwrap_or_else(default_concurrency),
            model: self.model.unwrap_or_default(),
            env: self.env,
            // ADR-0030 D2: 管理 API は env_from_secrets を書かない（`create` された行は必ず空。人が後から
            // ファイルへ足す）。
            env_from_secrets: self.credential_refs,
            account_pool: self.account_pool.into(),
            // ADR-0026 D7 / ADR-0027 D3: 管理 API は command/args/settings を書かない（`create` された行は
            // 必ず `None`。人が後からファイルへ足す）。
            command: None,
            args: None,
            settings: None,
        }
    }
}

/// `PATCH /api/v1/providers/{id}` の本文。`id`/`adapter` は変更できない（ADR-0017 D1: 並列度・tier・model。
/// `env` も自然な拡張として一緒に patch できるようにした）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProviderPatchBody {
    #[serde(default)]
    pub kind: Option<ProviderKind>,
    #[serde(default)]
    pub llm_source: Option<LlmSourceRef>,
    #[serde(default)]
    pub credential_refs: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub tier_models: Option<task_core::model_routing::TierModels>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub tiers: Option<Vec<Tier>>,
    #[serde(default)]
    pub concurrency: Option<usize>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// ADR-0024 D2: 渡したときだけ上書き。
    #[serde(default)]
    pub account_pool: Option<bool>,
}

impl ProviderPatchBody {
    pub fn apply(&self, mut file: ProviderConfigFile) -> ProviderConfigFile {
        if let Some(kind) = self.kind {
            file.kind = Some(kind);
        }
        if let Some(source) = &self.llm_source {
            file.llm_source = Some(source.clone());
        }
        if let Some(refs) = &self.credential_refs {
            for key in task_core::model_routing::CREDENTIAL_KEYS {
                file.env_from_secrets.remove(*key);
            }
            file.env_from_secrets.extend(refs.clone());
        }
        if let Some(id) = &self.account_id {
            file.account_id = (!id.is_empty()).then(|| id.clone());
        }
        if let Some(models) = &self.tier_models {
            file.tier_models = models.clone();
        }
        if let Some(tiers) = &self.tiers {
            file.tiers = tiers.clone();
        }
        if let Some(concurrency) = self.concurrency {
            file.concurrency = concurrency;
        }
        if let Some(model) = &self.model {
            file.model = model.clone();
        }
        if let Some(env) = &self.env {
            file.env = env.clone();
        }
        if let Some(account_pool) = self.account_pool {
            // 名前付き pool（"opencode-go"）の行を `true` で上書きしても名前は保つ。`false` は外す。
            file.account_pool = match (account_pool, file.account_pool) {
                (true, named @ task_core::AccountPoolSetting::Adapter(_)) => named,
                (on, _) => on.into(),
            };
        }
        file
    }
}

pub const KNOWN_ADAPTERS: [&str; 6] = [
    "fake",
    "claude-code",
    "codex",
    "acp",
    "paperqa",
    "local-deep-research",
];

/// ファイル名に安全に使える id か（`providers.d/<id>.toml` のパストラバーサル防止）。
pub fn valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn valid_adapter(adapter: &str) -> bool {
    KNOWN_ADAPTERS.contains(&adapter)
}

pub fn provider_file_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.toml"))
}

#[derive(Debug, thiserror::Error)]
pub enum AdminFileError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("failed to encode {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },
}

pub fn read_provider_file(path: &Path) -> Result<ProviderConfigFile, AdminFileError> {
    let text = std::fs::read_to_string(path).map_err(|source| AdminFileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| AdminFileError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

pub fn write_provider_file(
    dir: &Path,
    provider: &ProviderConfigFile,
) -> Result<(), AdminFileError> {
    std::fs::create_dir_all(dir).map_err(|source| AdminFileError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = provider_file_path(dir, &provider.id);
    let text = toml::to_string_pretty(provider).map_err(|source| AdminFileError::Encode {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, text).map_err(|source| AdminFileError::Write { path, source })
}

#[cfg(test)]
#[path = "admin/tests.rs"]
mod tests;

/// Copy legacy inline credentials into the existing account secret store before
/// atomically saving a provider reference. A failed write never removes the source.
/// Existing secret references keep their precedence; shadowed values are archived.
pub fn migrate_credentials(
    file: &mut ProviderConfigFile,
    dir: &Path,
) -> Result<(), crate::secrets::SecretFileError> {
    for key in task_core::model_routing::CREDENTIAL_KEYS {
        let Some(value) = file.env.get(*key) else {
            continue;
        };
        let id = format!("migrated-{}", ulid::Ulid::new());
        crate::secrets::write_secret_file(dir, &id, value)?;
        file.env_from_secrets.entry((*key).to_owned()).or_insert(id);
        file.env.remove(*key);
    }
    Ok(())
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    #[test]
    fn legacy_credentials_migrate_without_losing_settings_or_exposing_values() {
        let legacy = r#"
id = "gpt"
adapter = "codex"
model = "legacy-exact-id"
[env]
OPENAI_API_KEY = "secret-test-value"
CODEX_HOME = "/existing/account"
[env_from_secrets]
CUSTOM_ENV = "existing-reference"
"#;
        let mut file: ProviderConfigFile = toml::from_str(legacy).unwrap();
        let dir = tempfile::tempdir().unwrap();
        migrate_credentials(&mut file, dir.path()).unwrap();
        assert!(!file.env.contains_key("OPENAI_API_KEY"));
        assert_eq!(file.env["CODEX_HOME"], "/existing/account");
        assert_eq!(file.env_from_secrets["CUSTOM_ENV"], "existing-reference");
        let id = &file.env_from_secrets["OPENAI_API_KEY"];
        assert_eq!(
            std::fs::read_to_string(dir.path().join(id)).unwrap(),
            "secret-test-value"
        );
        let view = serde_json::to_string(&file.to_view()).unwrap();
        assert!(!view.contains("secret-test-value"));
        assert_eq!(file.model, "legacy-exact-id");
        assert!(file.tier_models.is_empty());
        let encoded = toml::to_string(&file).unwrap();
        let mut restored: ProviderConfigFile = toml::from_str(&encoded).unwrap();
        migrate_credentials(&mut restored, dir.path()).unwrap();
        assert_eq!(restored.env_from_secrets, file.env_from_secrets);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join(id))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn failed_migration_retains_source_and_tier_patch_preserves_authentication() {
        let mut file: ProviderConfigFile = toml::from_str(
            "id='claude'\nadapter='claude-code'\n[env]\nANTHROPIC_API_KEY='private'",
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let invalid = dir.path().join("file");
        std::fs::write(&invalid, "not a directory").unwrap();
        assert!(migrate_credentials(&mut file, &invalid).is_err());
        assert_eq!(file.env["ANTHROPIC_API_KEY"], "private");
        let patch: ProviderPatchBody = serde_json::from_value(serde_json::json!({"tier_models":{"frontier":{"name":"fable","model_id":null,"unavailable_reason":"unverified"}}})).unwrap();
        let file = patch.apply(file);
        assert_eq!(file.env["ANTHROPIC_API_KEY"], "private");
        assert!(
            task_core::model_routing::resolve(&file.tier_models, Tier::Frontier)
                .unwrap_err()
                .contains("unverified")
        );
    }
}
