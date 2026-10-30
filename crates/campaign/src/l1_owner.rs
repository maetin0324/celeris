//! preflight の L1（評価器の別 uid）の持ち主の判定（ADR 2026-10-30-ops-r2-isolation-principal D3）。
//! open の拒否だけでは通さない。同じ uid の chmod 000 は open を EACCES にするが、持ち主は chmod で戻せる。
//! deny path のうち `--eval-root` の下のものについて、lstat で次を確かめる。
//! - EVAL_ROOT から path までの各要素: 持ち主が run の uid でも 0 でもない、run の uid から書けない、symlink でない
//! - EVAL_ROOT の祖先（/ まで）: 持ち主が run の uid でない、run の uid から書けない（sticky の dir は rename できないので可）、
//!   symlink でない
//!
//! EVAL_ROOT の下で lstat が ENOENT・EACCES になった所で歩みを止める（その親は上の条件を満たしているので、
//! run の uid はそこに作ることも覗くこともできない）。判定は `owner_findings`（run の uid と stat を引数に取る純粋関数）。
use std::{
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use crate::preflight_isolation::CHECK;

/// lstat の結果のうち判定に使う欄。`mode` は `st_mode`（種別の bit を含む）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;
const S_IFDIR: u32 = 0o040_000;
const S_ISVTX: u32 = 0o1000;

impl Stat {
    pub fn of(m: &fs::Metadata) -> Self {
        Self {
            uid: m.uid(),
            gid: m.gid(),
            mode: m.mode(),
        }
    }
    fn is_symlink(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
    fn sticky_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR && self.mode & S_ISVTX != 0
    }
}

/// preflight を走らせる process の実効 uid と所属 group（`/proc/self/status` の Uid・Gid・Groups）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunIds {
    pub uid: u32,
    pub gids: Vec<u32>,
}

impl RunIds {
    pub fn current() -> Result<Self, String> {
        let s = fs::read_to_string("/proc/self/status").map_err(|e| format!("run uid: {e}"))?;
        Self::parse_status(&s)
    }

