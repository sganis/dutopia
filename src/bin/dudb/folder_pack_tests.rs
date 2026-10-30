// src/bin/dudb/folder_pack_tests.rs
use super::*;
use crate::extsort::FileMeta;
use crate::schema;
use dutopia::filepack::unpack;

const ALL: ListRule = ListRule { cap: 0, min_size: 0 };

fn rec(folder_id: i64, name: &str, user_id: u32, age: u8, size: u64) -> FileRec {
    let meta = FileMeta { user_id, age, size, atime: 1, mtime: 2 };
    FileRec { folder_id, name: name.into(), meta }
}

/// Write `recs` and return the connection plus (seen, listed, folders).
fn write(recs: &[FileRec], rule: ListRule) -> (Connection, (u64, u64, u64)) {
    let c = Connection::open_in_memory().unwrap();
    schema::create_tables(&c).unwrap();
    let mut w = FolderWriter::new(&c, rule).unwrap();
    for r in recs {
        w.push(r).unwrap();
    }
    let w = w.finish().unwrap();
    let counts = (w.files_seen, w.files_listed, w.folders);
    drop(w);
    (c, counts)
}

fn folder(c: &Connection, id: i64) -> PackedFolder {
    let blob: Vec<u8> = c
        .query_row("SELECT data FROM folder_files WHERE folder_id = ?1", [id], |r| r.get(0))
        .unwrap();
    unpack(&blob).unwrap()
}

fn names(f: &PackedFolder) -> Vec<&str> {
    f.files.iter().map(|f| f.name.as_str()).collect()
}

#[test]
fn one_row_per_folder_largest_first() {
    let recs = [rec(1, "a", 1, 0, 5), rec(1, "b", 1, 0, 50), rec(2, "c", 1, 0, 7)];
    let (c, counts) = write(&recs, ALL);
    assert_eq!(counts, (3, 3, 2));
    assert_eq!(names(&folder(&c, 1)), ["b", "a"]);
    let two = folder(&c, 2);
    assert_eq!(two.files[0], PackedFile {
        name: "c".into(), user_id: 1, age: 0, size: 7, atime: 1, mtime: 2
    });
}

#[test]
fn totals_count_every_file_per_user_and_age() {
    let mut recs = vec![rec(1, "a", 1, 0, 5), rec(1, "b", 1, 0, 50), rec(1, "c", 2, 2, 7)];
    recs[1].meta.mtime = 99;
    recs[0].meta.atime = 42;
    let (c, _) = write(&recs, ListRule { cap: 1, min_size: 10 });
    let f = folder(&c, 1);
    assert_eq!(
        f.totals,
        [
            GroupTotals { user_id: 1, age: 0, count: 2, bytes: 55, atime: 42, mtime: 99 },
            GroupTotals { user_id: 2, age: 2, count: 1, bytes: 7, atime: 1, mtime: 2 },
        ]
    );
    // Only "b" is both within the cap and above the size floor.
    assert_eq!(names(&f), ["b"]);
}

#[test]
fn small_files_only_count() {
    let recs = [rec(1, "tiny", 1, 0, 999), rec(1, "big", 1, 0, 1000)];
    let (c, counts) = write(&recs, ListRule { cap: 0, min_size: 1000 });
    assert_eq!(counts, (2, 1, 1));
    let f = folder(&c, 1);
    assert_eq!(names(&f), ["big"]);
    assert_eq!(f.totals[0].count, 2);
    // A folder of only small files still gets its totals row.
    let (c, counts) = write(&[rec(7, "tiny", 1, 0, 1)], ListRule { cap: 0, min_size: 1000 });
    assert_eq!(counts, (1, 0, 1));
    let f = folder(&c, 7);
    assert!(f.files.is_empty());
    assert_eq!((f.totals[0].count, f.totals[0].bytes), (1, 1));
}

#[test]
fn cap_applies_per_user_and_age() {
    // 10 files each for (alice, age 0), (alice, age 2) and (bob, age 0);
    // bob's are all smaller than alice's, so a per-folder cap would drop them.
    let mut recs = Vec::new();
    for i in 0..10u64 {
        recs.push(rec(1, &format!("a0_{i:02}"), 1, 0, 1000 + i));
        recs.push(rec(1, &format!("a2_{i:02}"), 1, 2, 2000 + i));
        recs.push(rec(1, &format!("b0_{i:02}"), 2, 0, i));
    }
    recs.sort_by(|a, b| a.name.cmp(&b.name));
    let (c, counts) = write(&recs, ListRule { cap: 3, min_size: 0 });
    assert_eq!(counts, (30, 9, 1));
    assert_eq!(
        names(&folder(&c, 1)),
        ["a2_09", "a2_08", "a2_07", "a0_09", "a0_08", "a0_07", "b0_09", "b0_08", "b0_07"]
    );
}

#[test]
fn streaming_prune_keeps_the_true_largest() {
    // Sizes rise then fall so the largest arrive both before and after the
    // in-flight prunes at 2 × cap.
    let sizes = [5u64, 9, 1, 8, 2, 7, 3, 6, 4, 10, 0, 11];
    let recs: Vec<FileRec> = sizes
        .iter()
        .enumerate()
        .map(|(i, &s)| rec(1, &format!("f{i:02}"), 1, 0, s))
        .collect();
    let (c, counts) = write(&recs, ListRule { cap: 2, min_size: 0 });
    assert_eq!(counts.1, 2);
    let f = folder(&c, 1);
    let got: Vec<u64> = f.files.iter().map(|f| f.size).collect();
    assert_eq!(got, [11, 10]);
    assert_eq!((f.totals[0].count, f.totals[0].bytes), (12, 66));
}

#[test]
fn equal_sizes_keep_names_in_order() {
    let recs: Vec<FileRec> = ["d", "a", "c", "b"].iter().map(|n| rec(1, n, 1, 0, 1)).collect();
    let (c, _) = write(&recs, ListRule { cap: 2, min_size: 0 });
    assert_eq!(names(&folder(&c, 1)), ["a", "b"]);
}
