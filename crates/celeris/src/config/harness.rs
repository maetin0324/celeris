//! `[[harnesses]]`（ADR-0046 D3）と互換の `[[roles]]`・`[[genres]]`・`[conversation]`、検証の煙試験（ADR-0041 D5）。

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use task_core::{CONVERSATION_GENRE, HarnessBudget, HarnessRegistry, HarnessSpec, RoleSpec, Tier};

use super::{Config, ConfigError, ProviderConfig};

/// ADR-0041 D5（Phase 51）: 検証（`--mode verify`）の煙試験が使う組み込みの id。
/// 役割・分野・プロバイダで同じ名前を使う（`Config::apply_verify_smoke` が足す）。
pub const SMOKE_ID: &str = "smoke";
/// 組み込みの分野 `smoke` の説明（ADR-0041 D5）。
pub const SMOKE_DESCRIPTION: &str = "検証の煙試験";
/// 煙試験の予算（小さく。偽のアダプタは 1 往復で終わる）。
pub const SMOKE_MAX_TURNS: u32 = 1;
/// 同上（壁時計）。
pub const SMOKE_MAX_WALL_SECS: u64 = 60;
/// 組み込みの役割 `smoke` の指示文。
pub const SMOKE_INSTRUCTIONS: &str =
    "検証（staging）の煙試験。偽のアダプタが 1 往復するだけで、外に出る操作は何もしない。";

/// ADR-0046 D3: TOML の基本文字列（`"` と `\\` と改行だけを逃がす。決定的）。
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// ADR-0046 D3: `Tier` の TOML の綴り。
fn tier_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Frontier => "frontier",
        Tier::Standard => "standard",
        Tier::Cheap => "cheap",
    }
}

/// `[[harnesses]] budget`（ADR-0046 D3）。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessBudgetConfig {
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub max_wall_secs: Option<u64>,
    #[serde(default)]
    pub max_retries: Option<u32>,
}

/// `[[harnesses]]`（ADR-0046 D3）: 実行契約。今までの `[[genres]]`（能力・入出力の契約・対話用か）と
/// `[[roles]]`（adapter・tier・指示文・予算）を 1 つにしたもの。**組織と 1 対 1 にしない**。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    /// タスクの `genre` 列がそのまま指す id（例 `"coding"` / `"literature"`）。
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// 省略時は tier だけで選ぶ（fake / claude-code / codex / acp / paperqa / local-deep-research）。
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub tier: Option<Tier>,
    /// ワーカーのプロンプトに前置きする指示文。`GET /config` には**出さない**。
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub input_artifacts: Vec<String>,
    #[serde(default)]
    pub output_artifacts: Vec<String>,
    #[serde(default)]
    pub budget: HarnessBudgetConfig,
    /// 対話用のハーネスか（今までの「対話用分野」）。
    #[serde(default)]
    pub conversation: bool,
    /// ADR-0052 D2（Phase 64）: 専用アダプタに届かないときに倒す先（`fallback = { tier = "cheap" }`）。
    /// `fallback = false` で無効。省略すると組み込みの既定を継ぐ（`knowledge` は tier `cheap`）。
    #[serde(default)]
    pub fallback: Option<task_core::HarnessFallback>,
}

impl HarnessConfig {
    /// task-core の型に写す（設定の順）。
    pub fn to_spec(&self) -> HarnessSpec {
        HarnessSpec {
            id: self.id.clone(),
            description: self.description.clone(),
            adapter: self.adapter.clone(),
            fallback: self.fallback.clone(),
            tier: self.tier,
            instructions: self.instructions.clone(),
            capabilities: self.capabilities.clone(),
            input_artifacts: self.input_artifacts.clone(),
            output_artifacts: self.output_artifacts.clone(),
            budget: HarnessBudget {
                max_turns: self.budget.max_turns,
                max_wall_secs: self.budget.max_wall_secs,
                max_retries: self.budget.max_retries,
            },
            conversation: self.conversation,
        }
    }
}

/// `[[roles]]`（ADR-0016 D1）: 役割ごとの既定。タスクに書かれた値 > ここの既定 > 全体の既定の順に効く。
/// `id` は自由記述で、ここに無い役割名をタスクに付けてもよい（既定も指示文も無いだけ）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleConfig {
    /// タスクの `role` が指す名前（例 `"lead"` / `"implementer"` / `"reviewer"`）。
    pub id: String,
    #[serde(default)]
    pub tier: Option<Tier>,
    /// 省略時は tier だけで選ぶ（fake / claude-code / codex）。
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub max_wall_secs: Option<u64>,
    /// ワーカーのプロンプトに前置きする指示文（何を任され、何を任せてよいか）。`GET /config` には**出さない**。
    #[serde(default)]
    pub instructions: Option<String>,
}

