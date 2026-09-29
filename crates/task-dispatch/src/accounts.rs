//! アカウントプールの純粋ロジック（ADR-0024 D1, D3, D4。ADR-0025 でアダプタの次元を追加）。
//!
//! ここは決定的なコードだけで構成する。LLM は呼ばない（DESIGN 原則 1）。`dispatcher.rs` への統合は別ステップで行う。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use task_core::{AccountAdapter, RateLimitObservation, RateWindow};

/// `^[A-Za-z0-9_-]{1,64}$`（先頭 `.` は文字集合に含まれないため自動的に拒否される。ADR-0024 D1）。
pub fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `accounts_dir` 直下の 1 ディレクトリ（ADR-0024 D1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDir {
    pub id: String,
    pub dir: PathBuf,
    pub logged_in: bool,
}

/// `root` の下の、有効な id を持つサブディレクトリを id 昇順で返す。`.` で始まる名前・ファイル・無効な id は飛ばす。
/// `root` が存在しなければ空を返す。ログイン済みの判定に使うファイル（`adapter.credentials_marker()`）の中身は
/// 読まない（存在だけを見る。ADR-0025 D1: claude-code は `.credentials.json`、codex は `auth.json`）。
pub fn scan_accounts(root: &Path, adapter: AccountAdapter) -> Vec<AccountDir> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    let marker = adapter.credentials_marker();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !valid_account_id(&name) {
            continue;
        }
        let logged_in = path.join(marker).exists();
        out.push(AccountDir {
            id: name,
            dir: path,
            logged_in,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// アカウントを cooldown にした理由（ADR-0024 D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCooldownReason {
    AuthFailed,
    Throttled,
    Exhausted,
}

/// アカウントの cooldown（Unix 秒の `until` まで選ばれない）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AccountCooldown {
    pub until: i64,
    pub reason: AccountCooldownReason,
}

/// 観測値の出所（ADR-0024 D4 / D6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    Run,
    Check,
}

/// D6 の手動確認の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountCheckRecord {
    pub at: i64,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// 1 アカウント分の観測値・cooldown・最終確認。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RateLimitObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ObservationSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown: Option<AccountCooldown>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<AccountCheckRecord>,
}

/// 永続化するファイルの形（`{"version":1,"accounts":{"<id>":{...}}}`）。
#[derive(Debug, Serialize, Deserialize)]
struct BookFile {
    version: u32,
    accounts: BTreeMap<String, AccountState>,
}

/// アカウントごとの観測値・cooldown・最終確認の帳簿（ADR-0024 D4）。タスクの真実ではないので replay の対象外。
///
/// **どのメソッドも自動では保存しない。** 呼び出し側が変更後に必要なら `save()` を呼ぶこと。
#[derive(Debug, Default)]
pub struct AccountBook {
    states: BTreeMap<String, AccountState>,
    path: Option<PathBuf>,
}

impl AccountBook {
    /// 保存先を持たない帳簿（テストや `celerisctl worker run` 向け）。`save` は no-op。
    pub fn new_in_memory() -> Self {
        Self {
            states: BTreeMap::new(),
            path: None,
        }
    }

