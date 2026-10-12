use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use task_core::{
    route_extract::{AsOf, Subject, features_for_subject},
    store::SqliteStore,
};

use crate::MANIFEST_SCHEMA;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: String,
    pub version: String,
    pub cutoff: String,
    pub seed: u64,
    pub problems: Vec<Problem>,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Problem {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    pub created_at: String,
    pub genre: String,
    pub kind: String,
    pub command: String,
    pub split: String,
    pub stratum: String,
    pub subset_12: bool,
    pub head_commit: Option<String>,
    pub base_commit: Option<String>,
    pub features_sha256: String,
    pub features: Value,
    pub sha256: String,
}

#[derive(Clone)]
struct RawProblem {
    id: String,
    task_id: String,
    created_at: String,
    genre: String,
    kind: String,
    command: String,
    head: Option<String>,
    base: Option<String>,
}

pub fn create(
    db: &Path,
    out: &Path,
    cutoff: &str,
    seed: u64,
    version: &str,
) -> Result<Manifest, String> {
    let uri = format!("file:{}?mode=ro", db.display());
    let conn = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    let mut stmt=conn.prepare("SELECT w.id,w.task_id,w.created_at,w.kind,w.json,w.head_commit,w.base_commit,t.json FROM work_units w JOIN tasks t ON t.id=w.task_id WHERE w.status='done' AND t.status='done' ORDER BY w.id").map_err(|e|e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    for row in rows {
        let (id, task_id, created_at, kind, wjson, head, base, tjson) =
            row.map_err(|e| e.to_string())?;
        let w: Value = serde_json::from_str(&wjson).map_err(|e| e.to_string())?;
        let t: Value = serde_json::from_str(&tjson).map_err(|e| e.to_string())?;
        let command = w
            .get("checks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find_map(|c| c["cmd"].as_str().map(str::to_owned));
        let Some(command) = command else { continue };
        let genre = t["genre"].as_str().unwrap_or("coding").to_owned();
        if genre != "coding" {
            continue;
        }
        raw.push(RawProblem {
            id,
            task_id,
            created_at,
            genre,
            kind,
            command,
            head,
            base,
        });
    }
    if raw.is_empty() {
        return Err("no completed coding work units with command checks found".into());
    }
    let mut strata: BTreeMap<String, Vec<RawProblem>> = BTreeMap::new();
    for p in raw {
        strata
            .entry(format!("{}:{}", p.genre, p.kind))
            .or_default()
            .push(p);
    }
    for ps in strata.values_mut() {
        ps.sort_by_key(|p| seeded(seed, &p.id));
    }
    let selected = round_robin(&strata, 30);
    if selected.len() < 30 {
        return Err(format!(
            "only {} eligible problems; 30 required",
            selected.len()
        ));
    }
    let subset_ids: BTreeSet<String> = round_robin(
        &selected
            .iter()
            .cloned()
            .fold(BTreeMap::<String, Vec<RawProblem>>::new(), |mut m, p| {
                m.entry(format!("{}:{}", p.genre, p.kind))
                    .or_default()
                    .push(p);
                m
            }),
        12,
    )
    .into_iter()
    .map(|p| p.id)
    .collect();
    let (store, _) = SqliteStore::open_client(db).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    for p in selected {
        let split = if p.created_at.as_str() < cutoff {
            "dev"
        } else {
            "held-out"
        }
        .to_owned();
        let feat = features_for_subject(
            &store,
            Subject {
                kind: "work_unit".into(),
                id: p.id.clone(),
            },
            Some(AsOf::Timestamp(p.created_at.clone())),
            None,
        )
        .map_err(|e| e.to_string())?;
        let features = serde_json::to_value(feat).map_err(|e| e.to_string())?;
        let feature_hash = hash_value(&features)?;
        let mut problem = Problem {
            id: p.id.clone(),
            task_id: p.task_id,
            work_unit_id: p.id.clone(),
            created_at: p.created_at,
            genre: p.genre.clone(),
            kind: p.kind.clone(),
            command: p.command,
            split,
            stratum: format!("{}:{}", p.genre, p.kind),
            subset_12: subset_ids.contains(&p.id),
            head_commit: p.head,
            base_commit: p.base,
            features_sha256: feature_hash,
            features,
            sha256: String::new(),
        };
        problem.sha256 = hash_serializable(&problem)?;
        problems.push(problem);
    }
    problems.sort_by(|a, b| a.id.cmp(&b.id));
    let mut manifest = Manifest {
        schema: MANIFEST_SCHEMA.into(),
        version: version.into(),
        cutoff: cutoff.into(),
        seed,
        problems,
        sha256: String::new(),
    };
    manifest.sha256 = manifest_hash(&manifest)?;
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let path = out.join("manifest.json");
    if path.exists() {
        let prior: Manifest = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if prior.version == version && prior.sha256 != manifest.sha256 {
            return Err(format!(
                "manifest version {version} already exists with different sha256; choose a new --version"
            ));
        }
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(manifest)
}

fn round_robin(map: &BTreeMap<String, Vec<RawProblem>>, n: usize) -> Vec<RawProblem> {
    let mut out = Vec::new();
    let mut i = 0;
    while out.len() < n {
        let mut any = false;
        for v in map.values() {
            if let Some(p) = v.get(i) {
                out.push(p.clone());
                any = true;
                if out.len() == n {
                    break;
                }
            }
        }
        if !any {
            break;
        }
        i += 1
    }
    out
}
fn seeded(seed: u64, id: &str) -> String {
    let mut h = Sha256::new();
    h.update(seed.to_be_bytes());
    h.update(id.as_bytes());
    format!("{:x}", h.finalize())
}
fn hash_value(v: &Value) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|e| e.to_string())?)
    ))
}
fn hash_serializable<T: Serialize>(v: &T) -> Result<String, String> {
    hash_value(&serde_json::to_value(v).map_err(|e| e.to_string())?)
}
fn manifest_hash(m: &Manifest) -> Result<String, String> {
    let mut v = serde_json::to_value(m).map_err(|e| e.to_string())?;
    v.as_object_mut()
        .ok_or("manifest serialization is not an object")?
        .insert("sha256".into(), Value::String(String::new()));
    hash_value(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().expect("tempdir");
        let db = dir.path().join("fixture.sqlite3");
        let _store = SqliteStore::open(&db).expect("migrate fixture database");
        let conn = Connection::open(&db).expect("fixture connection");
        for i in 0..30 {
            let task = format!("task-{i}");
            let wu = format!("wu-{i}");
            let created = if i < 15 {
                format!("2026-09-{:02}T00:00:00Z", (i % 28) + 1)
            } else {
                format!("2026-10-{:02}T00:00:00Z", ((i - 15) % 26) + 5)
            };
            let task_json=serde_json::json!({"title":task,"objective":"fixture objective","genre":"coding","acceptance":[]}).to_string();
            conn.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES(?1,'done','task',NULL,0,?2,?3)",rusqlite::params![task,created,task_json]).expect("insert task");
            let wu_json=serde_json::json!({"objective":"fixture work","kind":if i%2==0{"implement"}else{"test"},"checks":[{"cmd":"true","expect_exit":0}]}).to_string();
            conn.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,json,created_at,updated_at,head_commit,base_commit) VALUES(?1,?2,'plan',?1,0,?3,'done','[]',?4,?5,?5,'head','base')",rusqlite::params![wu,task,if i%2==0{"implement"}else{"test"},wu_json,created]).expect("insert work unit");
        }
        drop(conn);
        (dir, db)
    }

    #[test]
    fn manifest_freezes_stratified_30_and_12_with_features_and_rejects_changed_sha() {
        let (_dir, db) = fixture();
        let out = tempdir().expect("output dir");
        let first = create(&db, out.path(), "2026-10-05", 77, "v1").expect("manifest");
        assert_eq!(first.problems.len(), 30);
        assert_eq!(first.problems.iter().filter(|p| p.subset_12).count(), 12);
        assert_eq!(
            first.problems.iter().filter(|p| p.split == "dev").count(),
            15
        );
        assert!(
            first
                .problems
                .iter()
                .all(|p| p.features["schema"] == "celeris.task-features/1"
                    && !p.features_sha256.is_empty())
        );
        assert!(
            first
                .problems
                .iter()
                .filter(|p| p.subset_12)
                .map(|p| p.stratum.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                == 2
        );
        assert_eq!(
            create(&db, out.path(), "2026-10-05", 77, "v1")
                .expect("stable rerun")
                .sha256,
            first.sha256
        );
        assert!(
            create(&db, out.path(), "2026-10-05", 78, "v1")
                .unwrap_err()
                .contains("choose a new --version")
        );
    }
}