/// `[[genres]]`（ADR-0027 D1）: 分野の説明・既定の役割・分野に属する役割の一覧。分野そのものにはアダプタを
/// 持たせない（D2: `default_role` が指す役割が持つ）。`id` は重複させない。`default_role` と `roles` の各要素は
/// `[[roles]]` に存在すること、`default_role`（あれば）は `roles` に含まれることを `Config::validate` が確認する。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenreConfig {
    /// タスクの `genre` が指す名前（例 `"coding"` / `"literature"`）。
    pub id: String,
    /// プロンプトに入れる分野の説明（ADR-0027 D1）。
    pub description: String,
    /// ADR-0028 D1: この分野で「できること」の自由記述（固定 enum にしない）。省略時は空。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// ADR-0028 D1: この分野に投げるときに用意すべきものの目安（自由記述。celeris は中身を検査しない）。
    #[serde(default)]
    pub input_artifacts: Vec<String>,
    /// ADR-0028 D1: この分野から戻ってくるものの目安（自由記述）。
    #[serde(default)]
    pub output_artifacts: Vec<String>,
    /// タスクに `role` が無いときに、この分野の既定として使う役割 id。`roles` に含まれること。
    #[serde(default)]
    pub default_role: Option<String>,
    /// この分野に属する役割 id の一覧。`genre` と `role` を両方指定したタスクは、`role` がここに無ければ設定エラー。
    #[serde(default)]
    pub roles: Vec<String>,
}

/// `[conversation]`（Phase 30 / ADR-0033 D4 追記）: 対話が常に走る分野。書けば `[[genres]]` に存在する
/// こと（`Config::validate` が確認する）。書かなければ既定は `task_core::CONVERSATION_GENRE`
/// （`Config::conversation_genre_id` が返す）で、`[[genres]]` の中身は検証しない
/// （`[[genres]]` を使わない最小構成を壊さないため）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationConfig {
    /// タスクの `genre` が指す名前と同じ形。`[[genres]] id`。
    #[serde(default = "default_conversation_genre")]
    pub genre: String,
}

fn default_conversation_genre() -> String {
    CONVERSATION_GENRE.to_string()
}

impl Config {
    /// ADR-0041 D5（Phase 51）: `--mode verify` の煙試験に要るものを**組み込みで**足す。
    ///
    /// `Config::load` の後（`apply_overrides` の後）に、**verify モードのときだけ** celeris が呼ぶ。
    /// 設定ファイルに同じ id があっても**上書きする**（本番の設定に `smoke` という名前の役割や分野が
    /// あっても、検証の煙試験は必ず偽のアダプタで 1 往復するだけのものになる）。
    ///
    /// 足すもの:
    /// - `[[providers]] id = "smoke" adapter = "fake" tiers = ["standard"]`
    ///   （本番の設定には `fake` のプロバイダが無いので、これが無いと煙試験を起こせない）
    /// - `[[roles]] id = "smoke" adapter = "fake" tier = "standard"`（小さい予算）
    /// - `[[genres]] id = "smoke" description = "検証の煙試験" default_role = "smoke" roles = ["smoke"]`
    /// - `[adapters.fake].command` を `FakeAdapter::default_command()` に固定し、`[reviewer]` も
    ///   `fake` / `standard` にする（ADR-0041 §3「煙試験で本物の LLM を呼ばない。`fake` だけ」を、
    ///   指示文ではなく設定の形で守る）
    pub fn apply_verify_smoke(&mut self) {
        use task_worker::FakeAdapter;

        // 偽のアダプタは既定のコマンドに固定する（設定の `[adapters.fake]` に左右されない）。
        self.adapters.fake.command = FakeAdapter::default_command();
        // レビューも偽のアダプタだけ（`Check::Reviewer` を持つ煙試験を書いても LLM は呼ばれない）。
        self.reviewer.adapter = Some(FakeAdapter::ID.to_string());
        self.reviewer.tier = Some(Tier::Standard);

        self.providers.retain(|p| p.id != SMOKE_ID);
        self.providers.push(ProviderConfig {
            kind: None,
            llm_source: None,
            tier_models: Default::default(),
            account_id: None,
            id: SMOKE_ID.to_string(),
            adapter: FakeAdapter::ID.to_string(),
            tiers: vec![Tier::Standard],
            concurrency: 1,
            model: FakeAdapter::ID.to_string(),
            env: HashMap::new(),
            env_from_secrets: HashMap::new(),
            account_pool: false,
            command: None,
            args: None,
            settings: None,
        });
        self.roles.retain(|r| r.id != SMOKE_ID);
        self.roles.push(RoleConfig {
            id: SMOKE_ID.to_string(),
            tier: Some(Tier::Standard),
            adapter: Some(FakeAdapter::ID.to_string()),
            max_turns: Some(SMOKE_MAX_TURNS),
            max_wall_secs: Some(SMOKE_MAX_WALL_SECS),
            instructions: Some(SMOKE_INSTRUCTIONS.to_string()),
        });
        self.genres.retain(|g| g.id != SMOKE_ID);
        self.genres.push(GenreConfig {
            id: SMOKE_ID.to_string(),
            description: SMOKE_DESCRIPTION.to_string(),
            capabilities: vec![],
            input_artifacts: vec![],
            output_artifacts: vec![],
            default_role: Some(SMOKE_ID.to_string()),
            roles: vec![SMOKE_ID.to_string()],
        });
    }

