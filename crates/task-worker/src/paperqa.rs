//! `paperqa` アダプタ（DESIGN §5.4, ADR-0027 D3、ADR-0035 で「取得」の段を追加、
//! ADR-0063 Phase 109d C1/C2 で `pqa ask` CLI を PaperQA の Python API に置き換え）。
//!
//! PaperQA2 は celeris のワーカープロトコルもストリーム型の進捗形式も話さない、ただの調査エンジンで
//! ある。**アダプタ自身が** ADR-0006 D3 の結果ファイル規約（`artifacts/result.json`）を代わりに書き、
//! `Terminal::Done`/`Terminal::Error` を合成する。委譲（`delegate.json`）は扱わない
//! （ADR-0027 D3: 「委譲はしない」）。生存監視（wall-clock・無出力タイムアウト・SIGTERM→SIGKILL）は
//! `subprocess.rs` の低レベル部分を再利用する。
//!
//! ADR-0035: 1 run は **2 段**になった。
//!
//! 1. **取得** — 埋め込みの Python ランナー（`paperqa_acquire.py`）を `runs/<run_id>/` に書き出して起動し、
//!    arXiv / OpenAlex（鍵無し）で候補論文を集め、open access の PDF を**案件ごとの** corpus
//!    （`paper_directory/<project_id>/`）に落とす。**検索語はランナーの最初の段で LLM が立てる**
//!    （ADR-0035 D5 / Phase 36。PaperQA2 と同じ LLM 先に `chat/completions` を 1 回。答えが壊れて
//!    いれば、このアダプタが `objective` から決定的に抜いた語（`build_search_queries`）に落ちる）。
//! 2. **索引と回答** — ADR-0063 Phase 109d C1: `pqa ask` CLI ではなく、もう 1 つの埋め込み Python
//!    ランナー（`paperqa_ask.py`）が `paperqa.ask`/`Settings.from_name` を呼ぶ。対象（目的文から
//!    決定的に抜いた `research_targets`）が取れていれば対象ごと + 総括の複数回の `ask()`、取れなければ
//!    従来の単一の問い。`paper_directory` / `index_directory` / 索引名は案件ごと。
//!
//! 回答の後に、`ask()` が実際に使った証拠（`PQASession.contexts` の `docname`/`dockey`）と答えの本文
//! （文字列一致、補助）の和で `cited` を決定的に突き合わせ、`artifacts/sources.json` を書き直し、
//! `artifacts/answer.md`（対象が取れていれば対象別の表 + 節、引用された文献の一覧）の末尾に
//! `## 出典` を足し、証拠ゲート（ADR-0035 D3）で候補数・PDF 数・引用数を機械的に判定する。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use task_core::Task;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tracing::warn;

use crate::adapter::{AdapterError, EventSink, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use crate::progress;
use crate::protocol::{Answer, RunContext, RunRequest};
use crate::provider::classify_provider_failure;
use crate::subprocess::{
    LineOutcome, MAX_LINE_BYTES, kill_now, read_line_limited, read_tail, reap_after_terminal,
    write_result_json,
};

mod render;
pub use render::answer_cites;
use render::*;

/// run ごとに `runs/<run_id>/paperqa_acquire.py` として書き出す取得ランナー（ADR-0035 D1）。
const ACQUIRE_SCRIPT: &str = include_str!("paperqa_acquire.py");
/// 取得ランナーの最終行の目印（ADR-0035 D1）。
const ACQUIRE_RESULT_PREFIX: &str = "CELERIS_ACQUIRE ";
/// 取得ランナーの進捗行の目印。
const PROGRESS_PREFIX: &str = "progress:";
/// run ごとに `runs/<run_id>/paperqa_ask.py` として書き出す、PaperQA の Python API を呼ぶランナー
/// （ADR-0063 Phase 109d C1。`pqa ask` CLI の置き換え）。
const ASK_SCRIPT: &str = include_str!("paperqa_ask.py");
/// `paperqa_ask.py` が `paperqa` を import できなかったときの exit code（ADR-0063 Phase 109d C1）。
const ASK_EXIT_PAPERQA_NOT_IMPORTABLE: i32 = 3;
/// `paperqa_ask.py` の引数の誤り、または `settings_path`/`Settings.from_name` のどちらでも設定が
/// 見つからなかったときの exit code（ADR-0063 Phase 109e）。どちらも設定の誤りで再試行しても
/// 直らないので `retryable: false` にする。
const ASK_EXIT_BAD_USAGE_OR_SETTINGS_NOT_FOUND: i32 = 2;
/// `artifacts/result.json` の `summary` の上限（ADR-0027 D3）。
const SUMMARY_MAX_CHARS: usize = 1500;
/// `progress` に転送する 1 行あたりの上限（他アダプタと同じ規則。ADR-0026 の `truncate` を踏襲）。
const PROGRESS_LINE_MAX_CHARS: usize = 500;
/// 案件（`project_id`）が無いタスクの corpus / 索引の名前（ADR-0035 D1）。
const SHARED_PROJECT_KEY: &str = "_shared";
/// `objective` から作る検索語の本数の上限（ADR-0035 D1: 「2〜4 本」）。
const MAX_SEARCH_QUERIES: usize = 4;
/// LLM に立てさせる検索語の本数の上限（ADR-0035 D5: 「3〜6 本」）。
const MAX_LLM_SEARCH_QUERIES: u32 = 6;
/// 検索語を立てる `chat/completions` の `max_tokens`（ADR-0035 D5）。
const QUERY_LLM_MAX_TOKENS: u32 = 2000;
/// 検索語を立てる LLM に渡す案件の文脈の上限（字数）。
const QUERY_CONTEXT_MAX_CHARS: usize = 2000;

/// `[adapters.paperqa.acquire]`（ADR-0035 D1）: 文献の取得の設定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquireConfig {
    /// 取得ランナーを動かす python（標準ライブラリしか使わないので任意の python3 でよい）。
    /// 未指定なら `command`（`pqa`）と同じディレクトリの `python3`、`command` にディレクトリが
    /// 無ければ `python3`。
    #[serde(default)]
    pub command: Option<String>,
    /// 重複排除後に残す候補の上限。**`0` なら取得の段そのものを行わない**（従来どおり手元の corpus
    /// だけで答える。このとき証拠ゲート（`evidence`）も見ない）。
    #[serde(default = "default_max_candidates")]
    pub max_candidates: u32,
    /// 1 run で corpus に入れる PDF の上限。
    #[serde(default = "default_max_pdfs")]
    pub max_pdfs: u32,
    /// 検索語 1 本・エンジン 1 つあたりの取得件数。
    #[serde(default = "default_per_query")]
    pub per_query: u32,
    /// 1 回の HTTP 要求のタイムアウト（秒）。
    #[serde(default = "default_acquire_timeout_secs")]
    pub timeout_secs: u64,
    /// OpenAlex の polite pool に付ける連絡先（`mailto=`）。未指定なら付けない。
    #[serde(default)]
    pub mailto: Option<String>,
    /// ADR-0035 D5（Phase 36）: **検索語を LLM に立てさせる**（既定 `true`）。`false` にすると
    /// Phase 34 までの決定的な抽出（`build_search_queries`）だけを使う。`true` でも LLM の答えが
    /// 壊れていれば決定的な抽出に落ちる（ランナーが `progress:` に出す）。
    #[serde(default = "default_query_llm")]
    pub query_llm: bool,
    /// 検索語を立てる LLM のモデル名。未指定なら `[[providers]] model`（`--llm`）→ PaperQA の
    /// `settings` の `llm` の順に使う（**PaperQA2 と同じ LLM 先**。ADR-0035 D5）。
    /// LiteLLM の `provider/model` 形式の接頭辞（`openai/`）は落として渡す。
    #[serde(default)]
    pub query_model: Option<String>,
    /// 検索語を立てる 1 回の `chat/completions` のタイムアウト（秒）。
    #[serde(default = "default_query_timeout_secs")]
    pub query_timeout_secs: u64,
    /// OpenAlex の `filter=`。未指定ならランナーの既定
    /// `is_oa:true,primary_topic.field.id:17`（open access かつ Computer Science。ADR-0035 D5）。
    /// 計算機科学以外の分野で使うときだけ書き換える。
    #[serde(default)]
    pub openalex_filter: Option<String>,
    /// ADR-0063 D1（Phase 109）: 本文（PDF）が取れない候補は**アブストラクトで妥協する**（既定 `true`）。
    /// OpenAlex / arXiv / Unpaywall / Semantic Scholar のメタデータにある abstract をテキスト文書として
    /// corpus に入れる（`<key>_abstract.txt`。本文ではないことを明記した注記付き）。`false` にすると
    /// Phase 108 までどおり、PDF が無い候補は corpus に入らない。
    #[serde(default = "default_abstract_fallback")]
    pub abstract_fallback: bool,
}

impl Default for AcquireConfig {
    fn default() -> Self {
        Self {
            command: None,
            max_candidates: default_max_candidates(),
            max_pdfs: default_max_pdfs(),
            per_query: default_per_query(),
            timeout_secs: default_acquire_timeout_secs(),
            mailto: None,
            query_llm: default_query_llm(),
            query_model: None,
            query_timeout_secs: default_query_timeout_secs(),
            openalex_filter: None,
            abstract_fallback: default_abstract_fallback(),
        }
    }
}

fn default_max_candidates() -> u32 {
    30
}
fn default_max_pdfs() -> u32 {
    12
}
fn default_per_query() -> u32 {
    20
}
fn default_acquire_timeout_secs() -> u64 {
    30
}
fn default_query_llm() -> bool {
    true
}
fn default_query_timeout_secs() -> u64 {
    300
}
fn default_abstract_fallback() -> bool {
    true
}