    /// `path` から読む。ファイルが無ければ空から始める（`path` は覚えておき、以後の `save` で使う）。
    /// 読めない・壊れているときは warn を出し、空から始める（`path` は覚えたまま）。
    pub fn load(path: &Path) -> Self {
        let mut book = Self {
            states: BTreeMap::new(),
            path: Some(path.to_path_buf()),
        };
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<BookFile>(&text) {
                Ok(file) => book.states = file.accounts,
                Err(err) => {
                    tracing::warn!(path = %path.display(), error = %err, "account book: corrupt file, starting empty");
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "account book: unreadable file, starting empty");
            }
        }
        book
    }

    /// 観測値を記録する。`observed_at` が既存以上に新しいときに差し替える（N1: 同時刻なら新しく渡された方が勝つ）。
    pub fn record_observation(
        &mut self,
        id: &str,
        obs: RateLimitObservation,
        source: ObservationSource,
    ) {
        let state = self.states.entry(id.to_string()).or_default();
        let newer = state
            .usage
            .as_ref()
            .is_none_or(|u| obs.observed_at >= u.observed_at);
        if newer {
            state.usage = Some(obs);
            state.source = Some(source);
        }
    }

    /// D6 の確認結果を記録する（常に上書き。呼び出し側が最新の確認だけを渡す想定）。
    pub fn record_check(&mut self, id: &str, record: AccountCheckRecord) {
        let state = self.states.entry(id.to_string()).or_default();
        state.last_check = Some(record);
    }

    /// cooldown を設定する（N2）。`AuthFailed` は理由を最優先で上書きする（`until` は長い方を残す。GUI に
    /// 「再ログインが必要」を出し続けるため）。既存が未失効（`until > now`）の `AuthFailed` なら、
    /// `Throttled`/`Exhausted` を設定しようとしても理由は `AuthFailed` のまま `until` だけ伸ばす。
    /// それ以外は `until` が遅い方を残す。
    pub fn set_cooldown(&mut self, id: &str, cooldown: AccountCooldown, now: i64) {
        let state = self.states.entry(id.to_string()).or_default();
        let next = match state.cooldown {
            None => cooldown,
            Some(existing) if cooldown.reason == AccountCooldownReason::AuthFailed => {
                AccountCooldown {
                    until: cooldown.until.max(existing.until),
                    reason: AccountCooldownReason::AuthFailed,
                }
            }
            Some(existing)
                if existing.reason == AccountCooldownReason::AuthFailed && existing.until > now =>
            {
                AccountCooldown {
                    until: cooldown.until.max(existing.until),
                    reason: AccountCooldownReason::AuthFailed,
                }
            }
            Some(existing) if cooldown.until > existing.until => cooldown,
            Some(existing) => existing,
        };
        state.cooldown = Some(next);
    }

    /// `until <= now` の cooldown を消す（アカウント自体は残す）。
    pub fn clear_expired(&mut self, now: i64) {
        for state in self.states.values_mut() {
            if state.cooldown.as_ref().is_some_and(|c| c.until <= now) {
                state.cooldown = None;
            }
        }
    }

    /// 認証の再確認が成功したときの解除。新しい観測値による枯渇判定は evaluate が行う。
    pub fn clear_cooldown(&mut self, id: &str) {
        if let Some(state) = self.states.get_mut(id) {
            state.cooldown = None;
        }
    }

    pub fn clear_observation(&mut self, id: &str) {
        if let Some(state) = self.states.get_mut(id) {
            state.usage = None;
            state.source = None;
        }
    }

    pub fn state(&self, id: &str) -> Option<&AccountState> {
        self.states.get(id)
    }

    pub fn states(&self) -> &BTreeMap<String, AccountState> {
        &self.states
    }

    pub fn remove(&mut self, id: &str) -> Option<AccountState> {
        self.states.remove(id)
    }

    /// `<path>.tmp` に書いてから `path` へ rename する。in-memory（`path` 無し）なら no-op。
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = BookFile {
            version: 1,
            accounts: self.states.clone(),
        };
        let json = serde_json::to_vec_pretty(&file)?;
        let mut tmp_name = path.as_os_str().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path = PathBuf::from(tmp_name);
        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    }
}

/// 窓 1 つの実効使用率（ADR-0024 D3-1）: 観測があり `now < resets_at` ならその `utilization`、それ以外 0。
fn effective_utilization(w: RateWindow, now: i64) -> f64 {
    if now < w.resets_at {
        w.utilization
    } else {
        0.0
    }
}