    /// ADR-0016 D1: `[[roles]]` を task-core の型に写す（設定の順）。
    pub fn role_specs(&self) -> Vec<RoleSpec> {
        self.roles
            .iter()
            .map(|r| RoleSpec {
                id: r.id.clone(),
                tier: r.tier,
                adapter: r.adapter.clone(),
                max_turns: r.max_turns,
                max_wall_secs: r.max_wall_secs,
                instructions: r.instructions.clone(),
            })
            .collect()
    }

    /// ADR-0027 D1: `[[genres]]` を task-core の型に写す（設定の順）。
    pub fn genre_specs(&self) -> Vec<task_core::GenreSpec> {
        self.genres
            .iter()
            .map(|g| task_core::GenreSpec {
                id: g.id.clone(),
                description: g.description.clone(),
                capabilities: g.capabilities.clone(),
                input_artifacts: g.input_artifacts.clone(),
                output_artifacts: g.output_artifacts.clone(),
                default_role: g.default_role.clone(),
                roles: g.roles.clone(),
            })
            .collect()
    }

    /// ADR-0046 D3（Phase 59）: ハーネスのレジストリ（`HarnessRegistry::get(id)` が唯一の引き方）。
    ///
    /// `[[harnesses]]` があればそれが正。無ければ旧い `[[genres]]` + `[[roles]]` を決定的に写す
    /// （互換の読み込み）。どちらの場合も組み込み（conversation / plan / reviewer / smoke）が足される。
    pub fn harness_registry(&self) -> HarnessRegistry {
        if !self.harnesses.is_empty() {
            return HarnessRegistry::new(
                self.harnesses.iter().map(HarnessConfig::to_spec).collect(),
            );
        }
        let (registry, _dropped) = HarnessRegistry::from_legacy(
            &self.genre_specs(),
            &self.role_specs(),
            self.conversation_genre_id(),
        );
        registry
    }

    /// ADR-0046 D3: 旧い設定を写したときに「写さなかった役割」（どの分野の `default_role` でもなく、
    /// 同名のハーネスも無い役割）の id。`celerisctl config to-harnesses` が注意書きに出す。
    pub fn legacy_dropped_roles(&self) -> Vec<String> {
        if !self.harnesses.is_empty() {
            return Vec::new();
        }
        let (_, dropped) = HarnessRegistry::from_legacy(
            &self.genre_specs(),
            &self.role_specs(),
            self.conversation_genre_id(),
        );
        dropped
    }