/// `[adapters.paperqa.evidence]`（ADR-0035 D3）: 決定的な証拠ゲートの閾値（ADR-0031 D2 の literature 版）。
/// ハーネス（このアダプタ）が取得の結果と答えの引用を機械的に見る（LLM に判断させない）。
/// `0` を書けばその項目は見ない。全部 0 ならゲート無し。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaperQaEvidence {
    /// 検索が返した候補論文（重複排除後）の数の下限。
    #[serde(default = "default_min_candidates")]
    pub min_candidates: u32,
    /// corpus に入った PDF の数の下限（既にあったものを含む）。
    #[serde(default = "default_min_pdfs")]
    pub min_pdfs: u32,
    /// 答えが引用した出典の数の下限。
    #[serde(default = "default_min_cited")]
    pub min_cited: u32,
    /// ADR-0063 D1（Phase 109）: 閾値未達（`min_cited` 等）を hard error にするか（既定 `false`）。
    /// `false`（既定）なら証拠不足でも `Terminal::Done` にし、`research.json` の `evidence` と
    /// `answer.md` の「証拠の質」節に内訳と代替案を書いて reviewer / 受け入れ条件の判断に委ねる。
    /// `true` にすると Phase 108 までどおり `Terminal::Error{retryable: true}`。
    /// 取得が 0 件（検索経路の問題）は、この設定に関係なく常に hard error のまま。
    #[serde(default)]
    pub insufficient_is_error: bool,
}

impl Default for PaperQaEvidence {
    fn default() -> Self {
        Self {
            min_candidates: default_min_candidates(),
            min_pdfs: default_min_pdfs(),
            min_cited: default_min_cited(),
            insufficient_is_error: false,
        }
    }
}

fn default_min_candidates() -> u32 {
    5
}
fn default_min_pdfs() -> u32 {
    3
}
fn default_min_cited() -> u32 {
    2
}

/// `[adapters.paperqa]`（config.toml, ADR-0027 D3）。`[[providers]] adapter = "paperqa"` の行ごとに
/// `settings` / `env` / `model` を上書きできる（ADR-0026 D2 と同じ作り）。
#[derive(Debug, Clone)]
pub struct PaperQaConfig {
    /// ADR-0063 Phase 109d C2: 起動するコマンド。**`pqa` CLI ではなく python インタプリタ**
    /// （`paperqa_ask.py` を `<command> <script> <input.json>` として起動する。既定 `"python"`、
    /// 通常は `paperqa` パッケージが入った venv の `bin/python`）。旧 `pqa`（Phase 108 までの既定、
    /// または運用者が明示的に指している場合）が来たら、同じディレクトリの `python` に自動で
    /// 置き換えて 1 回警告する（`resolve_ask_command`）。
    pub command: String,
    /// `-s <name>`（拡張子は付けない。`pqa` 自身が `.json` を足す。ADR-0027 の実機の仕様）。未指定なら渡さない。
    pub settings: Option<String>,
    /// `--agent.index.paper_directory` の親ディレクトリ（実際に渡す値はこの下に**案件ごと**の
    /// サブディレクトリを足したもの。ADR-0035 D1）。未指定ならワークスペース相対 `papers`。
    pub paper_directory: Option<PathBuf>,
    /// `--agent.index.index_directory` の親ディレクトリ（実際に渡す値はこの下に**案件ごと**の
    /// サブディレクトリを足したもの。ADR-0035 D2。ADR-0027 D3 の「タスクごと」からの変更）。
    /// 未指定ならワークスペース相対 `index`。
    pub index_directory: Option<PathBuf>,
    /// `--agent.index.name`。未指定なら案件の鍵（`project_id`、無ければ `_shared`）を使う。
    pub index_name: Option<String>,
    /// `settings.llm` の上書き。設定されているときだけ渡す（PaperQA の設定ファイルの値より優先。
    /// ADR-0027 D3。ADR-0063 Phase 109d C1/C2: CLI の `--llm` から `paperqa_ask.py` への
    /// `input.json` の `model` フィールドに変わった。意味は同じ）。
    pub model: Option<String>,
    /// 追加の環境変数（例: `OPENAI_API_KEY` / `OPENAI_BASE_URL`。LiteLLM 経由の OpenAI 互換エンドポイント向け）。
    pub env: Vec<(String, String)>,
    /// Phase 108 までの `pqa ask` CLI 用の追加引数。ADR-0063 Phase 109d C1/C2 で CLI 呼び出しを
    /// 廃止したため、いまは何もしない（設定ファイルの互換性のためフィールドだけ残す）。
    pub extra_args: Vec<String>,
    /// ADR-0035 D1: 文献の取得。
    pub acquire: AcquireConfig,
    /// ADR-0035 D3: 決定的な証拠ゲートの閾値。
    pub evidence: PaperQaEvidence,
    /// ADR-0063 Phase 109d C3: `ask()` を呼ぶ回数の上限（対象ごとの問い + 総括の問い）。既定 10
    /// （Phase 109h: 8 から引き上げ）。比較先があるときは総括の問いを必ず残し、対象側を後ろから
    /// 詰める（`build_questions_for_targets`、`paperqa_ask.py`）。
    pub max_asks: u32,
    /// ADR-0063 Phase 109g A: `[knowledge] root`（絶対パス）。設定されていれば `paperqa_ask.py` が
    /// 比較先の設計条件（知識ベースのページ本文）をここから直接読む（`ask_input.json` の
    /// `comparison_context.knowledge_root`）。`None` ならページ本文は使わず、目的文の周辺 1 文に
    /// 落ちる（`research_targets::comparison_target_paragraph`）。コンテナで走る run では
    /// `container.knowledge_root` と同じ絶対パスがマウントされているので、そのまま読める。
    pub knowledge_root: Option<PathBuf>,
}

impl Default for PaperQaConfig {
    fn default() -> Self {
        Self {
            command: "python".to_string(),
            settings: None,
            paper_directory: None,
            index_directory: None,
            index_name: None,
            model: None,
            env: Vec::new(),
            extra_args: Vec::new(),
            acquire: AcquireConfig::default(),
            evidence: PaperQaEvidence::default(),
            max_asks: default_max_asks(),
            knowledge_root: None,
        }
    }
}

fn default_max_asks() -> u32 {
    10
}

#[derive(Debug, Clone)]
pub struct PaperQaAdapter {
    config: PaperQaConfig,
}

impl PaperQaAdapter {
    pub const ID: &'static str = "paperqa";

    pub fn new(config: PaperQaConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerAdapter for PaperQaAdapter {
    fn id(&self) -> &str {
        Self::ID
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        run_paperqa(&self.config, &req, run_id, &limits, sink).await
    }

    /// 他のアダプタ（`claude_code`/`codex`）と同じ規則: `extra` は `config.env` の末尾に足すので、
    /// 同名キーは `extra` が勝つ。
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut config = self.config.clone();
        config.env.extend(extra.iter().cloned());
        Some(Arc::new(PaperQaAdapter::new(config)))
    }
}

/// タスクの目的から `pqa ask` に渡す問いを組み立てる（ADR-0027 D3）。`claude_code::build_prompt` は
/// コーディング用の文面（受け入れ条件・`artifacts/result.json` の書式指示・委譲の案内）なので流用せず、
/// 素の目的 + 前置き（役割の指示文・記憶・直近のやり取り）+ 人間の回答履歴だけを使う（PaperQA2 は
/// ワーカープロトコルを話さない調査エンジン
/// であり、結果ファイルの書式やコマンド再実行の話をしても意味がないため）。`request.json`/`prompt.txt` は
/// 他のアダプタと同じ共有ヘルパ（`subprocess::write_run_request`/`write_run_prompt`）で残す。
pub fn build_question(task: &Task, context: &RunContext, artifacts: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", task.title));
    // ADR-0033 D4 / D6（Phase 24）: 前置き（役職と brief・永続の認可・記憶・直近のやり取り・役割の指示文）は
    // `crate::preamble` が 1 か所で組む。`RunContext` が Phase 23 までの中身なら出力は変わらない。
    out.push_str(&crate::preamble::render(context, artifacts));
    out.push_str(&task.objective);
    out.push('\n');
    // ADR-0063 Phase 109c A/C（縮小版。P-109c-1）: 目的文から対象が取れているとき、答えを
    // 「対象ごとの節 + 対象×観点の表」に構造化するよう指示する。対象ごとに `pqa ask` を複数回呼ぶ
    // （元の ADR C2）と PaperQA の Python API への切り替え（C1）は、実機で API 面を確認できず
    // 既存テストへの影響も大きいため Phase 109c では見送った（未解決事項として PROGRESS.md に記載）。
    let targets = crate::research_targets::research_targets(&task.objective);
    if !targets.is_empty() {
        let aspects = crate::research_targets::research_aspects(&task.objective);
        out.push_str(&render_structured_answer_instructions(&targets, &aspects));
    }
    if !context.answers.is_empty() {
        out.push_str("\n## Answers from a human to earlier questions\n");
        for Answer { question, answer } in &context.answers {
            out.push_str(&format!("- Q: {question}\n  A: {answer}\n"));
        }
    }
    out
}

/// ADR-0063 Phase 109c A/C: 対象が取れているときに `pqa ask` へ足す、答えの形式についての指示。
/// 各観点は事実（出典）か「未確認」のどちらかで書かせ、対象ごとの節に加えて対象×観点の表もまとめさせる
/// （観測された失敗: 対象別の整理が無い総論だけの答え、引用の対応不足）。
fn render_structured_answer_instructions(targets: &[String], aspects: &[String]) -> String {
    let mut out = String::from("\n## 回答の形式（必ず守ること）\n\n");
    out.push_str(
        "次の対象それぞれについて `### <対象名>` の見出しを立て、観点ごとに \
         `- <観点>: <文献にある事実（出典）>` または `- <観点>: 未確認` の形で書け。\
         文献に無い観点は推測せず「未確認」と書け。対象ごとの節のあとに、対象を行、観点を列とする \
         Markdown の表でまとめよ（セルは同じく事実（出典）か「未確認」）。\n\n",
    );
    out.push_str(&format!("対象: {}\n", targets.join("、")));
    out.push_str(&format!("観点: {}\n", aspects.join("、")));
    out
}

/// 案件の鍵（corpus と索引のサブディレクトリ名。ADR-0035 D1）。案件が無いタスクは `_shared`。
/// パス要素として安全な文字（英数字・`-`・`_`）だけを残す（ULID はもともと英数字だが、
/// 将来 id の形が変わってもディレクトリを抜け出さないため）。
pub fn project_key(task: &Task) -> String {
    let raw = match &task.project_id {
        Some(id) => id.to_string(),
        None => return SHARED_PROJECT_KEY.to_string(),
    };
    let safe: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.trim_matches('_').is_empty() {
        SHARED_PROJECT_KEY.to_string()
    } else {
        safe
    }
}

/// ADR-0063 D1（Phase 109）: 起点の資料（目的文や `inputs` に含まれる URL）の種類。決定的な分類
/// （LLM は使わない）。`Pdf` / `Doi` / `Arxiv` は取得ランナーに渡して論文として取り込み、`Github` は
/// 「一次情報（実装）」として `answer.md` の参照節に載せる（corpus には入れない）。`Other` は何もしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedUrlKind {
    Pdf,
    Doi,
    Arxiv,
    Github,
    Other,
}

