// src/bin/dudb/folder_pack.rs
//
// Turns the (folder_id, name)-sorted record stream of the raw pass into one
// `folder_files` row per folder (see `dutopia::filepack`): totals of all its
// files per (user, age), plus a list of the few worth acting on — the `cap`
// largest per (user, age) of at least `min_size` bytes.
//
// Capping per (user, age) instead of per folder keeps every filtered listing
// exact: a file in the top L (L <= cap) of any user/age filter is also in the
// top L of its own (user, age) group, so it was kept.
use crate::extsort::FileRec;
use anyhow::Result;
use dutopia::filepack::{GroupTotals, PackedFile, PackedFolder, Packer};
use rusqlite::{params, Connection, Statement};
use std::cmp::Ordering;
use std::collections::HashMap;

/// What goes into the per-folder list.
#[derive(Debug, Clone, Copy)]
pub struct ListRule {
    /// Largest files kept per (folder, user, age); 0 = no limit.
    pub cap: usize,
    /// Smaller files are only counted in the totals.
    pub min_size: u64,
}

/// Largest first; names break ties so the kept set is deterministic.
fn largest_first(a: &PackedFile, b: &PackedFile) -> Ordering {
    b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
}

/// Keep the `cap` largest of `v` (0 = keep all), in no particular order.
fn keep_largest(v: &mut Vec<PackedFile>, cap: usize) {
    if cap > 0 && v.len() > cap {
        v.select_nth_unstable_by(cap - 1, largest_first);
        v.truncate(cap);
    }
}

#[derive(Default)]
struct Group {
    totals: GroupTotals,
    /// Bounded to 2 × cap while streaming, so a folder with millions of
    /// files never sits in memory whole.
    listed: Vec<PackedFile>,
}

struct FolderFiles {
    folder_id: i64,
    groups: HashMap<(u32, u8), Group>,
}

impl FolderFiles {
    fn push(&mut self, r: &FileRec, rule: ListRule) {
        let m = &r.meta;
        let g = self.groups.entry((m.user_id, m.age)).or_default();
        let t = &mut g.totals;
        if t.count == 0 {
            *t = GroupTotals { user_id: m.user_id, age: m.age, atime: m.atime, mtime: m.mtime, ..*t };
        }
        t.count += 1;
        t.bytes = t.bytes.saturating_add(m.size);
        t.atime = t.atime.max(m.atime);
        t.mtime = t.mtime.max(m.mtime);
        if m.size < rule.min_size {
            return;
        }
        g.listed.push(PackedFile {
            // Already valid UTF-8 (lossy-converted when parsed).
            name: String::from_utf8_lossy(&r.name).into_owned(),
            user_id: m.user_id,
            age: m.age,
            size: m.size,
            atime: m.atime,
            mtime: m.mtime,
        });
        if rule.cap > 0 && g.listed.len() >= 2 * rule.cap {
            keep_largest(&mut g.listed, rule.cap);
        }
    }

    fn into_packed(self, cap: usize) -> PackedFolder {
        let mut out = PackedFolder::default();
        for (_, mut g) in self.groups {
            keep_largest(&mut g.listed, cap);
            out.files.append(&mut g.listed);
            out.totals.push(g.totals);
        }
        out.totals.sort_unstable_by_key(|t| (t.user_id, t.age));
        out.files.sort_unstable_by(largest_first);
        out
    }
}

/// Streams records in folder order and writes one row per folder.
pub struct FolderWriter<'c> {
    insert: Statement<'c>,
    packer: Packer,
    rule: ListRule,
    current: Option<FolderFiles>,
    pub files_seen: u64,
    pub files_listed: u64,
    pub folders: u64,
}

impl<'c> FolderWriter<'c> {
    pub fn new(conn: &'c Connection, rule: ListRule) -> Result<Self> {
        Ok(Self {
            insert: conn.prepare("INSERT INTO folder_files(folder_id, data) VALUES (?1, ?2)")?,
            packer: Packer::new()?,
            rule,
            current: None,
            files_seen: 0,
            files_listed: 0,
            folders: 0,
        })
    }

    /// Records must arrive grouped by folder (merge_runs emits them sorted).
    pub fn push(&mut self, r: &FileRec) -> Result<()> {
        if self.current.as_ref().is_some_and(|f| f.folder_id != r.folder_id) {
            self.flush()?;
        }
        let folder = self.current.get_or_insert_with(|| FolderFiles {
            folder_id: r.folder_id,
            groups: HashMap::new(),
        });
        folder.push(r, self.rule);
        self.files_seen += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Self> {
        self.flush()?;
        Ok(self)
    }

    fn flush(&mut self) -> Result<()> {
        let Some(folder) = self.current.take() else { return Ok(()) };
        let folder_id = folder.folder_id;
        let packed = folder.into_packed(self.rule.cap);
        let blob = self.packer.pack(&packed)?;
        self.insert.execute(params![folder_id, blob])?;
        self.files_listed += packed.files.len() as u64;
        self.folders += 1;
        Ok(())
    }
}

#[cfg(test)]
#[path = "folder_pack_tests.rs"]
mod tests;
