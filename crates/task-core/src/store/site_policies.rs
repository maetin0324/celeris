//! ADR 2026-10-08-browser-prod-enablement D3: 表 `browser_site_policies`（migration 0060）。
//!
//! 管理者の site policy（ログインの exact origin・URL・selector）の正本。秘密は持たない。
//! 変更は同じ transaction で `browser_site_policy_events` に追記する（selector・URL は載せない）。
//! 形式検証（ADR-0110 D2）は書く前に呼び手（API・daemon の種）が行う。ここでは参照の整合
//! （grant・開いている wait から参照されている policy は消さない）だけを見る。

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};

/// 行がどこから来たか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSitePolicySource {
    /// API（web の `/browser/settings`）で書いた。
    Api,
    /// config.toml の `[[api.browser_site_policies]]` から空の DB に入れた種。
    Config,
}

impl BrowserSitePolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Config => "config",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "config" => Self::Config,
            _ => Self::Api,
        }
    }
}

/// site policy の中身（`policy_id` で一意。1 policy = 1 origin）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSitePolicy {
    pub policy_id: String,
    pub exact_origin: String,
    pub login_url: String,
    pub password_selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// ADR 2026-10-09 credential username / post-login D1: username 欄の selector（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// 同 D2: ログイン後の読み取りの opt-in（任意。無ければ観測停止のまま）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_login: Option<crate::browser_wait::PostLogin>,
    /// 同 付記 2026-10-10b: IdP の同意頁で controller が押す固定ボタン（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<crate::browser_wait::ConsentPolicy>,
}

/// 保存された site policy（中身＋出自と時刻）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserSitePolicyRecord {
    #[serde(flatten)]
    pub policy: BrowserSitePolicy,
    pub source: BrowserSitePolicySource,
    /// RFC3339。
    pub created_at: String,
    /// RFC3339。
    pub updated_at: String,
}

/// 消せない理由（参照元）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserSitePolicyReferences {
    /// `credential_policy_ids` にこの policy を持つ組織のノード id（昇順）。
    pub node_ids: Vec<String>,
    /// この policy を参照している未解決の browser wait の id（昇順）。
    pub wait_ids: Vec<String>,
}

impl BrowserSitePolicyReferences {
    pub fn is_empty(&self) -> bool {
        self.node_ids.is_empty() && self.wait_ids.is_empty()
    }
}

/// 削除の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserSitePolicyDelete {
    Deleted,
    NotFound,
    /// 参照されているので消さなかった。
    InUse(BrowserSitePolicyReferences),
}

const COLUMNS: &str = "policy_id, exact_origin, login_url, password_selector, submit_selector, \
                       source, created_at, updated_at, username_selector, post_login_json, consent_json";

fn read_row(r: &Row<'_>) -> rusqlite::Result<BrowserSitePolicyRecord> {
    let source: String = r.get(5)?;
    let post_login = r
        .get::<_, Option<String>>(9)?
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let consent = r
        .get::<_, Option<String>>(10)?
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(BrowserSitePolicyRecord {
        policy: BrowserSitePolicy {
            policy_id: r.get(0)?,
            exact_origin: r.get(1)?,
            login_url: r.get(2)?,
            password_selector: r.get(3)?,
            submit_selector: r.get(4)?,
            username_selector: r.get(8)?,
            post_login,
            consent,
        },
        source: BrowserSitePolicySource::parse(&source),
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
    })
}

fn load(conn: &Connection, policy_id: &str) -> Result<Option<BrowserSitePolicyRecord>, StoreError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM browser_site_policies WHERE policy_id = ?1"),
            params![policy_id],
            read_row,
        )
        .optional()?)
}

fn append_event(
    tx: &Transaction<'_>,
    policy_id: &str,
    ts: &str,
    actor: &str,
    op: &str,
    source: BrowserSitePolicySource,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO browser_site_policy_events (policy_id, ts, actor, op, source) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![policy_id, ts, actor, op, source.as_str()],
    )?;
    Ok(())
}