impl SeedUrlKind {
    fn as_str(self) -> &'static str {
        match self {
            SeedUrlKind::Pdf => "pdf",
            SeedUrlKind::Doi => "doi",
            SeedUrlKind::Arxiv => "arxiv",
            SeedUrlKind::Github => "github",
            SeedUrlKind::Other => "other",
        }
    }
}

/// ADR-0063 D1: 目的文や `inputs` から拾った 1 件の起点 URL。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedUrl {
    pub url: String,
    pub kind: SeedUrlKind,
}

/// URL を種類ごとに分類する（決定的。ADR-0063 D1）。
pub fn classify_seed_url(url: &str) -> SeedUrlKind {
    let lower = url.to_ascii_lowercase();
    let without_query = lower.split(['?', '#']).next().unwrap_or(&lower);
    if lower.contains("arxiv.org") {
        SeedUrlKind::Arxiv
    } else if lower.contains("github.com") || lower.contains("gitlab.com") {
        SeedUrlKind::Github
    } else if lower.contains("doi.org/") {
        SeedUrlKind::Doi
    } else if without_query.ends_with(".pdf") {
        SeedUrlKind::Pdf
    } else {
        SeedUrlKind::Other
    }
}

/// テキストの中の `http(s)://` で始まるトークンを全て拾う（決定的、正規表現は使わない。ADR-0063 D1）。
/// 前後の日本語の括弧・句読点や引用符は落とす。
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for token in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '「' | '」' | '（' | '）' | '(' | ')' | '<' | '>' | '"' | '\'' | '　'
            )
    }) {
        let trimmed = token.trim_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// 起点の資料（目的文 + `inputs` の `path`）から seed URL を集める（重複排除、出現順。ADR-0063 D1）。
pub fn extract_seed_urls(objective: &str, inputs: &[task_core::ArtifactRef]) -> Vec<SeedUrl> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut candidates: Vec<String> = extract_urls(objective);
    for input in inputs {
        candidates.extend(extract_urls(&input.path));
    }
    for url in candidates {
        if seen.insert(url.clone()) {
            let kind = classify_seed_url(&url);
            out.push(SeedUrl { url, kind });
        }
    }
    out
}

/// ADR-0063 Phase 109g B: 知識ベースの索引（`context.knowledge.index`）のうち `primary-sources` /
/// `一次情報` タグを持つページの `sources` から拾う seed URL。`local_deep_research::must_read_urls`
/// と同じタグ判定（大小文字を無視して `primary-source`/`一次情報` を含む）。DOI / arXiv / PDF は
/// `extract_seed_urls` と同じ扱いで取得ランナーの種に、GitHub / GitLab は「一次情報（実装）」に載る
/// （`classify_seed_url` に委ねる。それ以外の URL は捨てる）。重複は落とし、出現順を保つ（決定的、
/// LLM は使わない）。
pub fn kb_primary_source_seed_urls(
    knowledge: Option<&crate::protocol::KnowledgeContext>,
) -> Vec<SeedUrl> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let Some(k) = knowledge else {
        return out;
    };
    for item in &k.index {
        let is_primary = item.tags.iter().any(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("primary-source") || t.contains("一次情報")
        });
        if !is_primary {
            continue;
        }
        for url in &item.sources {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            if seen.insert(url.clone()) {
                out.push(SeedUrl {
                    url: url.clone(),
                    kind: classify_seed_url(url),
                });
            }
        }
    }
    out
}

/// 依頼文（`objective`）から検索語を決定的に作る（ADR-0035 D1 手順 1。**LLM は使わない**）。
///
/// Phase 36（ADR-0035 D5）以降、これは**受け皿**である: 通常は LLM が立てた検索語を使い、その答えが
/// 壊れていたときだけこの語で検索する（`acquire.query_llm = false` にすると常にこちらだけを使う）。
///
/// 依頼文が日本語でも、その中の**英数字の名詞句**（`Pluvio` / `ad-hoc FS` /
/// `asynchronous I/O runtime` のように、日本語や句読点で区切られた ASCII の連なり）は英語の検索語に
/// なる。語数の多い順・出現順で最大 `MAX_SEARCH_QUERIES` 本を選ぶ。1 本も取れなければ
/// `objective` 全文を 1 本の検索語にする（検索エンジンに丸投げする最後の手段）。
pub fn build_search_queries(objective: &str) -> Vec<String> {
    /// 英字・数字とその間に入りうる記号（`I/O`、`ad-hoc`、`C++`、`.NET` 等）だけを句の材料にする。
    fn is_phrase_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | '+' | '#' | '.' | '_' | '\'')
    }
    fn is_stopword(word: &str) -> bool {
        matches!(
            word,
            "a" | "an"
                | "and"
                | "are"
                | "as"
                | "at"
                | "be"
                | "by"
                | "for"
                | "from"
                | "in"
                | "is"
                | "of"
                | "on"
                | "or"
                | "that"
                | "the"
                | "to"
                | "via"
                | "vs"
                | "with"
        )
    }

    // 1. ASCII の句に切り出す（区切りは日本語・句読点・改行）。句の中の単語は空白で区切られる。
    let mut phrases: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let flush_word = |word: &mut String, current: &mut Vec<String>| {
        let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if !trimmed.is_empty() {
            current.push(trimmed.to_string());
        }
        word.clear();
    };
    for c in objective.chars() {
        if is_phrase_char(c) {
            word.push(c);
        } else if c == ' ' || c == '\t' {
            flush_word(&mut word, &mut current);
        } else {
            flush_word(&mut word, &mut current);
            if !current.is_empty() {
                phrases.push(current.join(" "));
                current.clear();
            }
        }
    }
    flush_word(&mut word, &mut current);
    if !current.is_empty() {
        phrases.push(current.join(" "));
    }

    // 2. 検索語として意味の無いものを落とす。
    let mut kept: Vec<String> = Vec::new();
    for phrase in phrases {
        let words: Vec<&str> = phrase.split(' ').filter(|w| !w.is_empty()).collect();
        let meaningful: Vec<&str> = words
            .iter()
            .copied()
            .filter(|w| !is_stopword(&w.to_ascii_lowercase()))
            .collect();
        if meaningful.is_empty() {
            continue;
        }
        // 1 語だけの句は、大文字を含む（固有名詞・略語。`Pluvio` / `FS`）か 4 文字以上のときだけ残す。
        if meaningful.len() == 1 {
            let only = meaningful[0];
            let has_upper = only.chars().any(|c| c.is_ascii_uppercase());
            if !has_upper && only.chars().count() < 4 {
                continue;
            }
            if only.chars().count() < 2 {
                continue;
            }
        }
        let text = meaningful.join(" ");
        let lower = text.to_ascii_lowercase();
        if kept.iter().any(|k| k.to_ascii_lowercase() == lower) {
            continue;
        }
        kept.push(text);
    }

    // 3. 他の句に丸ごと含まれる句は落とす（`ad-hoc FS` ⊂ `ad-hoc FS server` のような場合）。
    let mut unique: Vec<String> = Vec::new();
    for (i, phrase) in kept.iter().enumerate() {
        let lower = phrase.to_ascii_lowercase();
        let contained = kept.iter().enumerate().any(|(j, other)| {
            j != i && other.len() > phrase.len() && other.to_ascii_lowercase().contains(&lower)
        });
        if !contained {
            unique.push(phrase.clone());
        }
    }

    // 4. 語数の多い順（同数なら出現順）で上限まで。
    let mut ordered: Vec<(usize, usize, String)> = unique
        .into_iter()
        .enumerate()
        .map(|(i, p)| (p.split(' ').count(), i, p))
        .collect();
    ordered.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let queries: Vec<String> = ordered
        .into_iter()
        .take(MAX_SEARCH_QUERIES)
        .map(|(_, _, p)| p)
        .collect();

    if queries.is_empty() {
        let fallback = objective.split_whitespace().collect::<Vec<_>>().join(" ");
        if fallback.is_empty() {
            Vec::new()
        } else {
            vec![fallback]
        }
    } else {
        queries
    }
}

