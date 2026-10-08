//! ADR 2026-10-08-browser-prod-enablement D4: 起票・retry 時の最小 browser policy の自動付与と
//! retry での引き継ぎ。
//!
//! task の作成はすべて `insert_tx` を通る（`POST /tasks`・CoS の operations・delegate.json・followups・
//! 計画の子 task・retry）ので、付与はここ 1 か所で同じ transaction に行う。決定的な規則だけで、LLM は呼ばない。
//!
//! - `requirements.browser.allowed_domains` が無い・空なら付けない（grant 全体に広げない。ADR 2026-10-05）。
//! - 既に保存された policy があれば何もしない（人の `PUT /tasks/{id}/browser/policy` を上書きしない）。
//! - 承認方針は変えない: `approval_actions = []` で、`click`・`download`・`credential_use` は
//!   `BrowserAction::ALWAYS_APPROVED` により毎回承認のまま（standing approval は作らない）。
//! - 自動付与は task policy を作るだけで grant を広げない。実効 policy は run 時に
//!   `EffectiveBrowserPolicy::derive` が担当の grant と照らす。

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};

use super::{SqliteStore, StoreError};
use crate::browser::origin_covers;
use crate::{BrowserAction, BrowserDomainMode, BrowserTaskPolicy, Task, TaskId};

/// 自動付与した policy の `policy_id`（task ごとの表なので task 間で衝突しない）。
pub const AUTO_POLICY_ID: &str = "auto";

/// 最小 policy を作る純関数。`site_policies` は `(policy_id, exact_origin)`、`granted` は browser grant を
/// 持つ組織のノードのどれかの `credential_policy_ids` の和。`allowed_domains` が空なら `None`。
pub fn minimal_task_policy(
    allowed_domains: &[String],
    site_policies: &[(String, String)],
    granted: &BTreeSet<String>,
) -> Option<BrowserTaskPolicy> {
    if allowed_domains.is_empty() {
        return None;
    }
    let credential_policy_ids: Vec<String> = site_policies
        .iter()
        .filter(|(id, origin)| {
            granted.contains(id) && allowed_domains.iter().any(|d| origin_covers(d, origin))
        })
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut allowed_actions = BrowserAction::PHASE1.to_vec();
    if !credential_policy_ids.is_empty() {
        allowed_actions.push(BrowserAction::CredentialUse);
    }
    let policy = BrowserTaskPolicy {
        policy_id: AUTO_POLICY_ID.to_string(),
        revision: 1,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: Vec::new(),
        network_domains: allowed_domains.to_vec(),
        allowed_actions,
        approval_actions: Vec::new(),
        credential_policy_ids,
        artifact_policy_id: None,
    };
    // requirements は作成時に検証済みだが、形の崩れた policy は書かない（付けなければ D2 で止まる）。
    policy.validate().ok().map(|()| policy)
}

/// browser grant を持つノードの `credential_policy_ids` の和。
fn granted_credential_policy_ids(conn: &Connection) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT profile_json FROM org_nodes ORDER BY id")?;
    let rows = stmt.query_map([], |r| r.get::<_, Option<String>>(0))?;
    let mut ids = BTreeSet::new();
    for row in rows {
        let Some(profile) = row? else { continue };
        let value: serde_json::Value = serde_json::from_str(&profile)?;
        if let Some(list) = value
            .pointer("/browser/credential_policy_ids")
            .and_then(|v| v.as_array())
        {
            ids.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
        }
    }
    Ok(ids)
}

fn site_policy_origins(conn: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    let mut stmt = conn
        .prepare("SELECT policy_id, exact_origin FROM browser_site_policies ORDER BY policy_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(super) fn task_policy_get_tx(
    conn: &Connection,
    task_id: TaskId,
) -> Result<Option<BrowserTaskPolicy>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT policy_json FROM browser_task_policies WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|value| {
        BrowserTaskPolicy::from_json(&value).map_err(|e| StoreError::Invalid(e.code().into()))
    })
    .transpose()
}

/// `policy` を書く。`replace = false` なら既存の行を残す（人の PUT を上書きしない）。
fn write_policy_tx(
    conn: &Connection,
    task_id: TaskId,
    policy: &BrowserTaskPolicy,
    replace: bool,
) -> Result<(), StoreError> {
    let raw = serde_json::to_string(policy)?;
    let sql = if replace {
        "INSERT INTO browser_task_policies (task_id, policy_json) VALUES (?1, ?2)
         ON CONFLICT(task_id) DO UPDATE SET policy_json = excluded.policy_json"
    } else {
        "INSERT INTO browser_task_policies (task_id, policy_json) VALUES (?1, ?2)
         ON CONFLICT(task_id) DO NOTHING"
    };
    conn.execute(sql, params![task_id.to_string(), raw])?;
    Ok(())
}

impl SqliteStore {
    /// 作成直後の task に最小 policy を付ける（`insert_tx` から）。保存済みの policy があれば何もしない。
    pub(super) fn attach_auto_browser_policy_tx(
        conn: &Connection,
        task: &Task,
    ) -> Result<(), StoreError> {
        let Some(browser) = &task.requirements.browser else {
            return Ok(());
        };
        if browser.allowed_domains.is_empty() {
            return Ok(());
        }
        let policy = minimal_task_policy(
            &browser.allowed_domains,
            &site_policy_origins(conn)?,
            &granted_credential_policy_ids(conn)?,
        );
        match policy {
            Some(policy) => write_policy_tx(conn, task.id, &policy, false),
            None => Ok(()),
        }
    }

    /// retry: 元の task の保存 policy を新しい task に写す（`revision` は 1 に戻し、他は同じ）。
    /// 元に policy が無ければ `insert_tx` の自動付与がそのまま残る。
    pub(super) fn inherit_browser_policy_tx(
        conn: &Connection,
        original: TaskId,
        new_task: TaskId,
    ) -> Result<(), StoreError> {
        let Some(mut policy) = task_policy_get_tx(conn, original)? else {
            return Ok(());
        };
        policy.revision = 1;
        write_policy_tx(conn, new_task, &policy, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granted(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn browser_policy_autoattach_minimal_policy_rules() {
        let sites = vec![
            (
                "manaba".to_string(),
                "https://manaba.tsukuba.ac.jp".to_string(),
            ),
            ("other".to_string(), "https://example.org".to_string()),
            (
                "ungranted".to_string(),
                "https://manaba.tsukuba.ac.jp".to_string(),
            ),
        ];
        let domains = vec!["manaba.tsukuba.ac.jp".to_string()];
        let p =
            minimal_task_policy(&domains, &sites, &granted(&["manaba", "other"])).expect("policy");
        assert_eq!(p.policy_id, AUTO_POLICY_ID);
        assert_eq!(p.revision, 1);
        assert_eq!(p.network_domains, domains);
        assert_eq!(p.credential_policy_ids, vec!["manaba".to_string()]);
        assert!(p.allowed_actions.contains(&BrowserAction::CredentialUse));
        assert!(p.approval_actions.is_empty(), "承認方針を変えない");
        assert_eq!(p.artifact_policy_id, None);

        // grant に site policy が無ければ credential_use を付けない。
        let p = minimal_task_policy(&domains, &sites, &granted(&[])).expect("policy");
        assert!(p.credential_policy_ids.is_empty());
        assert_eq!(p.allowed_actions, BrowserAction::PHASE1.to_vec());

        // allowed_domains が空なら付けない。
        assert!(minimal_task_policy(&[], &sites, &granted(&["manaba"])).is_none());
    }
}
