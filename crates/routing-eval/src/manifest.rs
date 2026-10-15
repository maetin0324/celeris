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

use crate::{
    MANIFEST_SCHEMA,
    recheck::{Checker, ProblemRecheck, RECHECK_SCHEMA, Recheck, write_into_manifest},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: String,
    pub version: String,
    pub cutoff: String,
    pub seed: u64,
    pub problems: Vec<Problem>,
    /// refill（`--repo`）で引いて recheck で落とした候補（理由つき・凍結に含む）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refill_rejected: Vec<RefillRejected>,
    pub sha256: String,
}

/// refill で recheck に落ちた候補 1 件。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RefillRejected {
    pub id: String,
    pub stratum: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub base_commit_from_parent: bool,
}

/// refill の指定: 層ごとに seed 順で候補を引き、recheck で採用したものだけで埋める。
pub struct Refill<'a> {
    pub checker: &'a mut Checker,
    /// 1 層から引く候補の上限（尽きた層は抜ける）。
    pub max_draws_per_stratum: usize,
    /// 済んだ recheck の行（JSON Lines）。読んで再利用し、新しい行を足す（中断から再開できる）。
    pub cache: Option<&'a Path>,
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
    /// Worker-visible task description; required in frozen manifests.
    pub prompt: String,
    pub split: String,
    pub stratum: String,
    pub subset_12: bool,
    pub head_commit: Option<String>,
    pub base_commit: Option<String>,
    /// base_commit を head の親 commit で補った（repair WU は base を記録しない）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub base_commit_from_parent: bool,
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
    prompt: String,
    head: Option<String>,
    base: Option<String>,
}