/// LiteLLM の `provider/model` 形式から供給者の接頭辞を落とす（ADR-0035 D5）。
/// `chat/completions` を直接叩くときに必要なのは、その口が出しているモデル名
/// （例: settings の `llm` が `openai/celeris/standard` なら要求モデルは `celeris/standard`）。
/// 接頭辞と見なすのは小文字・数字・`_` だけの最初の 1 区画（`openai/` / `hosted_vllm/`）。
/// ただし `celeris/<tier>` は proxy のモデル名そのものなので残す。
fn strip_provider_prefix(model: &str) -> String {
    let model = model.trim();
    match model.split_once('/') {
        Some((prefix, rest))
            if prefix != "celeris"
                && !rest.is_empty()
                && !prefix.is_empty()
                && prefix
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') =>
        {
            rest.to_string()
        }
        _ => model.to_string(),
    }
}

/// PaperQA の設定ファイル（`-s <settings>` に渡す値 + `.json`）の `llm`（ADR-0035 D5）。
/// 読めない・書かれていなければ `None`。
async fn settings_llm(settings: &str) -> Option<String> {
    let path = if settings.ends_with(".json") {
        PathBuf::from(settings)
    } else {
        PathBuf::from(format!("{settings}.json"))
    };
    let text = tokio::fs::read_to_string(&path).await.ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let llm = value.get("llm").and_then(serde_json::Value::as_str)?.trim();
    if llm.is_empty() {
        None
    } else {
        Some(llm.to_string())
    }
}

/// 検索語を立てる LLM のモデル名（ADR-0035 D5）。`acquire.query_model` → `[[providers]] model`
/// （`--llm`）→ PaperQA の `settings` の `llm` の順。**PaperQA2 と同じ LLM 先**を使うため。
async fn query_llm_model(config: &PaperQaConfig) -> Option<String> {
    for candidate in [config.acquire.query_model.clone(), config.model.clone()]
        .into_iter()
        .flatten()
    {
        let model = strip_provider_prefix(&candidate);
        if !model.is_empty() {
            return Some(model);
        }
    }
    let settings = config.settings.as_deref()?;
    let llm = settings_llm(settings).await?;
    let model = strip_provider_prefix(&llm);
    if model.is_empty() { None } else { Some(model) }
}

/// `config.env` に入っている値（`OPENAI_BASE_URL` / `OPENAI_API_KEY`）。同名キーは後の行が勝つ
/// （`with_env` の規則と同じ）。
fn env_value(config: &PaperQaConfig, key: &str) -> Option<String> {
    config
        .env
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// 検索語を立てる LLM に渡すもの（ADR-0035 D5）。タスクの `title` / `objective` と、
/// あればこの案件についての記憶（`context.memory.project`。`RunContext` に案件の依頼文そのものは
/// 無いので、案件単位で人が与えた文脈として最も近いものを渡す）。
fn query_request_block(req: &RunRequest) -> serde_json::Value {
    let context = req
        .context
        .memory
        .as_ref()
        .map(|m| m.project.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(|p| truncate_chars(&p, QUERY_CONTEXT_MAX_CHARS));
    serde_json::json!({
        "title": req.task.title,
        "objective": req.task.objective,
        "context": context,
    })
}

/// 取得ランナーを動かす python（ADR-0035 D1）。設定が無ければ `command`（既定は python インタプリタ
/// 自身。ADR-0063 Phase 109d C2）と同じディレクトリの `python3`（venv の中を指しているのが普通）、
/// ディレクトリが無ければ `python3`。
fn acquire_python(config: &PaperQaConfig) -> String {
    if let Some(command) = &config.acquire.command {
        return command.clone();
    }
    match Path::new(&config.command).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join("python3").to_string_lossy().into_owned()
        }
        _ => "python3".to_string(),
    }
}

/// ADR-0063 Phase 109d C2: `[adapters.paperqa] command` を実際に起動するコマンドに解決する。
/// 既定・現行の値は python インタプリタそのもの（`paperqa_ask.py` を `<command> <script> <input.json>`
/// として起動する）。旧 `pqa`（Phase 108 までの既定、または運用者がまだ CLI 実行ファイルを指している
/// 場合）が来たら、同じディレクトリの `python` に自動で置き換えて 1 回警告する。
fn resolve_ask_command(config: &PaperQaConfig, run_id: &str) -> String {
    let path = Path::new(&config.command);
    let is_old_pqa_cli = path.file_name().and_then(|n| n.to_str()) == Some("pqa");
    if !is_old_pqa_cli {
        return config.command.clone();
    }
    let replacement = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            parent.join("python").to_string_lossy().into_owned()
        }
        _ => "python".to_string(),
    };
    warn!(
        "run {run_id}: [adapters.paperqa] command={:?} looks like the old `pqa` CLI (ADR-0063 \
         Phase 109d C2 replaced it with the paperqa Python API); using the sibling python \
         interpreter {replacement:?} instead. Update the config to point at python directly.",
        config.command
    );
    replacement
}

/// ADR-0063 Phase 109e: `config.settings` はこのコードベースでは設定ファイルへの**フルパス**
/// （拡張子無し。`settings_llm` と同じ前提。ただし `.json` 付きでも名前だけでも受け付ける）として
/// 扱われてきた。`paperqa_ask.py` に渡す `settings_path`（**直接読む**絶対パス、`.json` 付き）/
/// `settings_dir`/`settings_name`（`settings_path` が組めない、または見つからないときの
/// `Settings.from_name` フォールバック用）に分解する。
///
/// Phase 109d までは `settings_dir` を `PQA_SETTINGS_DIR` として渡し `Settings.from_name` に
/// 探させていたが、本番の `Settings.from_name`（paperqa==2026.8.12）はその環境変数を見ない
/// （`~/.pqa/settings/` 固定）ため、それでは設定ファイルの実際の置き場所（`settings_dir`）を
/// 全く見つけられなかった（本番で観測。Phase 109e）。`settings_path` はその置き場所を
/// `paperqa_ask.py` に直接教える。
///
/// 3 通りの入力を同じ `settings_path` に正規化する:
/// - 拡張子無しのフルパス（`/a/b/name`）→ `/a/b/name.json`
/// - `.json` 付きのフルパス（`/a/b/name.json`）→ そのまま
/// - ディレクトリの無い名前だけ（`name`）→ `settings_path` は組めない（`None`）。
///   `settings_name` だけが残り、`paperqa_ask.py` 側は `Settings.from_name` に委ねる
///   （paperqa 自身の同梱設定名、例 `"high_quality"`、を指すときの唯一の経路）。
fn split_settings_path(settings: &str) -> (Option<String>, Option<String>, Option<String>) {
    let path = Path::new(settings);
    let name = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().into_owned());
    let settings_path = match (&dir, &name) {
        (Some(dir), Some(name)) => Some(format!("{dir}/{name}.json")),
        _ => None,
    };
    (settings_path, dir, name)
}

/// ADR-0063 Phase 109d C2: 答え（全問い）の `contexts` に現れた `docname`/`dockey` の集合
/// （英数字だけに正規化）。
fn context_identifiers(answers: &[AskAnswer]) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for answer in answers {
        for ctx in &answer.contexts {
            if !ctx.docname.is_empty() {
                set.insert(normalize_alnum(&ctx.docname));
            }
            if !ctx.dockey.is_empty() {
                set.insert(normalize_alnum(&ctx.dockey));
            }
        }
    }
    set
}

/// この候補が `context_ids`（`context_identifiers` の戻り値）のどれかと一致するか（ADR-0063 Phase
/// 109d C2）。`parsing.use_doc_details = false`（ADR-0027 の設定）だと PaperQA2 は corpus の
/// **ファイル名**から `docname` を作るので、`answer_cites` と同じくファイル名の語幹で突き合わせる。
fn candidate_in_contexts(context_ids: &BTreeSet<String>, candidate: &Candidate) -> bool {
    if candidate.file.is_empty() {
        return false;
    }
    let stem = candidate
        .file
        .trim_end_matches(".pdf")
        .trim_end_matches(".txt");
    let normalized_stem = normalize_alnum(stem);
    if normalized_stem.is_empty() {
        return false;
    }
    context_ids
        .iter()
        .any(|id| id.contains(&normalized_stem) || normalized_stem.contains(id.as_str()))
}

/// `artifacts/papers.json` の 1 件（ADR-0035 D1 手順 4。Phase 38 で `candidates.json` から改名）。ランナー（Python）が書き、
/// アダプタが `cited` の突き合わせと `## 出典` の組み立てに読む。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub venue: String,
    #[serde(default)]
    pub doi: String,
    #[serde(default)]
    pub arxiv_id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub pdf_url: String,
    /// corpus 内のファイル名（落とせなかった候補では空のこともある）。
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub pdf_downloaded: bool,
    #[serde(default)]
    pub source_engine: String,
    /// ADR-0063 D1（Phase 109）: メタデータの abstract（OpenAlex / arXiv / Semantic Scholar）。
    #[serde(default, rename = "abstract")]
    pub abstract_text: String,
    /// ADR-0063 D1: `true` なら corpus に入っているのは本文ではなくこの abstract（`file` はその
    /// テキストファイル）。`pdf_downloaded` と排他（両方 true にはならない）。
    #[serde(default)]
    pub abstract_only: bool,
}