/// 供給側失敗の cooldown 期限を決める（ADR-0024 D4）。
///
/// `AuthFailed` は常に `now + fallback_secs`。`Throttled` / `Exhausted` は、直近の観測に実効使用率
/// `EXHAUSTED_UTILIZATION` 以上の窓、または `status == "rejected"` かつ `resets_at > now` があれば、
/// それらのうち最も遅い `resets_at` を使う。無ければ `now + fallback_secs`。
pub fn cooldown_for_failure(
    state: Option<&AccountState>,
    reason: AccountCooldownReason,
    now: i64,
    fallback_secs: u64,
) -> AccountCooldown {
    let fallback_until = now + fallback_secs as i64;
    if matches!(reason, AccountCooldownReason::AuthFailed) {
        return AccountCooldown {
            until: fallback_until,
            reason,
        };
    }
    let mut latest: Option<i64> = None;
    let mut consider = |resets_at: i64| {
        latest = Some(latest.map_or(resets_at, |l| l.max(resets_at)));
    };
    if let Some(obs) = state.and_then(|s| s.usage.as_ref()) {
        for w in [obs.five_hour, obs.seven_day].into_iter().flatten() {
            if effective_utilization(w, now) >= EXHAUSTED_UTILIZATION {
                consider(w.resets_at);
            }
        }
        if obs.status.as_deref() == Some("rejected")
            && let Some(r) = obs.resets_at
            && r > now
        {
            consider(r);
        }
    }
    AccountCooldown {
        until: latest.unwrap_or(fallback_until),
        reason,
    }
}

/// 除外理由（ADR-0024 D3-2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludedReason {
    NotLoggedIn,
    AtCapacity,
    Cooldown,
    FiveHourExhausted,
    SevenDayExhausted,
    Rejected,
}

/// 選択の候補（ディスパッチャが `scan_accounts` と in_use の集計から作る）。
#[derive(Debug, Clone, Copy)]
pub struct AccountCandidate<'a> {
    pub id: &'a str,
    pub logged_in: bool,
    pub in_use: usize,
}

/// 1 アカウントの評価結果。`excluded` が無ければ `score` がある。
#[derive(Debug, Clone, PartialEq)]
pub struct AccountEvaluation {
    pub id: String,
    pub score: Option<f64>,
    pub excluded: Option<ExcludedReason>,
}

pub const FIVE_HOUR_SECS: i64 = 18_000;
pub const SEVEN_DAY_SECS: i64 = 604_800;
pub const EXHAUSTED_UTILIZATION: f64 = 0.97;
pub const IN_USE_PENALTY: f64 = 0.05;
pub const MIN_WEEK_FRACTION: f64 = 0.1;

/// 1 アカウントを評価する（ADR-0024 D3）。除外の判定順: 未ログイン → cooldown → 上限 → rejected →
/// 5 時間枠の枯渇 → 週次枠の枯渇。除外されなければスコアを計算する。
pub fn evaluate(
    c: &AccountCandidate<'_>,
    state: Option<&AccountState>,
    max_runs_per_account: usize,
    now: i64,
) -> AccountEvaluation {
    let id = c.id.to_string();
    let excluded = |reason: ExcludedReason| AccountEvaluation {
        id: id.clone(),
        score: None,
        excluded: Some(reason),
    };

    if !c.logged_in {
        return excluded(ExcludedReason::NotLoggedIn);
    }
    if state
        .and_then(|s| s.cooldown.as_ref())
        .is_some_and(|cd| cd.until > now)
    {
        return excluded(ExcludedReason::Cooldown);
    }
    if c.in_use >= max_runs_per_account {
        return excluded(ExcludedReason::AtCapacity);
    }
    let obs = state.and_then(|s| s.usage.as_ref());
    if let Some(o) = obs
        && o.status.as_deref() == Some("rejected")
    {
        let rejected = match o.resets_at {
            Some(r) => r > now,
            None => now - o.observed_at <= FIVE_HOUR_SECS,
        };
        if rejected {
            return excluded(ExcludedReason::Rejected);
        }
    }

    let five_hour = obs.and_then(|o| o.five_hour);
    let u5 = five_hour.map_or(0.0, |w| effective_utilization(w, now));
    if u5 >= EXHAUSTED_UTILIZATION {
        return excluded(ExcludedReason::FiveHourExhausted);
    }
    let seven_day = obs.and_then(|o| o.seven_day);
    let u7 = seven_day.map_or(0.0, |w| effective_utilization(w, now));
    if u7 >= EXHAUSTED_UTILIZATION {
        return excluded(ExcludedReason::SevenDayExhausted);
    }

    let score = if obs.is_none() {
        1.0 - IN_USE_PENALTY * c.in_use as f64
    } else {
        let h5 = 1.0 - u5;
        let h7 = match seven_day {
            // ADR-0024 D3: `h_7d = min(1, (1 − u_7d) / max(t_7d, 0.1))`。上限はクランプしない
            // （`resets_at` が 7 日を超えていれば `t7 > 1` になり、その分 `h7` は下がる。S4）。
            Some(w) if now < w.resets_at => {
                let t7 =
                    ((w.resets_at - now) as f64 / SEVEN_DAY_SECS as f64).max(MIN_WEEK_FRACTION);
                ((1.0 - u7) / t7).min(1.0)
            }
            _ => 1.0 - u7,
        };
        h5.min(h7) - IN_USE_PENALTY * c.in_use as f64
    };
    AccountEvaluation {
        id,
        score: Some(score),
        excluded: None,
    }
}