    /// ADR-0046 D3（Phase 59）: `[[harnesses]]` を書いた設定を、既存の経路（`genre` / `role` を見る
    /// ディスパッチャ・task-ops）がそのまま使えるように `genres` / `roles` へ射影する。
    ///
    /// 射影するのは**設定に書かれたハーネスだけ**（組み込みの `plan` / `reviewer` / `smoke` は
    /// タスクの `genre` として使わないので、計画 run の「使える分野」に混ぜない）。ただし
    /// `[conversation] genre` が組み込みを指しているときは、その 1 件だけ足す（対話が指示文を失わないように）。
    /// `[[harnesses]]` が無い設定では**何もしない**（Phase 58 までと 1 バイトも変わらない）。
    pub fn project_harnesses(&mut self) {
        if self.harnesses.is_empty() {
            return;
        }
        let registry = self.harness_registry();
        let mut specs: Vec<HarnessSpec> =
            self.harnesses.iter().map(HarnessConfig::to_spec).collect();
        let conversation = self.conversation_genre_id().to_string();
        if !specs.iter().any(|h| h.id == conversation)
            && let Some(builtin) = registry.get(&conversation)
        {
            specs.push(builtin.clone());
        }
        self.genres = specs
            .iter()
            .map(|h| {
                let g = h.genre_spec();
                GenreConfig {
                    id: g.id,
                    description: g.description,
                    capabilities: g.capabilities,
                    input_artifacts: g.input_artifacts,
                    output_artifacts: g.output_artifacts,
                    default_role: g.default_role,
                    roles: g.roles,
                }
            })
            .collect();
        self.roles = specs
            .iter()
            .map(|h| {
                let r = h.role_spec();
                RoleConfig {
                    id: r.id,
                    tier: r.tier,
                    adapter: r.adapter,
                    max_turns: r.max_turns,
                    max_wall_secs: r.max_wall_secs,
                    instructions: r.instructions,
                }
            })
            .collect();
    }