/// `paperqa_ask.py` が `output_path` に書く 1 件の証拠（`PQASession.contexts` の 1 件。
/// ADR-0063 Phase 109d C1/C4）。`ask()` に渡した Python の型ではなく、その `docname`/`dockey`/
/// `citation`/`score` だけを写した JSON 表現。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AskContext {
    #[serde(default)]
    pub docname: String,
    #[serde(default)]
    pub dockey: String,
    #[serde(default)]
    pub citation: String,
    #[serde(default)]
    pub score: Option<f64>,
    /// この証拠を使った問いの `id`（`AskAnswer::id`）。
    #[serde(default)]
    pub question: String,
}

/// `paperqa_ask.py` が `output_path` に書く 1 問分の答え（ADR-0063 Phase 109d C1）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AskAnswer {
    #[serde(default)]
    pub id: String,
    /// 対象ごとの問いなら対象名、総括や（対象が取れないときの）単一の問いなら `None`。
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub has_successful_answer: bool,
    #[serde(default)]
    pub contexts: Vec<AskContext>,
    #[serde(default)]
    pub references: String,
    /// 再試行しても失敗した場合のメッセージ（成功すれば `None`）。
    #[serde(default)]
    pub error: Option<String>,
}

/// `paperqa_ask.py` の `output_path` 全体（ADR-0063 Phase 109d C1）。`error` が `Some` なら
/// `paperqa` そのものが import できなかった（exit 3。`answers` は空）。
#[derive(Debug, Clone, Default, Deserialize)]
struct AskOutput {
    #[serde(default)]
    answers: Vec<AskAnswer>,
    #[serde(default)]
    target_aspect_table: String,
    /// ADR-0063 Phase 109h: `max_asks` に収めるため後ろから削られた対象（比較先があるときだけ。
    /// 総括の問いは必ず残す。`build_questions_for_targets` が返す）。
    #[serde(default)]
    dropped_targets: Vec<String>,
    /// ADR-0063 Phase 109h: 比較先の設計条件に実際に使われた知識ベースのページの path
    /// （`find_comparison_page_path` の一致がページ本文の読み込みまで成功したときだけ。
    /// 見つからなければ `None`、目的文の周辺 1 文にフォールバックしたときも `None`）。
    #[serde(default)]
    comparison_page: Option<String>,
    /// ADR-0063 Phase 109h: 総括の判断の問いに埋め込んだ設計条件の文字数（比較先が無い、または
    /// 設計条件が全く取れなかったときは 0）。
    #[serde(default)]
    comparison_context_chars: u32,
    #[serde(default)]
    error: Option<String>,
}

/// 取得の段の結果（`CELERIS_ACQUIRE` の中身）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AcquireCounts {
    candidates: u32,
    pdfs: u32,
    /// ADR-0063 D1（Phase 109）: 本文が取れず abstract で corpus に入れた件数。
    abstracts: u32,
}

/// 子プロセスの標準出力を 1 行ずつ読み、生存監視（壁時計・無出力）を行う（両方の段で共用）。
struct StreamedRun {
    stdout: String,
    stderr_tail: String,
    exit: std::process::ExitStatus,
    /// 上限を超えて強制終了したときの終端（超えていなければ `None`）。
    timeout: Option<Terminal>,
}

#[allow(clippy::too_many_arguments)]
async fn stream_child(
    mut child: Child,
    stdout_log_path: PathBuf,
    stderr_log_path: PathBuf,
    start: Instant,
    limits: &RunLimits,
    run_id: &str,
    sink: &dyn EventSink,
    mut on_line: impl FnMut(&str),
) -> Result<StreamedRun, AdapterError> {
    let stderr_log_path_for_task = stderr_log_path.clone();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AdapterError::Other("worker stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Other("worker stderr was not piped".into()))?;

    let stderr_task = tokio::spawn(async move {
        let mut reader = stderr;
        match tokio::fs::File::create(&stderr_log_path_for_task).await {
            Ok(mut file) => {
                if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
                    warn!("failed to write worker stderr.log: {e}");
                }
            }
            Err(e) => warn!("failed to create worker stderr.log: {e}"),
        }
    });

    let mut stdout_file = tokio::fs::File::create(&stdout_log_path).await?;
    let mut reader = BufReader::new(stdout);

    let mut last_activity = Instant::now();
    let mut stdout_buf = String::new();
    let mut force_kill = false;
    let mut timeout_terminal: Option<Terminal> = None;

    loop {
        // 壁時計は run 全体（取得 + pqa）で数える。2 段目に入る時点で残りが無ければすぐ打ち切る。
        let wall_elapsed = start.elapsed();
        if wall_elapsed >= limits.wall_clock {
            timeout_terminal = Some(Terminal::Error {
                message: "wall clock exceeded".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let idle_elapsed = last_activity.elapsed();
        if idle_elapsed >= limits.idle_timeout {
            timeout_terminal = Some(Terminal::Error {
                message: "idle timeout".into(),
                retryable: true,
            });
            force_kill = true;
            break;
        }
        let wait = (limits.wall_clock - wall_elapsed).min(limits.idle_timeout - idle_elapsed);

        let outcome = match tokio::time::timeout(
            wait,
            read_line_limited(&mut reader, MAX_LINE_BYTES),
        )
        .await
        {
            Err(_elapsed) => continue, // タイムアウト。ループ先頭で上限超過を検知する。
            Ok(Err(e)) => return Err(AdapterError::Io(e)),
            Ok(Ok(outcome)) => outcome,
        };

        match outcome {
            LineOutcome::Eof => break,
            LineOutcome::TooLong => {
                // pqa / ランナーのフォーマットは celeris が定義したものではないので寛容に無視する
                // （claude_code と同じ考え方）。
                sink.heartbeat();
                last_activity = Instant::now();
                warn!("run {run_id}: discarding overlong line from the paperqa stage");
            }
            LineOutcome::Line(bytes) => {
                sink.heartbeat();
                last_activity = Instant::now();
                stdout_file.write_all(&bytes).await?;
                stdout_file.write_all(b"\n").await?;
                let text = String::from_utf8_lossy(&bytes);
                let trimmed = text.trim().to_string();
                stdout_buf.push_str(&text);
                stdout_buf.push('\n');
                if !trimmed.is_empty() {
                    on_line(&trimmed);
                }
            }
        }
    }

    let exit = if force_kill {
        kill_now(&mut child, limits.kill_grace).await?
    } else {
        reap_after_terminal(&mut child, limits.kill_grace).await?
    };
    if let Err(e) = stderr_task.await {
        warn!("run {run_id}: stderr capture task failed: {e}");
    }
    stdout_file.flush().await?;
    let stderr_tail = read_tail(&stderr_log_path, 4096).await;

    Ok(StreamedRun {
        stdout: stdout_buf,
        stderr_tail,
        exit,
        timeout: timeout_terminal,
    })
}

/// 取得の段（ADR-0035 D1 / D2 手順 1）。**失敗しても run は止めない**（`progress` に残して `pqa` に進み、
/// 判定は証拠ゲートに任せる）。壁時計・無出力の上限に当たったときだけ終端を返す。
#[allow(clippy::too_many_arguments)]
async fn run_acquire(
    config: &PaperQaConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    start: Instant,
    sink: &dyn EventSink,
    run_dir: &Path,
    artifacts_dir: &Path,
    paper_directory: &Path,
    seed_urls: &[SeedUrl],
) -> Result<(AcquireCounts, Option<Terminal>), AdapterError> {
    // ADR-0035 D5（Phase 36）: 検索語はランナーの中で LLM が立てる。ここで作るのは
    // **LLM の答えが壊れていたときの受け皿**（Phase 34 までの決定的な抽出）。
    let queries = build_search_queries(&req.task.objective);
    let model = if config.acquire.query_llm {
        query_llm_model(config).await
    } else {
        None
    };
    match &model {
        Some(model) => progress::emit_status(
            sink,
            &truncate_chars(
                &format!("acquiring literature; the search terms are written by {model}"),
                PROGRESS_LINE_MAX_CHARS,
            ),
        ),
        None => progress::emit_status(
            sink,
            &truncate_chars(
                &format!(
                    "acquiring literature for {} search term(s) (deterministic): {}",
                    queries.len(),
                    queries.join(" | ")
                ),
                PROGRESS_LINE_MAX_CHARS,
            ),
        ),
    }

    let script_path = run_dir.join("paperqa_acquire.py");
    let input_path = run_dir.join("acquire_input.json");
    // ADR-0063 D4（Phase 109）: `acquire_input.json` はワーカー/レビュアーが読める run ディレクトリに
    // 平文で残る（権限 664）。API キーの実際の値はここには書かず、子プロセスの環境変数（`.envs(config.env)`
    // で既に渡っている）だけで受け渡す。JSON にはプレースホルダだけを書く（`paperqa_acquire.py` の
    // `resolve_env_placeholder` が解決する）。
    let api_key_placeholder =
        env_value(config, "OPENAI_API_KEY").map(|_| "<env:OPENAI_API_KEY>".to_string());
    let seed_urls_json: Vec<serde_json::Value> = seed_urls
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SeedUrlKind::Pdf | SeedUrlKind::Doi | SeedUrlKind::Arxiv
            )
        })
        .map(|s| serde_json::json!({"url": s.url, "kind": s.kind.as_str()}))
        .collect();
    let input = serde_json::json!({
        "queries": queries,
        "query_llm": {
            "enabled": model.is_some(),
            "model": model,
            "base_url": env_value(config, "OPENAI_BASE_URL"),
            "api_key": api_key_placeholder,
            "timeout_secs": config.acquire.query_timeout_secs,
            "max_tokens": QUERY_LLM_MAX_TOKENS,
            "max_queries": MAX_LLM_SEARCH_QUERIES,
        },
        "request": query_request_block(req),
        "paper_directory": paper_directory.to_string_lossy(),
        "papers_path": artifacts_dir.join("papers.json").to_string_lossy(),
        "sources_path": artifacts_dir.join("sources.json").to_string_lossy(),
        "queries_path": artifacts_dir.join("queries.json").to_string_lossy(),
        "openalex_filter": config.acquire.openalex_filter,
        "max_candidates": config.acquire.max_candidates,
        "max_pdfs": config.acquire.max_pdfs,
        "per_query": config.acquire.per_query,
        "timeout_secs": config.acquire.timeout_secs,
        "mailto": config.acquire.mailto,
        "abstract_fallback": config.acquire.abstract_fallback,
        "seed_urls": seed_urls_json,
    });
    tokio::fs::write(&script_path, ACQUIRE_SCRIPT).await?;
    let input_text = serde_json::to_string_pretty(&input)?;
    tokio::fs::write(&input_path, format!("{input_text}\n")).await?;

    let python = acquire_python(config);
    let mut command = Command::new(&python);
    command
        .arg(&script_path)
        .arg(&input_path)
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    // ADR-0095 D2: 本番 DB を読み取り専用にした namespace で起動する（コンテナ非対応の adapter）。
    let mut command = crate::db_guard::launch(command, None);

    let child = match command.spawn() {
        // ADR-0044 §5 Phase 53 追記（Phase 55）: 取得ランナーも同じ run の一族（`pqa ask` の前に
        // 順に走るので、表の `run_id` は重ならない）。
        Ok(child) => {
            let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());
            child
        }
        Err(e) => {
            // 取得ランナーが起動できないのは設定の誤り（python のパス）だが、ここでは run を止めず
            // 0 件として先に進む（ゲートが「取得が 0 件」として人に返す）。
            warn!(
                "run {run_id}: could not start the literature acquisition runner ({python}): {e}"
            );
            progress::emit_status(
                sink,
                &truncate_chars(
                    &format!("literature acquisition could not start ({python}): {e}"),
                    PROGRESS_LINE_MAX_CHARS,
                ),
            );
            return Ok((AcquireCounts::default(), None));
        }
    };

    let mut counts: Option<AcquireCounts> = None;
    let streamed = stream_child(
        child,
        run_dir.join("acquire.stdout.log"),
        run_dir.join("acquire.stderr.log"),
        start,
        limits,
        run_id,
        sink,
        |line| {
            if let Some(rest) = line.strip_prefix(PROGRESS_PREFIX) {
                progress::emit_status(
                    sink,
                    &truncate_chars(
                        &format!("acquire: {}", rest.trim()),
                        PROGRESS_LINE_MAX_CHARS,
                    ),
                );
            } else if let Some(rest) = line.strip_prefix(ACQUIRE_RESULT_PREFIX) {
                match serde_json::from_str::<serde_json::Value>(rest) {
                    Ok(value) => {
                        let number = |key: &str| -> u32 {
                            value
                                .get(key)
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0) as u32
                        };
                        counts = Some(AcquireCounts {
                            candidates: number("candidates"),
                            pdfs: number("pdfs"),
                            abstracts: number("abstracts"),
                        });
                    }
                    Err(e) => warn!("run {run_id}: could not parse CELERIS_ACQUIRE line: {e}"),
                }
            }
        },
    )
    .await?;

    if let Some(terminal) = streamed.timeout {
        return Ok((counts.unwrap_or_default(), Some(terminal)));
    }
    if !streamed.exit.success() {
        let tail = streamed
            .stderr_tail
            .lines()
            .next_back()
            .unwrap_or("")
            .to_string();
        warn!("run {run_id}: the literature acquisition runner failed: {tail}");
        progress::emit_status(
            sink,
            &truncate_chars(
                &format!("literature acquisition failed: {tail}"),
                PROGRESS_LINE_MAX_CHARS,
            ),
        );
    }
    let counts = counts.unwrap_or_default();
    progress::emit_status(
        sink,
        &format!(
            "acquire: {} candidate(s), {} PDF(s), {} abstract(s) in the corpus",
            counts.candidates, counts.pdfs, counts.abstracts
        ),
    );
    Ok((counts, None))
}