/// スコア最大のアカウントを選ぶ。同点は `in_use` の少ない方、次に `id` の昇順（ADR-0024 D3-4）。
/// 選べる候補が無ければ `None`。
pub fn select_account(
    cands: &[AccountCandidate<'_>],
    book: &AccountBook,
    max_runs_per_account: usize,
    now: i64,
) -> Option<String> {
    let mut best: Option<(String, f64, usize)> = None;
    for c in cands {
        let eval = evaluate(c, book.state(c.id), max_runs_per_account, now);
        let Some(score) = eval.score else { continue };
        let take = match &best {
            None => true,
            Some((best_id, best_score, best_in_use)) => match score
                .partial_cmp(best_score)
                .unwrap_or(std::cmp::Ordering::Equal)
            {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => match c.in_use.cmp(best_in_use) {
                    std::cmp::Ordering::Less => true,
                    std::cmp::Ordering::Greater => false,
                    std::cmp::Ordering::Equal => c.id < best_id.as_str(),
                },
            },
        };
        if take {
            best = Some((c.id.to_string(), score, c.in_use));
        }
    }
    best.map(|(id, _, _)| id)
}

#[cfg(test)]
mod tests;

/// ADR-0053 D4.1（Phase 66/F3）: 1 枠だけの残り（`measured_remaining` と同じ「観測が新しく、枠が
/// 有効」という規律を 1 枠に適用する）。観測が古い（300 秒超）・未来（壁時計のずれ）・枠が無い／
/// 期限切れ／範囲外の `utilization` はすべて `None`（測れない、を捏造しない）。
///
/// ADR-0074 D4.1（Phase F3）: `llm-proxy::sources_view` にあった同名の私的関数をここに移した
/// （`dispatcher` と `GET /llm/sources` の両方が同じ関数で読む。値を捏造しない規律を 1 か所にする）。
pub fn window_remaining(
    obs: &RateLimitObservation,
    now: i64,
    window: Option<RateWindow>,
) -> Option<f64> {
    if now < obs.observed_at || now - obs.observed_at > 300 {
        return None;
    }
    let w = window?;
    if w.resets_at <= now || !w.utilization.is_finite() || !(0.0..=1.0).contains(&w.utilization) {
        return None;
    }
    Some(1.0 - w.utilization)
}

/// Conservative quota evidence: require both unexpired windows and a recent observation.
/// Missing/expired/stale values are unknown, never estimated as a full allowance.
pub fn measured_remaining(obs: &RateLimitObservation, now: i64) -> Option<f64> {
    if now < obs.observed_at || now - obs.observed_at > 300 {
        return None;
    }
    let windows = [obs.five_hour?, obs.seven_day?];
    if windows.iter().any(|w| {
        w.resets_at <= now || !w.utilization.is_finite() || !(0.0..=1.0).contains(&w.utilization)
    }) {
        return None;
    }
    Some(1.0 - windows.iter().map(|w| w.utilization).fold(0.0, f64::max))
}

#[cfg(test)]
mod routing_tests;

// ---------------------------------------------------------------------------
// ADR-0074 D4（Phase F3 quota）: run の前後の観測を quota 消費の推定に使うための、アカウントの
// 使用状況の追跡。`AccountBook` の観測値そのものとは別軸（こちらは「誰が今そのアカウントを
// 使っているか」という揮発的な状態機械で、replay の対象外）。
// ---------------------------------------------------------------------------

use std::collections::BTreeSet;
use std::collections::VecDeque;

/// D4.2:「重なった run の集合」1 メンバー（run が終わった時点で確定する情報。`QuotaActivity::end`
/// の引数）。
#[derive(Debug, Clone)]
pub struct QuotaGroupMember {
    pub task_id: task_core::TaskId,
    pub run_id: String,
    pub work_unit_id: Option<String>,
    pub source: String,
    pub weighted_tokens: f64,
    /// 参考の定価 USD（`Usage.cost_usd`）。グループが閉じたときに他タスクへ書く
    /// `Event::QuotaEstimated.list_price_usd` に使う（この run の `Usage` はもう手元に無いため、
    /// `begin`/`end` の時点で持っていた値をここに保存しておく）。
    pub list_price_usd: Option<f64>,
}

/// ADR-0053/ADR-0074: `AccountAdapter` を quota の `source` 文字列に写す
/// （`llm-proxy::server::source_label` と同じ語彙。プールを使うのはこの 2 つだけ）。
pub fn quota_source_label(adapter: AccountAdapter) -> &'static str {
    match adapter {
        AccountAdapter::ClaudeCode => "claude-oauth",
        AccountAdapter::Codex => "codex-oauth",
    }
}