fn insert_or_replace(
    tx: &Transaction<'_>,
    policy: &BrowserSitePolicy,
    source: BrowserSitePolicySource,
    now: &str,
) -> Result<(), StoreError> {
    let post_login = policy
        .post_login
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let consent = policy
        .consent
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "INSERT INTO browser_site_policies (policy_id, exact_origin, login_url, password_selector, \
         submit_selector, source, created_at, updated_at, username_selector, post_login_json, \
         consent_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10) \
         ON CONFLICT(policy_id) DO UPDATE SET exact_origin = excluded.exact_origin, \
         login_url = excluded.login_url, password_selector = excluded.password_selector, \
         submit_selector = excluded.submit_selector, source = excluded.source, \
         updated_at = excluded.updated_at, username_selector = excluded.username_selector, \
         post_login_json = excluded.post_login_json, consent_json = excluded.consent_json",
        params![
            policy.policy_id,
            policy.exact_origin,
            policy.login_url,
            policy.password_selector,
            policy.submit_selector,
            source.as_str(),
            now,
            policy.username_selector,
            post_login,
            consent,
        ],
    )?;
    Ok(())
}

/// grant（組織のノードの `profile.browser.credential_policy_ids`）と未解決の wait からの参照。
fn references(
    conn: &Connection,
    policy_id: &str,
) -> Result<BrowserSitePolicyReferences, StoreError> {
    let mut node_ids = Vec::new();
    let mut stmt = conn.prepare("SELECT id, profile_json FROM org_nodes ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (id, profile) = row?;
        let Some(profile) = profile else { continue };
        let value: serde_json::Value = serde_json::from_str(&profile)?;
        let granted = value
            .pointer("/browser/credential_policy_ids")
            .and_then(|v| v.as_array())
            .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(policy_id)));
        if granted {
            node_ids.push(id);
        }
    }
    let mut stmt = conn.prepare(
        "SELECT wait_id FROM browser_waits WHERE credential_policy_id = ?1 \
         AND state IN ('pending', 'registered', 'approved') ORDER BY wait_id",
    )?;
    let wait_ids = stmt
        .query_map(params![policy_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BrowserSitePolicyReferences { node_ids, wait_ids })
}