async fn run_paperqa(
    config: &PaperQaConfig,
    req: &RunRequest,
    run_id: &str,
    limits: &RunLimits,
    sink: &dyn EventSink,
) -> Result<RunOutcome, AdapterError> {
    let start = Instant::now();
    let run_dir = req.workspace.join("runs").join(run_id);
    tokio::fs::create_dir_all(&run_dir).await?;

    // 前回の run（リトライ）の名残を今回の結果と誤読しない（claude_code/codex と同じ理由。ADR-0006 D3）。
    // ADR-0036 D1/D2: 成果物の置き場はディスパッチャが決めた `artifacts_dir`（共有 workspace ではタスクごと）。
    let artifacts_dir = req.artifacts_dir.clone();
    let artifacts_rel = req.artifacts_rel();
    // ADR-0035 D5: `queries.json`（どの検索語で探したか）も前回の残りを消す。
    // ADR-0063 D1（Phase 109）: `research.json`（証拠の内訳）も同様。
    for stale in [
        "result.json",
        "answer.md",
        "papers.json",
        "sources.json",
        "queries.json",
        "research.json",
    ] {
        let _ = tokio::fs::remove_file(artifacts_dir.join(stale)).await;
    }

    let question = build_question(&req.task, &req.context, &artifacts_rel);
    // ADR-0023 D2 / M1: この run で何を渡したかを残す。
    crate::subprocess::write_run_request(&run_dir, req, run_id).await;
    crate::subprocess::write_run_prompt(&run_dir, &question, run_id).await;

    // ADR-0063 Phase 109c A: `research.json` に残す（`build_question` が既に同じ関数で問いに反映済み）。
    let research_targets = crate::research_targets::research_targets(&req.task.objective);
    let research_aspects = crate::research_targets::research_aspects(&req.task.objective);

    // ADR-0063 D1: 起点の資料（目的文 + `inputs` の URL）。PDF / DOI / arXiv は取得ランナーへの seed に、
    // GitHub / GitLab は「一次情報（実装）」として答えの参照節に載せる（corpus には入れない）。
    // ADR-0063 Phase 109g B: 知識ベースの `primary-sources` タグのページの `sources` にある URL も
    // 同じ種に足す（既知の論文を種にする。目的文由来の URL が優先、重複は落とす）。
    let mut seed_urls = extract_seed_urls(&req.task.objective, &req.task.inputs);
    let mut seen_seed_urls: std::collections::BTreeSet<String> =
        seed_urls.iter().map(|s| s.url.clone()).collect();
    for kb_seed in kb_primary_source_seed_urls(req.context.knowledge.as_ref()) {
        if seen_seed_urls.insert(kb_seed.url.clone()) {
            seed_urls.push(kb_seed);
        }
    }
    let fetchable_seeds: Vec<SeedUrl> = seed_urls
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SeedUrlKind::Pdf | SeedUrlKind::Doi | SeedUrlKind::Arxiv
            )
        })
        .cloned()
        .collect();
    let github_urls: Vec<String> = seed_urls
        .iter()
        .filter(|s| s.kind == SeedUrlKind::Github)
        .map(|s| s.url.clone())
        .collect();

    // ADR-0035 D1 / D2: corpus も索引も**案件ごと**（同じ案件の別タスク・リトライで使い回せる。
    // ADR-0027 D3 の「索引はタスクごと」からの変更）。
    let project = project_key(&req.task);
    let paper_directory = config
        .paper_directory
        .clone()
        .unwrap_or_else(|| PathBuf::from("papers"))
        .join(&project);
    let index_directory = config
        .index_directory
        .clone()
        .unwrap_or_else(|| PathBuf::from("index"))
        .join(&project);
    let index_name = config.index_name.clone().unwrap_or_else(|| project.clone());

    // ---------------------------------------------------------------- 1 段目: 取得
    let acquiring = config.acquire.max_candidates > 0;
    let acquired = if acquiring {
        let absolute_papers = if paper_directory.is_absolute() {
            paper_directory.clone()
        } else {
            req.workspace.join(&paper_directory)
        };
        if let Err(e) = tokio::fs::create_dir_all(&artifacts_dir).await {
            warn!("run {run_id}: could not create artifacts/ directory: {e}");
        }
        let (counts, timeout) = run_acquire(
            config,
            req,
            run_id,
            limits,
            start,
            sink,
            &run_dir,
            &artifacts_dir,
            &absolute_papers,
            &fetchable_seeds,
        )
        .await?;
        if let Some(terminal) = timeout {
            // タイムアウトは供給側失敗として分類しない（他アダプタと同じ。ADR-0010 D5）。
            write_result_json(&run_dir, &terminal, None).await?;
            return Ok(RunOutcome {
                terminal,
                exit_code: None,
            });
        }
        counts
    } else {
        AcquireCounts::default()
    };

    // ---------------------------------------------------------------- 2 段目: 索引と回答
    // ADR-0063 Phase 109d C1/C2: `pqa ask` CLI ではなく、埋め込みの python ランナー
    // （`paperqa_ask.py`）が PaperQA の Python API（`paperqa.ask`/`Settings`）を呼ぶ。
    let stdout_log_path = run_dir.join("stdout.log");
    let stderr_log_path = run_dir.join("stderr.log");

    let ask_script_path = run_dir.join("paperqa_ask.py");
    let ask_input_path = run_dir.join("ask_input.json");
    let ask_output_path = run_dir.join("ask_output.json");
    let (settings_path, settings_dir, settings_name) = config
        .settings
        .as_deref()
        .map(split_settings_path)
        .unwrap_or((None, None, None));
    // ADR-0063 Phase 109d C3: 対象が複数（比較先込み）でも `max_asks` を超えない。
    let comparison_target = crate::research_targets::comparison_target(&req.task.objective);
    // ADR-0063 Phase 109g A: 総括の問い（比較分類の判断）の材料。マッチングと本文の切り詰めは
    // `paperqa_ask.py` の純関数に任せ、ここでは材料（KB の root と索引、目的文の周辺 1 文）を渡すだけ
    // （知識ベースの本文はハーネス〈task-worker〉自身では読まない。ADR-0047 D2「索引だけ」の境界を守る）。
    let comparison_fallback_paragraph =
        crate::research_targets::comparison_target_paragraph(&req.task.objective);
    let knowledge_index_for_comparison: Vec<serde_json::Value> = req
        .context
        .knowledge
        .as_ref()
        .map(|k| {
            k.index
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "path": item.path,
                        "title": item.title,
                        "tags": item.tags,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let ask_input = serde_json::json!({
        "settings_name": settings_name,
        "settings_dir": settings_dir,
        // ADR-0063 Phase 109e: `paperqa_ask.py` がこれを直接読む（`Settings.from_name` は
        // `PQA_SETTINGS_DIR` を見ないので使えない。あればフォールバックとしてのみ使う）。
        "settings_path": settings_path,
        "paper_directory": paper_directory.to_string_lossy(),
        "index_directory": index_directory.to_string_lossy(),
        "index_name": index_name,
        "model": config.model,
        "targets": research_targets,
        "aspects": research_aspects,
        "comparison_target": comparison_target,
        // ADR-0063 Phase 109g A: `paperqa_ask.py::find_comparison_page_path` /
        // `comparison_design_context` が読む材料。
        "comparison_context": {
            "knowledge_root": config.knowledge_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "knowledge_index": knowledge_index_for_comparison,
            "fallback_paragraph": comparison_fallback_paragraph,
        },
        "max_asks": config.max_asks,
        "fallback_question": question,
        "output_path": ask_output_path.to_string_lossy(),
    });
    tokio::fs::write(&ask_script_path, ASK_SCRIPT).await?;
    let ask_input_text = serde_json::to_string_pretty(&ask_input)?;
    tokio::fs::write(&ask_input_path, format!("{ask_input_text}\n")).await?;

    let ask_command = resolve_ask_command(config, run_id);
    let mut command = Command::new(&ask_command);
    command
        .arg(&ask_script_path)
        .arg(&ask_input_path)
        .envs(config.env.iter().cloned())
        .current_dir(req.cwd())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    // ADR-0095 D2: 本番 DB を読み取り専用にした namespace で起動する（コンテナ非対応の adapter）。
    let mut command = crate::db_guard::launch(command, None);

    let child = command.spawn().map_err(AdapterError::Spawn)?;
    // ADR-0044 §5 Phase 53 追記（Phase 55）: この run のプロセスグループを覚える（`kill_tree` の入口）。
    let _process_group = crate::process_group::ProcessGroup::register(run_id, child.id());

    let streamed = stream_child(
        child,
        stdout_log_path,
        stderr_log_path,
        start,
        limits,
        run_id,
        sink,
        |line| {
            // `paperqa_ask.py` は `progress: <text>` を出す（ADR-0035 D5 の acquire ランナーと同じ流儀）。
            progress::emit_status(sink, &truncate_chars(line, PROGRESS_LINE_MAX_CHARS));
        },
    )
    .await?;

    if let Some(terminal) = streamed.timeout {
        // タイムアウトは供給側失敗として分類しない（他アダプタと同じ。ADR-0010 D5）。
        write_result_json(&run_dir, &terminal, None).await?;
        return Ok(RunOutcome {
            terminal,
            exit_code: streamed.exit.code(),
        });
    }

    let exit_status = streamed.exit;
    let stdout_buf = streamed.stdout;
    let stderr_tail = streamed.stderr_tail;
    let classify_text = format!("{stdout_buf}\n{stderr_tail}");
    let ask_output: Option<AskOutput> = match tokio::fs::read_to_string(&ask_output_path).await {
        Ok(text) => serde_json::from_str::<AskOutput>(&text).ok(),
        Err(_) => None,
    };
    // 成功と見なせる出力（`error` なし・答えが 1 件でも非空）だけを次に渡す。
    let usable_ask_output = ask_output
        .clone()
        .filter(|o| o.error.is_none() && o.answers.iter().any(|a| !a.answer.trim().is_empty()));

    let (terminal, provider_failure) = if exit_status.code()
        == Some(ASK_EXIT_PAPERQA_NOT_IMPORTABLE)
    {
        // ADR-0063 Phase 109d C1: `paperqa` が import できない (テスト環境や未整備の venv)。再試行しても
        // 直らないので `retryable: false` の分かりやすいエラーにする（一般の非 0 終了とは区別する）。
        (
            Terminal::Error {
                message: "paperqa (the Python package) is not importable in the environment \
                          used by [adapters.paperqa] command; install `paperqa` there \
                          (ADR-0063 Phase 109d C1)"
                    .to_string(),
                retryable: false,
            },
            None,
        )
    } else if exit_status.code() == Some(ASK_EXIT_BAD_USAGE_OR_SETTINGS_NOT_FOUND) {
        // ADR-0063 Phase 109e: 引数の誤り、または `settings_path`/`Settings.from_name` のどちらでも
        // 設定ファイルが見つからなかった（`paperqa_ask.py::load_settings`）。どちらも設定の誤りで
        // 再試行しても直らないので `retryable: false`。`output_path` の `error`（探したパスを列挙した
        // メッセージ）を優先し、無ければ stderr の最後の行を使う。
        let message = ask_output
            .as_ref()
            .and_then(|o| o.error.clone())
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                stderr_tail
                    .lines()
                    .next_back()
                    .unwrap_or("paperqa_ask.py: bad usage or settings not found")
                    .to_string()
            });
        (
            Terminal::Error {
                message: format!("paperqa_ask.py could not start (exit=2): {message}"),
                retryable: false,
            },
            None,
        )
    } else if !exit_status.success() {
        let exit_repr = match exit_status.code() {
            Some(code) => code.to_string(),
            None => "signal".to_string(),
        };
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: format!("paperqa_ask.py exited with a non-zero status (exit={exit_repr})"),
                retryable: true,
            },
            pf,
        )
    } else if let Some(ask_output) = usable_ask_output {
        // ADR-0027 D3 手順 4: アダプタが `artifacts/answer.md` と `artifacts/result.json` を書く。
        if let Err(e) = tokio::fs::create_dir_all(&artifacts_dir).await {
            warn!("run {run_id}: could not create artifacts/ directory: {e}");
        }

        // ADR-0063 Phase 109d C2: `cited` は全 `ask()` 呼び出しの `contexts`（実際に使った証拠）の
        // `docname`/`dockey` の和集合が主。本文一致（`answer_cites`、答え全体を連結したもの）は補助として
        // OR する（Phase 109b A2 までの仕組みの名残 -- モデルが引用マーカーを本文に残さない場合の保険）。
        let context_ids = context_identifiers(&ask_output.answers);
        let combined_answer_text: String = ask_output
            .answers
            .iter()
            .map(|a| a.answer.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let candidates = read_candidates(&artifacts_dir.join("papers.json")).await;
        let mut cited_from_contexts: u32 = 0;
        let marked: Vec<(Candidate, bool)> = candidates
            .into_iter()
            .map(|c| {
                let via_contexts = candidate_in_contexts(&context_ids, &c);
                if via_contexts {
                    cited_from_contexts += 1;
                }
                let cited = via_contexts || answer_cites(&combined_answer_text, &c);
                (c, cited)
            })
            .collect();
        // ADR-0063 D1: 本文からの引用とアブストのみの引用を区別する（`research.json` / `answer.md` の
        // 「証拠の質」節に内訳を書くため）。
        let cited_fulltext = marked
            .iter()
            .filter(|(c, cited)| *cited && c.pdf_downloaded)
            .count() as u32;
        let cited_abstract_only = marked
            .iter()
            .filter(|(c, cited)| *cited && c.abstract_only)
            .count() as u32;
        if acquiring {
            write_sources_json(&artifacts_dir.join("sources.json"), &marked, run_id).await;
        }

        // ADR-0063 D1: 決定的な証拠ゲート（LLM には判断させない）。取得の段を行わない構成では見ない。
        let evidence = if acquiring {
            Some(evidence_gate(
                &config.evidence,
                acquired,
                cited_fulltext,
                cited_abstract_only,
                cited_from_contexts,
            ))
        } else {
            None
        };
        if let Some(ev) = &evidence {
            write_research_json(
                &artifacts_dir.join("research.json"),
                &ev.summary,
                &research_targets,
                &research_aspects,
                &ask_output.dropped_targets,
                ask_output.comparison_page.as_deref(),
                ask_output.comparison_context_chars,
                run_id,
            )
            .await;
        }

        // ADR-0063 Phase 109d C4: 対象が取れていれば「# 対象別の整理」（表 + 対象ごとの節 + 総括）、
        // 取れていなければフォールバックの単一の答えをそのまま使う。
        let target_section = render_target_sections(
            &ask_output.answers,
            &ask_output.target_aspect_table,
            comparison_target.as_deref(),
        );
        let body_text = if target_section.is_empty() {
            ask_output
                .answers
                .first()
                .map(|a| a.answer.clone())
                .unwrap_or_default()
        } else {
            target_section
        };
        let contexts_section = render_contexts_section(&ask_output.answers);

        let answer_md = if acquiring {
            let mut body = format!(
                "{body_text}{contexts_section}{}",
                render_sources_section(&marked)
            );
            if let Some(ev) = &evidence {
                body.push_str(&render_evidence_section(
                    &ev.summary,
                    &ask_output.dropped_targets,
                ));
            }
            body.push_str(&render_primary_sources_section(&github_urls));
            body
        } else {
            format!("{body_text}{contexts_section}")
        };
        if let Err(e) = tokio::fs::write(artifacts_dir.join("answer.md"), &answer_md).await {
            warn!("run {run_id}: could not write artifacts/answer.md: {e}");
        }
        // ADR-0063 Phase 109b A3: 調査系タスクの成果物名を LDR（`report.md`）と揃える。CoS が
        // 受け入れ条件を `artifact_exists report.md` で書いても通るよう、`answer.md` と同じ内容を
        // `report.md` にも書く（両方残す。`answer.md` が PaperQA 独自の詳しい名前として引き続き主）。
        if let Err(e) = tokio::fs::write(artifacts_dir.join("report.md"), &answer_md).await {
            warn!("run {run_id}: could not write artifacts/report.md: {e}");
        }
        // 書いたものは celeris にも知らせる（run の成果物一覧と `Check::ArtifactExists` の解決に使われる）。
        // 他のアダプタではワーカー自身が `artifact` メッセージで申告するが、pqa は申告しないのでアダプタが行う。
        // ADR-0035 D3 / ADR-0063 D1: ゲートに落ちても成果物は残す（人が読めるように）ので、申告はゲートより前に行う。
        // ADR-0036 D4: 申告する `path` は workspace 相対のまま（`artifacts_dir` 基準で組む）。
        let mut to_register: Vec<(&str, String, &str)> = vec![
            (
                "answer.md",
                format!("{artifacts_rel}/answer.md"),
                "markdown",
            ),
            (
                "report.md",
                format!("{artifacts_rel}/report.md"),
                "markdown",
            ),
        ];
        if acquiring {
            to_register.push((
                "papers.json",
                format!("{artifacts_rel}/papers.json"),
                "json",
            ));
            to_register.push((
                "sources.json",
                format!("{artifacts_rel}/sources.json"),
                "json",
            ));
            // ADR-0035 D5: どの検索語で探したか（LLM の出力そのまま、落ちたなら落ちた理由も）。
            to_register.push((
                "queries.json",
                format!("{artifacts_rel}/queries.json"),
                "json",
            ));
            // ADR-0063 D1: 証拠の質の内訳。
            to_register.push((
                "research.json",
                format!("{artifacts_rel}/research.json"),
                "json",
            ));
        }
        for (name, rel_path, kind) in to_register {
            if !req.workspace.join(&rel_path).is_file() {
                continue;
            }
            match crate::artifact::resolve(&req.workspace, name, &rel_path, Some(kind)) {
                Ok(artifact) => sink.artifact(&artifact),
                Err(e) => warn!("run {run_id}: could not register {rel_path}: {e}"),
            }
        }

        let gate_message = evidence.as_ref().and_then(|ev| ev.error.clone());

        if let Some(message) = gate_message {
            // ADR-0031 D2 と同じ: retryable な `Terminal::Error`。供給側の失敗（`AdapterError`）にはしない。
            (
                Terminal::Error {
                    message,
                    retryable: true,
                },
                None,
            )
        } else {
            // 総括の問い（対象が取れているとき）があればそれを、無ければ最初の答えを要約の元にする。
            let summary_source = ask_output
                .answers
                .iter()
                .find(|a| a.id == "summary")
                .or_else(|| ask_output.answers.first())
                .map(|a| a.answer.as_str())
                .unwrap_or("");
            let summary = single_line_summary(summary_source, SUMMARY_MAX_CHARS);
            let result_file = serde_json::json!({ "summary": summary, "evidence": [] });
            match serde_json::to_string_pretty(&result_file) {
                Ok(text) => {
                    if let Err(e) =
                        tokio::fs::write(artifacts_dir.join("result.json"), format!("{text}\n"))
                            .await
                    {
                        warn!("run {run_id}: could not write artifacts/result.json: {e}");
                    }
                }
                Err(e) => warn!("run {run_id}: could not serialize artifacts/result.json: {e}"),
            }
            (
                Terminal::Done {
                    summary,
                    evidence: Vec::new(),
                    usage: None,
                },
                None,
            )
        }
    } else {
        let pf = classify_provider_failure(&classify_text);
        (
            Terminal::Error {
                message: "paperqa_ask.py produced no answer".to_string(),
                retryable: true,
            },
            pf,
        )
    };

    write_result_json(&run_dir, &terminal, provider_failure).await?;

    if let (Terminal::Error { message, .. }, Some(pf)) = (&terminal, provider_failure) {
        return Err(AdapterError::from_provider_failure(pf, message));
    }

    Ok(RunOutcome {
        terminal,
        exit_code: exit_status.code(),
    })
}