pub fn create(
    db: &Path,
    out: &Path,
    cutoff: &str,
    seed: u64,
    version: &str,
    refill: Option<Refill<'_>>,
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
        let prompt = worker_prompt(&w, &t, &command);
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
            prompt,
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
    let (store, _) = SqliteStore::open_client(db).map_err(|e| e.to_string())?;
    let Some(refill) = refill else {
        let selected = round_robin(&strata, 30);
        if selected.len() < 30 {
            return Err(format!(
                "only {} eligible problems; 30 required",
                selected.len()
            ));
        }
        let mut problems = Vec::new();
        for p in selected {
            problems.push(to_problem(&store, p, cutoff, false)?);
        }
        let manifest = freeze(problems, Vec::new(), cutoff, seed, version)?;
        write_manifest(out, &manifest)?;
        return Ok(manifest);
    };
    let mut cache = load_cache(refill.cache)?;
    let mut drawn: BTreeMap<String, usize> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut rows = Vec::new();
    let mut rejected = Vec::new();
    while problems.len() < 30 {
        let mut any = false;
        for (stratum, pool) in &strata {
            if problems.len() == 30 {
                break;
            }
            let cursor = drawn.entry(stratum.clone()).or_default();
            while *cursor < pool.len().min(refill.max_draws_per_stratum) {
                let mut raw = pool[*cursor].clone();
                *cursor += 1;
                let mut from_parent = false;
                let needs_parent =
                    raw.kind == "repair" && raw.base.as_deref().is_none_or(|b| b.trim().is_empty());
                let parent = raw
                    .head
                    .as_deref()
                    .filter(|_| needs_parent)
                    .and_then(|h| refill.checker.parent_of(h));
                if let Some(parent) = parent {
                    raw.base = Some(parent);
                    from_parent = true;
                }
                let problem = to_problem(&store, raw, cutoff, from_parent)?;
                let row = match cache.iter().find(|r| same_check(r, &problem)) {
                    Some(r) => r.clone(),
                    None => {
                        let r = refill.checker.check(&problem);
                        append_cache(refill.cache, &r)?;
                        cache.push(r.clone());
                        r
                    }
                };
                if row.adopted {
                    problems.push(problem);
                    rows.push(row);
                    any = true;
                    break;
                }
                rejected.push(RefillRejected {
                    id: problem.id,
                    stratum: problem.stratum,
                    reason: row.reason.unwrap_or_default(),
                    base_commit_from_parent: from_parent,
                });
            }
        }
        if !any {
            break;
        }
    }
    if problems.len() < 30 {
        return Err(format!(
            "refill adopted only {} problems after {} rejections (max {} draws per stratum); 30 required",
            problems.len(),
            rejected.len(),
            refill.max_draws_per_stratum
        ));
    }
    rejected.sort_by(|a, b| a.id.cmp(&b.id));
    let manifest = freeze(problems, rejected, cutoff, seed, version)?;
    let path = write_manifest(out, &manifest)?;
    let mut rows: Vec<ProblemRecheck> = rows
        .into_iter()
        .map(|mut r| {
            r.subset_12 = manifest
                .problems
                .iter()
                .any(|p| p.id == r.id && p.subset_12);
            r
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    let record = Recheck {
        schema: RECHECK_SCHEMA.into(),
        manifest_sha256: manifest.sha256.clone(),
        repo: refill.checker.repo().display().to_string(),
        checked_at: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| e.to_string())?,
        timeout_s: refill.checker.timeout().as_secs(),
        adopted: rows.len(),
        excluded: 0,
        excluded_by_reason: BTreeMap::new(),
        problems: rows,
    };
    write_into_manifest(&path, &record)?;
    Ok(manifest)
}

fn same_check(row: &ProblemRecheck, p: &Problem) -> bool {
    row.id == p.id
        && row.command == p.command
        && row.base_commit.as_deref() == p.base_commit.as_deref().filter(|b| !b.trim().is_empty())
        && row.head_commit.as_deref() == p.head_commit.as_deref().filter(|h| !h.trim().is_empty())
}

fn load_cache(path: Option<&Path>) -> Result<Vec<ProblemRecheck>, String> {
    let Some(path) = path.filter(|p| p.exists()) else {
        return Ok(Vec::new());
    };
    fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

fn append_cache(path: Option<&Path>, row: &ProblemRecheck) -> Result<(), String> {
    use std::io::Write;
    let Some(path) = path else { return Ok(()) };
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let line = serde_json::to_string(row).map_err(|e| e.to_string())?;
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

fn to_problem(
    store: &SqliteStore,
    p: RawProblem,
    cutoff: &str,
    base_commit_from_parent: bool,
) -> Result<Problem, String> {
    let split = if p.created_at.as_str() < cutoff {
        "dev"
    } else {
        "held-out"
    }
    .to_owned();
    let feat = features_for_subject(
        store,
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
    Ok(Problem {
        id: p.id.clone(),
        task_id: p.task_id,
        work_unit_id: p.id.clone(),
        created_at: p.created_at,
        genre: p.genre.clone(),
        kind: p.kind.clone(),
        command: p.command,
        prompt: p.prompt,
        split,
        stratum: format!("{}:{}", p.genre, p.kind),
        subset_12: false,
        head_commit: p.head,
        base_commit: p.base,
        base_commit_from_parent,
        features_sha256: feature_hash,
        features,
        sha256: String::new(),
    })
}

/// 選んだ問題に subset_12 と sha256 を付けて凍結する（選んだ順に層の round robin）。
fn freeze(
    mut problems: Vec<Problem>,
    refill_rejected: Vec<RefillRejected>,
    cutoff: &str,
    seed: u64,
    version: &str,
) -> Result<Manifest, String> {
    let mut by_stratum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &problems {
        by_stratum
            .entry(p.stratum.clone())
            .or_default()
            .push(p.id.clone());
    }
    let subset_ids: BTreeSet<String> = round_robin(&by_stratum, 12).into_iter().collect();
    for problem in &mut problems {
        problem.subset_12 = subset_ids.contains(&problem.id);
        problem.sha256 = hash_serializable(&*problem)?;
    }
    problems.sort_by(|a, b| a.id.cmp(&b.id));
    let mut manifest = Manifest {
        schema: MANIFEST_SCHEMA.into(),
        version: version.into(),
        cutoff: cutoff.into(),
        seed,
        problems,
        refill_rejected,
        sha256: String::new(),
    };
    manifest.sha256 = manifest_hash(&manifest)?;
    Ok(manifest)
}

fn write_manifest(out: &Path, manifest: &Manifest) -> Result<std::path::PathBuf, String> {
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let path = out.join("manifest.json");
    if path.exists() {
        let prior: Manifest = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if prior.version == manifest.version && prior.sha256 != manifest.sha256 {
            return Err(format!(
                "manifest version {} already exists with different sha256; choose a new --version",
                manifest.version
            ));
        }
    }
    fs::write(
        &path,
        serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn worker_prompt(work_unit: &Value, task: &Value, check_command: &str) -> String {
    let mut sections = Vec::new();
    for (label, value) in [
        ("Parent task", task.get("title").and_then(Value::as_str)),
        (
            "Task objective",
            task.get("objective").and_then(Value::as_str),
        ),
        (
            "Work unit objective",
            work_unit.get("objective").and_then(Value::as_str),
        ),
        (
            "Definition of done",
            work_unit.get("done_when").and_then(Value::as_str),
        ),
    ] {
        if let Some(value) = value.map(str::trim).filter(|s| !s.is_empty()) {
            let safe = if check_command.is_empty() {
                value.to_owned()
            } else {
                value.replace(check_command, "[check command redacted]")
            };
            sections.push(format!("{label}: {safe}"));
        }
    }
    sections.join("\n\n")
}

fn round_robin<T: Clone>(map: &BTreeMap<String, Vec<T>>, n: usize) -> Vec<T> {
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
pub fn manifest_hash(m: &Manifest) -> Result<String, String> {
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
            let task_json=serde_json::json!({"title":task,"objective":"fixture objective","genre":"coding","acceptance":[{"text":"must not leak"}]}).to_string();
            conn.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES(?1,'done','task',NULL,0,?2,?3)",rusqlite::params![task,created,task_json]).expect("insert task");
            let wu_json=serde_json::json!({"objective":"fixture work","done_when":"true command is mentioned here","kind":if i%2==0{"implement"}else{"test"},"checks":[{"cmd":"true","expect_exit":0}]}).to_string();
            conn.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,json,created_at,updated_at,head_commit,base_commit) VALUES(?1,?2,'plan',?1,0,?3,'done','[]',?4,?5,?5,'head','base')",rusqlite::params![wu,task,if i%2==0{"implement"}else{"test"},wu_json,created]).expect("insert work unit");
        }
        drop(conn);
        (dir, db)
    }

    #[test]
    fn manifest_freezes_stratified_30_and_12_with_features_and_rejects_changed_sha() {
        let (_dir, db) = fixture();
        let out = tempdir().expect("output dir");
        let first = create(&db, out.path(), "2026-10-05", 77, "v1", None).expect("manifest");
        assert_eq!(first.problems.len(), 30);
        assert!(
            first
                .problems
                .iter()
                .all(|p| p.prompt.contains("fixture objective"))
        );
        assert!(first.problems.iter().all(|p| !p.prompt.contains("true")));
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
            create(&db, out.path(), "2026-10-05", 77, "v1", None)
                .expect("stable rerun")
                .sha256,
            first.sha256
        );
        assert!(
            create(&db, out.path(), "2026-10-05", 78, "v1", None)
                .unwrap_err()
                .contains("choose a new --version")
        );
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git is available");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// base（`flag.txt` が無い）と head（ある）の 2 commit を持つ一時リポジトリ。
    fn refill_repo(root: &Path) -> (std::path::PathBuf, String, String) {
        let repo = root.join("repo");
        let hooks = root.join("hooks");
        fs::create_dir_all(&repo).expect("mkdir repo");
        fs::create_dir_all(&hooks).expect("mkdir hooks");
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        git_ok(&repo, &["config", "user.email", "eval@example.com"]);
        git_ok(&repo, &["config", "user.name", "eval"]);
        git_ok(&repo, &["config", "commit.gpgsign", "false"]);
        git_ok(
            &repo,
            &["config", "core.hooksPath", hooks.to_str().expect("utf-8")],
        );
        fs::write(repo.join("readme.md"), b"base\n").expect("write");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "base"]);
        let base = git_ok(&repo, &["rev-parse", "HEAD"]);
        fs::write(repo.join("flag.txt"), b"head\n").expect("write");
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "head"]);
        let head = git_ok(&repo, &["rev-parse", "HEAD"]);
        (repo, base, head)
    }

    /// 36 候補: implement 12（判別できる）・repair 12（base 無し、親で補えば判別できる）・
    /// test 12（6 は base でも通る noop、6 は判別できる）。
    fn refill_fixture(dir: &Path, base: &str, head: &str) -> std::path::PathBuf {
        let db = dir.join("refill.sqlite3");
        let _store = SqliteStore::open(&db).expect("migrate fixture database");
        let conn = Connection::open(&db).expect("fixture connection");
        for i in 0..36 {
            let task = format!("task-{i:02}");
            let wu = format!("wu-{i:02}");
            let created = format!("2026-09-{:02}T00:00:00Z", (i % 28) + 1);
            let kind = ["implement", "repair", "test"][i % 3];
            let cmd = if kind == "test" && i < 18 {
                "true"
            } else {
                "test -f flag.txt"
            };
            let base_col = (kind != "repair").then_some(base);
            let task_json = serde_json::json!({"title": task, "objective": "fixture objective", "genre": "coding"}).to_string();
            conn.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES(?1,'done','task',NULL,0,?2,?3)", rusqlite::params![task, created, task_json]).expect("insert task");
            let wu_json = serde_json::json!({"objective": "fixture work", "kind": kind, "checks": [{"cmd": cmd, "expect_exit": 0}]}).to_string();
            conn.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,json,created_at,updated_at,head_commit,base_commit) VALUES(?1,?2,'plan',?1,0,?3,'done','[]',?4,?5,?5,?6,?7)", rusqlite::params![wu, task, kind, wu_json, created, head, base_col]).expect("insert work unit");
        }
        drop(conn);
        db
    }

    #[test]
    fn refill_fills_30_from_adopted_records_rejections_and_marks_parent_bases() {
        let dir = tempdir().expect("tempdir");
        let (repo, base, head) = refill_repo(dir.path());
        let db = refill_fixture(dir.path(), &base, &head);
        let out = dir.path().join("out");
        let cache = dir.path().join("cache.jsonl");
        let mut checker = crate::recheck::Checker::new(
            &repo,
            Some(&dir.path().join("wt")),
            std::time::Duration::from_secs(60),
        )
        .expect("checker");
        let refill = Refill {
            checker: &mut checker,
            max_draws_per_stratum: 12,
            cache: Some(&cache),
        };
        let m = create(&db, &out, "2026-10-05", 0, "v2", Some(refill)).expect("refill manifest");
        assert_eq!(m.problems.len(), 30);
        assert_eq!(m.problems.iter().filter(|p| p.subset_12).count(), 12);
        assert_eq!(m.refill_rejected.len(), 6);
        assert!(m.refill_rejected.iter().all(|r| r.stratum == "coding:test"
            && r.reason == crate::recheck::REASON_NOOP_BASE_PASSES
            && !r.base_commit_from_parent));
        let repairs: Vec<_> = m.problems.iter().filter(|p| p.kind == "repair").collect();
        assert_eq!(repairs.len(), 12);
        assert!(
            repairs
                .iter()
                .all(|p| p.base_commit_from_parent
                    && p.base_commit.as_deref() == Some(base.as_str()))
        );
        assert!(
            m.problems
                .iter()
                .filter(|p| p.kind != "repair")
                .all(|p| !p.base_commit_from_parent)
        );
        let written: Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).expect("read manifest"))
                .expect("manifest json");
        assert_eq!(written["recheck"]["adopted"], 30);
        assert_eq!(written["recheck"]["manifest_sha256"], m.sha256.as_str());
        let cached = fs::read_to_string(&cache).expect("read cache");
        assert_eq!(cached.lines().count(), 36);

        // 同じ cache で引き直すと git を走らせず同じ凍結値になる（中断からの再開）。
        let mut again = crate::recheck::Checker::new(
            &repo,
            Some(&dir.path().join("wt2")),
            std::time::Duration::from_secs(60),
        )
        .expect("checker");
        let rerun = create(
            &db,
            &out,
            "2026-10-05",
            0,
            "v2",
            Some(Refill {
                checker: &mut again,
                max_draws_per_stratum: 12,
                cache: Some(&cache),
            }),
        )
        .expect("rerun from cache");
        assert_eq!(rerun.sha256, m.sha256);
        assert_eq!(
            fs::read_to_string(&cache)
                .expect("read cache")
                .lines()
                .count(),
            36
        );
    }

    #[test]
    fn refill_reports_a_shortfall_instead_of_lowering_the_bar() {
        let dir = tempdir().expect("tempdir");
        let (repo, base, head) = refill_repo(dir.path());
        let db = refill_fixture(dir.path(), &base, &head);
        let mut checker = crate::recheck::Checker::new(
            &repo,
            Some(&dir.path().join("wt")),
            std::time::Duration::from_secs(60),
        )
        .expect("checker");
        let err = create(
            &db,
            &dir.path().join("out"),
            "2026-10-05",
            0,
            "v2",
            Some(Refill {
                checker: &mut checker,
                max_draws_per_stratum: 8,
                cache: None,
            }),
        )
        .unwrap_err();
        assert!(err.contains("30 required"), "{err}");
        assert!(!dir.path().join("out/manifest.json").exists());
    }

    #[test]
    fn old_manifest_without_prompt_is_rejected() {
        let old = r#"{"id":"p","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"task","command":"check","split":"dev","stratum":"coding:task","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}"#;
        assert!(serde_json::from_str::<Problem>(old).is_err());
    }
}