#[derive(Debug, Clone)]
struct QuotaGroup {
    before: Option<RateLimitObservation>,
    before_valid: bool,
    open_run_ids: BTreeSet<String>,
    finished: Vec<QuotaGroupMember>,
    ever_multi: bool,
}

/// `QuotaActivity::end` の結果（D4.2 手順 1/2 のどちらを試すべきかを呼び出し側に伝える）。
#[derive(Debug, Clone)]
pub enum QuotaEndOutcome {
    /// この run は開始から終了まで、同じアカウントを使う他の run と一度も重ならなかった
    /// （`measured` の対象になりうる。`before`/`before_valid` はグループ開始〈= この run の
    /// 開始〉時点のスナップショット）。
    Exclusive {
        before: Option<RateLimitObservation>,
        before_valid: bool,
    },
    /// 重なりがあった。グループはまだ閉じていない（他に走っている run がある。D4.2 手順 2 の
    /// 「集合の最後の run が終わった時点で決まる。それまでは pending」）。
    Pending,
    /// 重なりがあり、この run でグループが閉じた（全メンバーの重み付きトークンで按分できる）。
    Closed {
        before: Option<RateLimitObservation>,
        before_valid: bool,
        members: Vec<QuotaGroupMember>,
    },
}

/// D4.2 手順 1/2 の入力を決めるための、アカウントの使用中 run の追跡（決定的な状態機械）。
/// **どのメソッドも保存しない**（`AccountBook` と違い、揮発データ。プロセスの再起動で失われても、
/// D4.2 の優先順位により次の run は `estimated`/`unknown` に倒れるだけで、値を捏造しない）。
#[derive(Debug, Default)]
pub struct QuotaActivity {
    groups: std::collections::HashMap<(AccountAdapter, String), QuotaGroup>,
}

impl QuotaActivity {
    pub fn new() -> Self {
        Self::default()
    }

    /// `run_id` がこのアカウントで `begin` 済み（まだ `end` していない）かどうか。
    /// `begin` を呼ばなかった run（例: planner run。`Dispatcher::quota_begin` の doc 参照）に対して
    /// 誤って `end` を呼び、無関係な他の run のグループを壊さないための、呼び出し側の防御に使う。
    pub fn is_tracked(&self, adapter: AccountAdapter, account_id: &str, run_id: &str) -> bool {
        self.groups
            .get(&(adapter, account_id.to_string()))
            .is_some_and(|g| g.open_run_ids.contains(run_id))
    }