/// ADR-0035 D3 / ADR-0063 D1: 証拠の内訳を集計し、閾値未達をどう扱うか決める。
///
/// - 取得が 0 件（`acquired.candidates == 0`）は「検索経路の問題」を示す固有のメッセージで**常に**
///   hard error（`insufficient_is_error` に関わらず）。
/// - それ以外の閾値未達（`min_candidates` / `min_pdfs` / `min_cited`）は、`insufficient_is_error`
///   が `true` のときだけ hard error。既定（`false`）では `summary.insufficient = true` のまま
///   `error = None` を返し、呼び出し側は `Terminal::Done` にして reviewer / 受け入れ条件に委ねる
///   （`research.json` の `evidence` と `answer.md` の「証拠の質」節で内訳を残す）。
fn evidence_gate(
    thresholds: &PaperQaEvidence,
    acquired: AcquireCounts,
    cited_fulltext: u32,
    cited_abstract_only: u32,
    cited_from_contexts: u32,
) -> EvidenceGateResult {
    let cited = cited_fulltext + cited_abstract_only;
    let enabled =
        thresholds.min_candidates > 0 || thresholds.min_pdfs > 0 || thresholds.min_cited > 0;
    if !enabled {
        return EvidenceGateResult {
            error: None,
            summary: EvidenceSummary {
                cited,
                cited_fulltext,
                cited_abstract_only,
                cited_from_contexts,
                min_cited: thresholds.min_cited,
                insufficient: false,
            },
        };
    }
    if acquired.candidates == 0 {
        // 「論文が見つからなかった」と「検索経路が壊れている」を運用者が区別できるようにする
        // （ADR-0031 D2 と同じ理由）。
        return EvidenceGateResult {
            error: Some(
                "literature search returned nothing (possible network or API problem)".to_string(),
            ),
            summary: EvidenceSummary {
                cited,
                cited_fulltext,
                cited_abstract_only,
                cited_from_contexts,
                min_cited: thresholds.min_cited,
                insufficient: true,
            },
        };
    }
    let mut problems = Vec::new();
    if thresholds.min_candidates > 0 && acquired.candidates < thresholds.min_candidates {
        problems.push(format!(
            "candidates={} (min {})",
            acquired.candidates, thresholds.min_candidates
        ));
    }
    if thresholds.min_pdfs > 0 && acquired.pdfs < thresholds.min_pdfs {
        problems.push(format!(
            "pdfs={} (min {})",
            acquired.pdfs, thresholds.min_pdfs
        ));
    }
    if thresholds.min_cited > 0 && cited < thresholds.min_cited {
        problems.push(format!("cited={cited} (min {})", thresholds.min_cited));
    }
    let insufficient = !problems.is_empty();
    let error = if insufficient && thresholds.insufficient_is_error {
        Some(format!(
            "insufficient literature evidence: {}",
            problems.join(", ")
        ))
    } else {
        None
    };
    EvidenceGateResult {
        error,
        summary: EvidenceSummary {
            cited,
            cited_fulltext,
            cited_abstract_only,
            cited_from_contexts,
            min_cited: thresholds.min_cited,
            insufficient,
        },
    }
}

