//! `/proc/self/mountinfo` を読んでマウント点のファイルシステム種別・ソースを引くユーティリティ
//! （ADR-0064 D1）。`task-api`（`GET /health` の `db.filesystem` / `db.device`）が使う、決定的な純関数だけを置く（I/O はしない。呼び出し側が
//! `/proc/self/mountinfo` を読んで渡す）。

use std::path::Path;

/// マウント点のファイルシステム種別とマウントソース（`/proc/self/mountinfo` の `-` の後の
/// 1・2 番目のフィールド）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountInfo {
    pub fstype: String,
    /// マウントソース。ループマウントなら `/dev/loop0` のような値になる。
    pub source: String,
}

/// `/proc/self/mountinfo` の内容から、`target` を含む最長一致のマウント点の情報を返す。
/// `mountinfo` の各行は `... <mount_point> ... - <fstype> <source> <options>` の形
/// （`man 5 proc_pid_mountinfo`）。同じマウント点に複数行あれば後の行（実体）を優先する。
pub fn mount_info_for(mountinfo: &str, target: &Path) -> Option<MountInfo> {
    let mut best: Option<(usize, MountInfo)> = None;
    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let Some(mount_point) = before.split_whitespace().nth(4) else {
            continue;
        };
        let mut fields = after.split_whitespace();
        let Some(fstype) = fields.next() else {
            continue;
        };
        let Some(source) = fields.next() else {
            continue;
        };
        if target.starts_with(mount_point)
            && best
                .as_ref()
                .is_none_or(|(len, _)| mount_point.len() >= *len)
        {
            best = Some((
                mount_point.len(),
                MountInfo {
                    fstype: fstype.to_string(),
                    source: source.to_string(),
                },
            ));
        }
    }
    best.map(|(_, info)| info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_info_for_picks_the_longest_matching_mount_point() {
        let mountinfo = "\
25 30 0:24 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
26 25 0:52 / /home rw,relatime shared:2 - nfs4 server:/home rw,vers=4.2
27 26 0:53 / /home/u/local rw,relatime shared:3 - ext4 /dev/sdb1 rw";
        assert_eq!(
            mount_info_for(mountinfo, Path::new("/var/lib/celeris")),
            Some(MountInfo {
                fstype: "ext4".into(),
                source: "/dev/mapper/root".into()
            })
        );
        assert_eq!(
            mount_info_for(mountinfo, Path::new("/home/u/workspace")),
            Some(MountInfo {
                fstype: "nfs4".into(),
                source: "server:/home".into()
            })
        );
        assert_eq!(
            mount_info_for(mountinfo, Path::new("/home/u/local/db")),
            Some(MountInfo {
                fstype: "ext4".into(),
                source: "/dev/sdb1".into()
            })
        );
        assert_eq!(mount_info_for("garbage", Path::new("/home")), None);
    }

    #[test]
    fn mount_info_for_prefers_the_later_line_at_the_same_mount_point() {
        // 同じマウント点に autofs と実体が並ぶ場合は後の行（実体）を採る。
        let autofs_first = "\
25 30 0:24 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw
26 25 0:51 / /home rw,relatime shared:2 - autofs systemd-1 rw
27 25 0:52 / /home rw,relatime shared:3 - nfs4 server:/home rw,vers=4.2";
        assert_eq!(
            mount_info_for(autofs_first, Path::new("/home/u/x")).map(|i| i.fstype),
            Some("nfs4".to_string())
        );
    }
}