    /// run の開始。`snapshot` はこの時点で `AccountBook` から読んだ観測値、`snapshot_valid` は
    /// D4.1 の有効性（`task_core::quota::before_is_valid`）。同じアカウントで既に開いている
    /// グループがあれば、それに参加する（そのグループの `before` は最初の run のものを保つ）。
    pub fn begin(
        &mut self,
        adapter: AccountAdapter,
        account_id: &str,
        run_id: &str,
        snapshot: Option<RateLimitObservation>,
        snapshot_valid: bool,
    ) {
        let key = (adapter, account_id.to_string());
        let group = self.groups.entry(key).or_insert_with(|| QuotaGroup {
            before: snapshot,
            before_valid: snapshot_valid,
            open_run_ids: BTreeSet::new(),
            finished: Vec::new(),
            ever_multi: false,
        });
        group.open_run_ids.insert(run_id.to_string());
        if group.open_run_ids.len() > 1 {
            group.ever_multi = true;
        }
    }

    /// run の終了。`member.run_id` は `begin` に渡したものと一致させること。`begin` を呼んでいない
    /// run（呼び出し側の不具合。通常は起きない）は `Exclusive { before: None, before_valid: false }`
    /// を返す（unknown に落ちるだけで、値を捏造しない）。
    pub fn end(
        &mut self,
        adapter: AccountAdapter,
        account_id: &str,
        member: QuotaGroupMember,
    ) -> QuotaEndOutcome {
        let key = (adapter, account_id.to_string());
        let Some(group) = self.groups.get_mut(&key) else {
            return QuotaEndOutcome::Exclusive {
                before: None,
                before_valid: false,
            };
        };
        group.open_run_ids.remove(&member.run_id);
        if !group.ever_multi && group.open_run_ids.is_empty() {
            let group = self
                .groups
                .remove(&key)
                .expect("key was just matched above");
            return QuotaEndOutcome::Exclusive {
                before: group.before,
                before_valid: group.before_valid,
            };
        }
        group.finished.push(member);
        if group.open_run_ids.is_empty() {
            let group = self
                .groups
                .remove(&key)
                .expect("key was just matched above");
            QuotaEndOutcome::Closed {
                before: group.before,
                before_valid: group.before_valid,
                members: group.finished,
            }
        } else {
            QuotaEndOutcome::Pending
        }
    }
}

/// D4.2 手順 3 の較正材料: `(source, window)` ごとの直近 `measured` の `(used_pct, weighted_tokens)`
/// の組（最大 `task_core::quota::CALIBRATION_MAX_SAMPLES` 件のリングバッファ）。
///
/// **ADR からの逸脱**（`docs/adr/0074-...md` の「Phase F3（quota）実装時の逸脱・明確化」参照）:
/// D4.2 は「同じ source の直近 14 日の `measured` run（最大 20 件）」と書いているが、events を
/// 横断して探す store 側の問い合わせは今回作らず、dispatcher プロセスの寿命の間だけ持つ
/// in-memory のリングバッファ（件数の上限だけを守る。日数の上限は無い）にした。再起動すれば
/// 較正はやり直しになり、しばらく `unknown` に倒れる（値を捏造しない、という規律の範囲内）。
#[derive(Debug, Default)]
pub struct QuotaCalibrationBook {
    by_key: BTreeMap<String, VecDeque<(f64, f64)>>,
}

fn calibration_key(source: &str, window: task_core::QuotaWindow) -> String {
    format!("{source}:{}", window.as_str())
}

impl QuotaCalibrationBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(
        &mut self,
        source: &str,
        window: task_core::QuotaWindow,
        used_pct: f64,
        weighted_tokens: f64,
    ) {
        if weighted_tokens <= 0.0 {
            return;
        }
        let entry = self
            .by_key
            .entry(calibration_key(source, window))
            .or_default();
        entry.push_back((used_pct, weighted_tokens));
        while entry.len() > task_core::quota::CALIBRATION_MAX_SAMPLES {
            entry.pop_front();
        }
    }

    pub fn calibration(
        &self,
        source: &str,
        window: task_core::QuotaWindow,
    ) -> Option<task_core::QuotaCalibration> {
        let samples = self.by_key.get(&calibration_key(source, window))?;
        let pairs: Vec<(f64, f64)> = samples.iter().copied().collect();
        task_core::quota::calibrate(&pairs)
    }
}

#[cfg(test)]
mod quota_activity_tests;

#[cfg(test)]
mod quota_calibration_tests;