async fn read_candidates(path: &Path) -> Vec<Candidate> {
    let Ok(text) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<Candidate>>(&text).unwrap_or_default()
}

/// `artifacts/sources.json` を `cited` を決めた後の値で書き直す（LDR と同じ形。ADR-0031 D1 / ADR-0035 D2）。
async fn write_sources_json(path: &Path, marked: &[(Candidate, bool)], run_id: &str) {
    let sources: Vec<BTreeMap<&str, serde_json::Value>> = marked
        .iter()
        .map(|(candidate, cited)| {
            let url = if !candidate.url.is_empty() {
                &candidate.url
            } else {
                &candidate.pdf_url
            };
            BTreeMap::from([
                ("url", serde_json::Value::String(url.clone())),
                ("title", serde_json::Value::String(candidate.title.clone())),
                (
                    "engine",
                    serde_json::Value::String(candidate.source_engine.clone()),
                ),
                ("cited", serde_json::Value::Bool(*cited)),
            ])
        })
        .collect();
    match serde_json::to_string_pretty(&sources) {
        Ok(text) => {
            if let Err(e) = tokio::fs::write(path, format!("{text}\n")).await {
                warn!("run {run_id}: could not write artifacts/sources.json: {e}");
            }
        }
        Err(e) => warn!("run {run_id}: could not serialize artifacts/sources.json: {e}"),
    }
}

/// `artifacts/result.json` の `summary`: 改行・連続空白を単一の空白にたたみ（single-line-safe）、
/// 文字数で上限まで切り詰める。
fn single_line_summary(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max_chars)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests;