    /// `/proc/<pid>/status` の本文から実効 uid と group を読む。
    pub fn parse_status(status: &str) -> Result<Self, String> {
        let field = |name: &str| {
            status
                .lines()
                .find_map(|l| l.strip_prefix(name))
                .map(|v| {
                    v.split_whitespace()
                        .map(str::parse::<u32>)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(|e| format!("run uid: {name} {e}"))
        };
        let uid = field("Uid:")?
            .and_then(|v| v.get(1).copied())
            .ok_or("run uid: no effective Uid in /proc/self/status")?;
        let mut gids = field("Gid:")?
            .map(|v| v.get(1).copied())
            .unwrap_or(None)
            .into_iter()
            .collect::<Vec<_>>();
        gids.extend(field("Groups:")?.unwrap_or_default());
        gids.sort_unstable();
        gids.dedup();
        Ok(Self { uid, gids })
    }

    /// run の uid がこの entry に書けるか（持ち主・group・その他の順で該当する 1 組の bit だけを見る）。
    fn can_write(&self, s: &Stat) -> bool {
        if self.uid == 0 {
            return true;
        }
        let bit = if s.uid == self.uid {
            0o200
        } else if self.gids.contains(&s.gid) {
            0o020
        } else {
            0o002
        };
        s.mode & bit != 0
    }
}

/// 1 つの要素の判定。`inside` は EVAL_ROOT とその下（root 持ちも拒否、sticky でも書ければ拒否）。
fn reason(run: &RunIds, s: &Stat, inside: bool) -> Option<&'static str> {
    if s.is_symlink() {
        return Some("symlink");
    }
    if s.uid == run.uid {
        return Some(if inside {
            "owned_by_run_uid"
        } else {
            "ancestor_owned_by_run_uid"
        });
    }
    if inside && s.uid == 0 {
        return Some("owned_by_root");
    }
    if run.can_write(s) && (inside || !s.sticky_dir()) {
        return Some(if inside {
            "writable_by_run_uid"
        } else {
            "ancestor_writable_by_run_uid"
        });
    }
    None
}

/// deny path 1 件の判定。満たさなければ `<reason> <要素> (uid <n>, mode <o>)` を返す。
fn path_reason(
    run: &RunIds,
    eval_root: &Path,
    path: &Path,
    stat: &dyn Fn(&Path) -> io::Result<Stat>,
) -> Option<String> {
    let Ok(rel) = path.strip_prefix(eval_root) else {
        return Some(format!("not_under_eval_root {}", eval_root.display()));
    };
    let explain = |why: &str, p: &Path, s: &Stat| {
        format!(
            "{why} {} (uid {}, mode {:o})",
            p.display(),
            s.uid,
            s.mode & 0o7777
        )
    };
    // EVAL_ROOT から path まで（理由の分かりやすい方を先に）。
    let mut cur = eval_root.to_path_buf();
    for (i, c) in std::iter::once(None)
        .chain(rel.components().map(Some))
        .enumerate()
    {
        if let Some(c) = c {
            cur.push(c);
        }
        match stat(&cur) {
            Ok(s) => {
                if let Some(why) = reason(run, &s, true) {
                    return Some(explain(why, &cur, &s));
                }
            }
            // 親は確かめ済み（run の uid の物でなく書けない）。run の uid は作ることも覗くこともできない。
            Err(e)
                if i > 0
                    && matches!(
                        e.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                    ) =>
            {
                break;
            }
            Err(e) => return Some(format!("stat_failed {}: {e}", cur.display())),
        }
    }
    // 祖先（/ まで）。stat できなければ確かめられないので止める。
    for a in eval_root.ancestors().skip(1) {
        match stat(a) {
            Ok(s) => {
                if let Some(why) = reason(run, &s, false) {
                    return Some(explain(why, a, &s));
                }
            }
            Err(e) => return Some(format!("stat_failed {}: {e}", a.display())),
        }
    }
    None
}

/// L1 の持ち主の判定（純粋関数）。`deny` は EVAL_ROOT の下の path。1 件ごとに
/// `deny_readable:L1:<path>:<理由>` を返す（空なら通過）。
pub fn owner_findings(
    run: &RunIds,
    eval_root: &Path,
    deny: &[PathBuf],
    stat: &dyn Fn(&Path) -> io::Result<Stat>,
) -> Vec<String> {
    deny.iter()
        .filter_map(|p| {
            path_reason(run, eval_root, p, stat)
                .map(|why| format!("{CHECK}:L1:{}:{why}", p.display()))
        })
        .collect()
}

/// 実 filesystem の lstat。
pub fn lstat(p: &Path) -> io::Result<Stat> {
    fs::symlink_metadata(p).map(|m| Stat::of(&m))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const DIR: u32 = S_IFDIR;
    const FILE: u32 = 0o100_000;
    const HUMAN: u32 = 1001;
    const EVAL: u32 = 2002;

    /// `/`・`/srv`・`/srv/eval`（EVAL_ROOT）・`/srv/eval/k4`・`/srv/eval/k4/private.json` の stat。
    fn tree(edit: impl FnOnce(&mut HashMap<PathBuf, Stat>)) -> HashMap<PathBuf, Stat> {
        let st = |uid, mode| Stat {
            uid,
            gid: uid,
            mode,
        };
        let mut m = HashMap::from([
            (PathBuf::from("/"), st(0, DIR | 0o755)),
            (PathBuf::from("/srv"), st(0, DIR | 0o755)),
            (PathBuf::from("/srv/eval"), st(EVAL, DIR | 0o700)),
            (PathBuf::from("/srv/eval/k4"), st(EVAL, DIR | 0o700)),
            (
                PathBuf::from("/srv/eval/k4/private.json"),
                st(EVAL, FILE | 0o600),
            ),
        ]);
        edit(&mut m);
        m
    }

    fn judge(run: &RunIds, m: &HashMap<PathBuf, Stat>) -> Vec<String> {
        let stat = |p: &Path| {
            m.get(p)
                .copied()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        };
        owner_findings(
            run,
            Path::new("/srv/eval"),
            &[PathBuf::from("/srv/eval/k4/private.json")],
            &stat,
        )
    }

    fn human() -> RunIds {
        RunIds {
            uid: HUMAN,
            gids: vec![HUMAN],
        }
    }

    fn one(run: &RunIds, m: &HashMap<PathBuf, Stat>, want: &str) {
        let f = judge(run, m);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(
            f[0].starts_with("deny_readable:L1:/srv/eval/k4/private.json:") && f[0].contains(want),
            "{f:?} !~ {want}"
        );
    }

    /// 評価器の別 uid が持つ 0700 の EVAL_ROOT は通る（人の uid でも、注入した別の run uid でも）。
    #[test]
    fn eval_root_of_another_uid_passes() {
        assert_eq!(judge(&human(), &tree(|_| {})), Vec::<String>::new());
        let injected = RunIds {
            uid: 4242,
            gids: vec![4242, 100],
        };
        assert!(judge(&injected, &tree(|_| {})).is_empty());
        // 祖先の sticky の world-writable dir（/tmp 型）は rename できないので通る。
        let m = tree(|m| {
            m.insert(
                "/srv".into(),
                Stat {
                    uid: 0,
                    gid: 0,
                    mode: DIR | 0o1777,
                },
            );
        });
        assert!(judge(&human(), &m).is_empty());
        // EVAL_ROOT の下で path が無い（親は別 uid・書けない）なら run の uid は作れないので通る。
        let m = tree(|m| {
            m.remove(Path::new("/srv/eval/k4/private.json"));
        });
        assert!(judge(&human(), &m).is_empty());
    }

    /// 同じ uid が持つ path を chmod 000 にしても（open は EACCES）、持ち主が run の uid なので止まる。
    #[test]
    fn same_uid_chmod_000_stops() {
        let m = tree(|m| {
            for (p, mode) in [
                ("/srv/eval", DIR),
                ("/srv/eval/k4", DIR),
                ("/srv/eval/k4/private.json", FILE),
            ] {
                m.insert(
                    p.into(),
                    Stat {
                        uid: HUMAN,
                        gid: HUMAN,
                        mode,
                    },
                );
            }
        });
        one(
            &human(),
            &m,
            "owned_by_run_uid /srv/eval (uid 1001, mode 0)",
        );
        // EVAL_ROOT は別 uid でも、下の file だけ同じ uid なら止まる。
        let m = tree(|m| {
            m.insert(
                "/srv/eval/k4/private.json".into(),
                Stat {
                    uid: HUMAN,
                    gid: HUMAN,
                    mode: FILE,
                },
            );
        });
        one(&human(), &m, "owned_by_run_uid /srv/eval/k4/private.json");
    }

    /// root 持ちの EVAL_ROOT・その下の dir は止まる（run が root なら全て止まる）。
    #[test]
    fn root_owned_eval_root_stops() {
        let m = tree(|m| {
            m.insert(
                "/srv/eval".into(),
                Stat {
                    uid: 0,
                    gid: 0,
                    mode: DIR | 0o700,
                },
            );
        });
        one(&human(), &m, "owned_by_root /srv/eval");
        let m = tree(|m| {
            m.insert(
                "/srv/eval/k4".into(),
                Stat {
                    uid: 0,
                    gid: 0,
                    mode: DIR | 0o700,
                },
            );
        });
        one(&human(), &m, "owned_by_root /srv/eval/k4");
        one(
            &RunIds {
                uid: 0,
                gids: vec![0],
            },
            &tree(|_| {}),
            "writable_by_run_uid /srv/eval",
        );
    }

    /// 祖先が run の uid の物、または run の uid から書ける（group・その他）なら止まる。
    #[test]
    fn writable_or_owned_ancestor_stops() {
        let m = tree(|m| {
            m.insert(
                "/srv".into(),
                Stat {
                    uid: HUMAN,
                    gid: HUMAN,
                    mode: DIR | 0o755,
                },
            );
        });
        one(&human(), &m, "ancestor_owned_by_run_uid /srv");
        let m = tree(|m| {
            m.insert(
                "/srv".into(),
                Stat {
                    uid: 0,
                    gid: 50,
                    mode: DIR | 0o775,
                },
            );
        });
        one(
            &RunIds {
                uid: HUMAN,
                gids: vec![HUMAN, 50],
            },
            &m,
            "ancestor_writable_by_run_uid /srv",
        );
        assert!(
            judge(&human(), &m).is_empty(),
            "group 50 に入っていなければ書けない"
        );
        let m = tree(|m| {
            m.insert(
                "/".into(),
                Stat {
                    uid: 0,
                    gid: 0,
                    mode: DIR | 0o757,
                },
            );
        });
        one(&human(), &m, "ancestor_writable_by_run_uid / ");
        // EVAL_ROOT の下では sticky でも書ければ止まる。
        let m = tree(|m| {
            m.insert(
                "/srv/eval/k4".into(),
                Stat {
                    uid: EVAL,
                    gid: EVAL,
                    mode: DIR | 0o1777,
                },
            );
        });
        one(&human(), &m, "writable_by_run_uid /srv/eval/k4");
    }

    /// symlink と stat できない祖先・EVAL_ROOT は止まる（確かめられないものは通さない）。
    #[test]
    fn symlinks_and_unstattable_entries_stop() {
        let m = tree(|m| {
            m.insert(
                "/srv".into(),
                Stat {
                    uid: 0,
                    gid: 0,
                    mode: S_IFLNK | 0o777,
                },
            );
        });
        one(&human(), &m, "symlink /srv");
        let m = tree(|m| {
            m.remove(Path::new("/srv"));
        });
        one(&human(), &m, "stat_failed /srv");
        let m = tree(|m| {
            m.remove(Path::new("/srv/eval"));
        });
        one(&human(), &m, "stat_failed /srv/eval");
    }

    #[test]
    fn run_ids_are_read_from_proc_status() {
        let s = "Name:\tx\nUid:\t1001\t1002\t1001\t1001\nGid:\t1001\t1003\t1001\t1001\nGroups:\t65534 27 \n";
        assert_eq!(
            RunIds::parse_status(s).unwrap(),
            RunIds {
                uid: 1002,
                gids: vec![27, 1003, 65534]
            }
        );
        assert!(RunIds::parse_status("Name:\tx\n").is_err());
        let me = RunIds::current().unwrap();
        assert_eq!(me.uid, fs::metadata("/proc/self").unwrap().uid());
    }
}