    /// ADR-0046 D3（Phase 59）: `celerisctl config to-harnesses` の出力。旧い `[[genres]]` + `[[roles]]`
    /// を `[[harnesses]]` の形に書き出す（人がこれで設定を差し替える）。決定的（LLM は使わない）。
    ///
    /// ADR-0046 D6 の改名に合わせて、旧い対話用分野の id が `secretary` のときは **`conversation`**
    /// という id で書き出し、`[conversation] genre = "conversation"` も一緒に出す。
    pub fn to_harnesses_toml(&self) -> String {
        let registry = self.harness_registry();
        let declared: Vec<&HarnessSpec> = if self.harnesses.is_empty() {
            // 旧い設定から写したもののうち、**設定に由来するもの**だけを書き出す
            // （組み込みだけのハーネスは書き出さない。設定に同じ id があれば書き出す）。
            registry
                .all()
                .iter()
                .filter(|h| {
                    self.genres.iter().any(|g| g.id == h.id)
                        || self.roles.iter().any(|r| r.id == h.id)
                })
                .collect()
        } else {
            registry
                .all()
                .iter()
                .filter(|h| self.harnesses.iter().any(|c| c.id == h.id))
                .collect()
        };
        let legacy_conversation = self.conversation_genre_id().to_string();
        let rename_conversation = legacy_conversation == CONVERSATION_GENRE;
        let mut out = String::new();
        out.push_str(
            "# ADR-0046 D3: `[[genres]]` + `[[roles]]` を `[[harnesses]]` に写したもの
",
        );
        out.push_str(
            "# （`celerisctl config to-harnesses` が生成。決定的で、LLM は使っていない）。
",
        );
        out.push_str(
            "#
",
        );
        out.push_str(
            "# 使い方: 下の `[[harnesses]]` と `[conversation]` を config.toml に貼り、
",
        );
        out.push_str("#   **既存の `[[genres]]` と `[[roles]]` の節を全部消す**（両方あると `[[harnesses]]` が勝つ）。
");
        let dropped = self.legacy_dropped_roles();
        if !dropped.is_empty() {
            out.push_str(&format!(
                "#
# 写せなかった役割（どの分野の `default_role` でもなく、同名のハーネスも無い）: {}
                 #   これらは `tasks.role` の互換としてしか使われない。必要なら手で `[[harnesses]]` に足すこと。
",
                dropped.join(", ")
            ));
        }
        if rename_conversation {
            out.push_str(&format!(
                "#
# ADR-0046 D6: 対話用のハーネスは `{CONVERSATION_GENRE}` から `conversation` に改名した
                 #   （根ノードも `secretary` → `cos`）。下の `[conversation]` も一緒に貼ること。
"
            ));
        }
        for h in declared {
            let id = if rename_conversation && h.id == legacy_conversation {
                task_core::BUILTIN_CONVERSATION.to_string()
            } else {
                h.id.clone()
            };
            out.push_str(
                "
[[harnesses]]
",
            );
            out.push_str(&format!(
                "id = {}
",
                toml_string(&id)
            ));
            out.push_str(&format!(
                "description = {}
",
                toml_string(&h.description)
            ));
            if let Some(adapter) = &h.adapter {
                out.push_str(&format!(
                    "adapter = {}
",
                    toml_string(adapter)
                ));
            }
            if let Some(tier) = h.tier {
                out.push_str(&format!(
                    "tier = {}
",
                    toml_string(tier_str(tier))
                ));
            }
            if h.conversation {
                out.push_str(
                    "conversation = true
",
                );
            }
            for (name, list) in [
                ("capabilities", &h.capabilities),
                ("input_artifacts", &h.input_artifacts),
                ("output_artifacts", &h.output_artifacts),
            ] {
                if list.is_empty() {
                    continue;
                }
                let items: Vec<String> = list.iter().map(|v| toml_string(v)).collect();
                out.push_str(&format!(
                    "{name} = [{}]
",
                    items.join(", ")
                ));
            }
            if !h.budget.is_empty() {
                let mut parts: Vec<String> = Vec::new();
                if let Some(v) = h.budget.max_turns {
                    parts.push(format!("max_turns = {v}"));
                }
                if let Some(v) = h.budget.max_wall_secs {
                    parts.push(format!("max_wall_secs = {v}"));
                }
                if let Some(v) = h.budget.max_retries {
                    parts.push(format!("max_retries = {v}"));
                }
                out.push_str(&format!(
                    "budget = {{ {} }}
",
                    parts.join(", ")
                ));
            }
            if let Some(instructions) = &h.instructions {
                out.push_str(&format!(
                    "instructions = {}
",
                    toml_string(instructions)
                ));
            }
        }
        let conversation_id = if rename_conversation {
            task_core::BUILTIN_CONVERSATION
        } else {
            legacy_conversation.as_str()
        };
        out.push_str(&format!(
            "
[conversation]
genre = {}
",
            toml_string(conversation_id)
        ));
        out
    }

    /// Phase 30（ADR-0033 D4 追記）: 対話が常に走る分野の id。`[conversation] genre`、省略時は
    /// `task_core::CONVERSATION_GENRE`（`"secretary"`）。
    pub fn conversation_genre_id(&self) -> &str {
        self.conversation
            .as_ref()
            .map(|c| c.genre.as_str())
            .unwrap_or(CONVERSATION_GENRE)
    }
}

impl Config {
    /// ADR-0046 D3（Phase 59）: `[[harnesses]]` があれば `genres` / `roles` に射影してから検証する
    /// （既存の経路は `genre` / `role` のまま動く）。無ければ旧い形のまま検証し、warn を 1 行出す。
    pub(super) fn merge_harnesses(&mut self) {
        if self.harnesses.is_empty() {
            if !self.genres.is_empty() || !self.roles.is_empty() {
                tracing::warn!(
                    genres = self.genres.len(),
                    roles = self.roles.len(),
                    "config: [[genres]] + [[roles]] は ADR-0046 D3 で [[harnesses]] に置き換わった。                     互換で読み込んだ。`celerisctl config to-harnesses --config <this file>` で新しい形を書き出せる"
                );
            }
        } else {
            self.project_harnesses();
        }
    }
}

/// ADR-0046 D3（Phase 59）: ハーネスの id は重複させない。adapter は providers と同じ判定。
pub(super) fn validate_harnesses(harnesses: &[HarnessConfig]) -> Result<(), ConfigError> {
    let mut harness_ids = std::collections::HashSet::new();
    for h in harnesses {
        if h.id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[[harnesses]] id must not be empty".to_string(),
            ));
        }
        if !harness_ids.insert(&h.id) {
            return Err(ConfigError::Invalid(format!(
                "duplicate harness id: {}",
                h.id
            )));
        }
        if let Some(adapter) = &h.adapter
            && adapter != task_worker::FakeAdapter::ID
            && adapter != task_worker::ClaudeCodeAdapter::ID
            && adapter != task_worker::CodexAdapter::ID
            && adapter != task_worker::AcpAdapter::ID
            && adapter != task_worker::BrowserSpecialistAdapter::ID
            && adapter != task_worker::PaperQaAdapter::ID
            && adapter != task_worker::LdrAdapter::ID
            && adapter != task_worker::LangMemAdapter::ID
        {
            return Err(ConfigError::Invalid(format!(
                "[[harnesses]] {}: adapter {adapter:?} is not available in this build (fake, claude-code, codex, acp, browser-specialist, paperqa, local-deep-research, langmem only)",
                h.id
            )));
        }
        if h.budget.max_turns == Some(0) {
            return Err(ConfigError::Invalid(format!(
                "[[harnesses]] {}: max_turns must be >= 1",
                h.id
            )));
        }
        if h.budget.max_wall_secs == Some(0) {
            return Err(ConfigError::Invalid(format!(
                "[[harnesses]] {}: max_wall_secs must be >= 1",
                h.id
            )));
        }
    }
    Ok(())
}