impl SqliteStore {
    /// 全件（`policy_id` 昇順）。
    pub fn browser_site_policy_list(&self) -> Result<Vec<BrowserSitePolicyRecord>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM browser_site_policies ORDER BY policy_id"
        ))?;
        let rows = stmt
            .query_map([], read_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn browser_site_policy_get(
        &self,
        policy_id: &str,
    ) -> Result<Option<BrowserSitePolicyRecord>, StoreError> {
        let conn = self.lock()?;
        load(&conn, policy_id)
    }

    /// 作成または置換（`source = api`）。`created_at` は最初の作成時のまま。返り値の 2 つ目は新規作成か。
    pub fn browser_site_policy_upsert(
        &self,
        policy: &BrowserSitePolicy,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<(BrowserSitePolicyRecord, bool), StoreError> {
        let now = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let created = load(&tx, &policy.policy_id)?.is_none();
        insert_or_replace(&tx, policy, BrowserSitePolicySource::Api, &now)?;
        append_event(
            &tx,
            &policy.policy_id,
            &now,
            actor,
            "upsert",
            BrowserSitePolicySource::Api,
        )?;
        let stored = load(&tx, &policy.policy_id)?
            .ok_or_else(|| StoreError::Invalid("site policy vanished after upsert".into()))?;
        tx.commit()?;
        Ok((stored, created))
    }

    /// 参照が無ければ消す（読み切り→判定→削除→event を 1 つの IMMEDIATE transaction で）。
    pub fn browser_site_policy_delete(
        &self,
        policy_id: &str,
        actor: &str,
        now: OffsetDateTime,
    ) -> Result<BrowserSitePolicyDelete, StoreError> {
        let now = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(existing) = load(&tx, policy_id)? else {
            return Ok(BrowserSitePolicyDelete::NotFound);
        };
        let refs = references(&tx, policy_id)?;
        if !refs.is_empty() {
            return Ok(BrowserSitePolicyDelete::InUse(refs));
        }
        tx.execute(
            "DELETE FROM browser_site_policies WHERE policy_id = ?1",
            params![policy_id],
        )?;
        append_event(&tx, policy_id, &now, actor, "delete", existing.source)?;
        tx.commit()?;
        Ok(BrowserSitePolicyDelete::Deleted)
    }

    /// config の種を入れる: DB に同じ `policy_id` が無いものだけを `source = config` で入れ、入れた id を返す。
    /// DB にあれば DB が勝つ（内容が違っても書き換えない）。
    pub fn browser_site_policy_seed(
        &self,
        policies: &[BrowserSitePolicy],
        now: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError> {
        let now = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = Vec::new();
        for policy in policies {
            if load(&tx, &policy.policy_id)?.is_some() {
                continue;
            }
            insert_or_replace(&tx, policy, BrowserSitePolicySource::Config, &now)?;
            append_event(
                &tx,
                &policy.policy_id,
                &now,
                "config",
                "upsert",
                BrowserSitePolicySource::Config,
            )?;
            inserted.push(policy.policy_id.clone());
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// 変更の監査（`(policy_id, op, source, actor)`、追記順）。
    pub fn browser_site_policy_events(
        &self,
    ) -> Result<Vec<(String, String, String, String)>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT policy_id, op, source, actor FROM browser_site_policy_events ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(id: &str) -> BrowserSitePolicy {
        BrowserSitePolicy {
            policy_id: id.into(),
            exact_origin: "https://manaba.example".into(),
            login_url: "https://manaba.example/login".into(),
            password_selector: "#p".into(),
            submit_selector: None,
            username_selector: None,
            post_login: None,
            consent: None,
        }
    }

    fn insert_wait(store: &SqliteStore, wait_id: &str, state: &str) {
        let conn = store.lock().expect("lock");
        conn.execute(
            "INSERT INTO browser_waits (wait_id, task_id, run_id, session_id, reason, origin, purpose, \
             credential_policy_id, policy_revision, policy_hash, deadline, resume_key, version, state, \
             created_at) VALUES (?1, 't', 'r', 's', 'waiting_for_auth', 'https://manaba.example', 'login', \
             'manaba', 1, 'h', '2030-01-01T00:00:00Z', ?1, 1, ?2, '2030-01-01T00:00:00Z')",
            params![wait_id, state],
        )
        .expect("insert wait");
    }

    #[test]
    fn browser_site_policy_db_round_trips_username_selector_and_post_login() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(&dir.path().join("c.db")).expect("open");
        let now = OffsetDateTime::now_utc();
        let mut p = policy("manaba");
        store
            .browser_site_policy_upsert(&p, "admin", now)
            .expect("upsert");
        assert_eq!(
            store
                .browser_site_policy_get("manaba")
                .expect("get")
                .expect("row")
                .policy,
            p
        );
        p.username_selector = Some("#u".into());
        p.post_login = Some(crate::browser_wait::PostLogin {
            read_origins: vec!["https://lms.example".into()],
            actions: vec![
                crate::browser_wait::PostLoginAction::Snapshot,
                crate::browser_wait::PostLoginAction::Download,
            ],
        });
        p.consent = Some(crate::browser_wait::ConsentPolicy {
            selector: "input[name=_eventId_proceed]".into(),
            choice_selector: Some("#_shib_idp_doNotRememberConsent".into()),
        });
        let (stored, created) = store
            .browser_site_policy_upsert(&p, "admin", now)
            .expect("replace");
        assert!(!created);
        assert_eq!(stored.policy, p);
        assert_eq!(store.browser_site_policy_list().expect("list")[0].policy, p);
    }

    #[test]
    fn browser_site_policy_db_delete_refuses_open_wait_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SqliteStore::open(&dir.path().join("c.db")).expect("open");
        let now = OffsetDateTime::now_utc();
        store
            .browser_site_policy_upsert(&policy("manaba"), "admin", now)
            .expect("upsert");
        insert_wait(&store, "w-done", "denied");
        insert_wait(&store, "w-open", "pending");
        let outcome = store
            .browser_site_policy_delete("manaba", "admin", now)
            .expect("delete");
        assert_eq!(
            outcome,
            BrowserSitePolicyDelete::InUse(BrowserSitePolicyReferences {
                node_ids: vec![],
                wait_ids: vec!["w-open".into()],
            })
        );
        {
            let conn = store.lock().expect("lock");
            conn.execute(
                "UPDATE browser_waits SET state = 'expired' WHERE wait_id = 'w-open'",
                [],
            )
            .expect("update");
        }
        assert_eq!(
            store
                .browser_site_policy_delete("manaba", "admin", now)
                .expect("delete"),
            BrowserSitePolicyDelete::Deleted
        );
        assert_eq!(
            store
                .browser_site_policy_delete("manaba", "admin", now)
                .expect("delete"),
            BrowserSitePolicyDelete::NotFound
        );
        // 拒否した削除は event を残さない（upsert と delete の 2 件だけ）。
        let ops: Vec<String> = store
            .browser_site_policy_events()
            .expect("events")
            .into_iter()
            .map(|(_, op, _, _)| op)
            .collect();
        assert_eq!(ops, vec!["upsert", "delete"]);
    }
}