/// ADR-0016 D1: 役割の id は重複させない。adapter は providers と同じ判定。上限は 1 以上。
/// 返り値は定義された役割の id（`[[genres]]` の検証が使う）。
pub(super) fn validate_roles(roles: &[RoleConfig]) -> Result<HashSet<&String>, ConfigError> {
    let mut role_ids = std::collections::HashSet::new();
    for r in roles {
        if r.id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[[roles]] id must not be empty".to_string(),
            ));
        }
        if !role_ids.insert(&r.id) {
            return Err(ConfigError::Invalid(format!("duplicate role id: {}", r.id)));
        }
        if let Some(adapter) = &r.adapter
            && adapter != task_worker::FakeAdapter::ID
            && adapter != task_worker::ClaudeCodeAdapter::ID
            && adapter != task_worker::CodexAdapter::ID
            && adapter != task_worker::AcpAdapter::ID
            && adapter != task_worker::BrowserSpecialistAdapter::ID
            && adapter != task_worker::PaperQaAdapter::ID
            && adapter != task_worker::LdrAdapter::ID
            && adapter != task_worker::LangMemAdapter::ID
        {
            return Err(ConfigError::Invalid(format!(
                "[[roles]] {}: adapter {adapter:?} is not available in this build (fake, claude-code, codex, acp, browser-specialist, paperqa, local-deep-research, langmem only)",
                r.id
            )));
        }
        if r.max_turns == Some(0) {
            return Err(ConfigError::Invalid(format!(
                "[[roles]] {}: max_turns must be >= 1",
                r.id
            )));
        }
        if r.max_wall_secs == Some(0) {
            return Err(ConfigError::Invalid(format!(
                "[[roles]] {}: max_wall_secs must be >= 1",
                r.id
            )));
        }
    }
    Ok(role_ids)
}

/// ADR-0027 D1: 分野の id は重複させない。`default_role` と `roles` の各要素は `[[roles]]` に存在すること、
/// `default_role`（あれば）は `roles` に含まれること。
/// 返り値は定義された分野の id（`[conversation]` と `[[org]]` の検証が使う）。
pub(super) fn validate_genres<'a>(
    genres: &'a [GenreConfig],
    role_ids: &HashSet<&String>,
) -> Result<HashSet<&'a String>, ConfigError> {
    let mut genre_ids = std::collections::HashSet::new();
    for g in genres {
        if g.id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "[[genres]] id must not be empty".to_string(),
            ));
        }
        if !genre_ids.insert(&g.id) {
            return Err(ConfigError::Invalid(format!(
                "duplicate genre id: {}",
                g.id
            )));
        }
        for role_id in &g.roles {
            if !role_ids.contains(role_id) {
                return Err(ConfigError::Invalid(format!(
                    "[[genres]] {}: role {role_id:?} in roles is not defined in [[roles]]",
                    g.id
                )));
            }
        }
        if let Some(default_role) = &g.default_role {
            if !role_ids.contains(default_role) {
                return Err(ConfigError::Invalid(format!(
                    "[[genres]] {}: default_role {default_role:?} is not defined in [[roles]]",
                    g.id
                )));
            }
            if !g.roles.iter().any(|r| r == default_role) {
                return Err(ConfigError::Invalid(format!(
                    "[[genres]] {}: default_role {default_role:?} must be included in roles",
                    g.id
                )));
            }
        }
    }
    Ok(genre_ids)
}

/// Phase 30（ADR-0033 D4 追記）: `[conversation]` を明示したのに、その分野が `[[genres]]` に
/// 無ければ設定エラー（対話用の分野が無い）。省略時の既定（`CONVERSATION_GENRE`）は、
/// `[[genres]]` を使わない最小構成を壊さないよう、ここでは検証しない
/// （`conversation_genre_id()` の呼び出し側が `GenreSpec::find` で見つからなければ既定の
/// 役割で走るだけで、実害は無い）。
pub(super) fn validate_conversation(
    conversation: Option<&ConversationConfig>,
    genre_ids: &HashSet<&String>,
) -> Result<(), ConfigError> {
    if let Some(conversation) = conversation
        && !genre_ids.contains(&conversation.genre)
    {
        return Err(ConfigError::Invalid(format!(
            "[conversation]: genre {:?} is not defined in [[genres]] (対話用の分野が無い)",
            conversation.genre
        )));
    }
    Ok(())
}
