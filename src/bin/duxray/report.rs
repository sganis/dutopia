// src/bin/duxray/report.rs
//
// Human-readable report, mirroring the Python prototype's sections:
// size distribution, age/abandonment, totals, small-file waste, data
// quality, junk, permissions, paths and top-N rankings.

use crate::stats::{
    ABANDON_LABELS, AGE_LABELS, BAND_EDGES, NBAND, THRESHOLDS, Stats,
};
use std::path::Path;

pub fn print_report(s: &Stats, input: &Path, now: i64, top: usize) {
    if s.files == 0 {
        println!("no regular files found");
        return;
    }
    let n = s.files as f64;
    let pct = |v: u64| 100.0 * v as f64 / n;
    let bshare = |v: u64| {
        if s.total_size == 0 {
            0.0
        } else {
            100.0 * v as f64 / s.total_size as f64
        }
    };

    println!("file:           {}", input.display());
    println!("regular files:  {}", s.files);
    println!("directories:    {}   (skipped)", s.dirs);
    if s.other > 0 {
        println!("other types:    {}   (skipped)", s.other);
    }
    if s.parse_errors > 0 {
        println!("parse errors:   {}", s.parse_errors);
    }
    println!();

    println!("== size distribution ==");
    println!(
        "  {:>12} : {:>15} ({:>6})  {:>12} ({:>6})",
        "band", "files", "file%", "logical", "byte%"
    );
    for i in 0..NBAND {
        let label = if i < NBAND - 1 {
            format!(
                "{}-{}",
                fmt_size(BAND_EDGES[i]),
                fmt_size(BAND_EDGES[i + 1])
            )
        } else {
            format!(">{}", fmt_size(BAND_EDGES[NBAND - 1]))
        };
        let bar = "#".repeat((pct(s.band_files[i]) / 2.0) as usize);
        println!(
            "  {:>12} : {:>15} ({:6.2}%)  {:>12} ({:6.2}%) {}",
            label,
            s.band_files[i],
            pct(s.band_files[i]),
            fmt_bytes(s.band_size[i]),
            bshare(s.band_size[i]),
            bar
        );
    }
    println!();
    println!("  cumulative (< threshold):");
    for (i, (name, _t)) in THRESHOLDS.iter().enumerate() {
        println!(
            "  < {:>4} : {:>15} ({:6.2}%)  bytes: {} ({:.2}%)",
            name,
            s.thr_lt[i],
            pct(s.thr_lt[i]),
            fmt_bytes(s.thr_size[i]),
            bshare(s.thr_size[i])
        );
    }
    println!("  empty: {} ({:.2}%)", s.empty, pct(s.empty));
    println!();

    println!("== age distribution ==");
    for i in 0..4 {
        println!(
            "  mtime {:>7} : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
            AGE_LABELS[i],
            s.mtime_bucket[i],
            pct(s.mtime_bucket[i]),
            fmt_bytes(s.mtime_bytes[i]),
            bshare(s.mtime_bytes[i])
        );
    }
    println!();
    for i in 0..4 {
        println!(
            "  atime {:>7} : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
            AGE_LABELS[i],
            s.atime_bucket[i],
            pct(s.atime_bucket[i]),
            fmt_bytes(s.atime_bytes[i]),
            bshare(s.atime_bytes[i])
        );
    }
    println!();
    println!("  last touched (max of atime, mtime):");
    for i in 0..4 {
        println!(
            "  touch {:>7} : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
            AGE_LABELS[i],
            s.touched_bucket[i],
            pct(s.touched_bucket[i]),
            fmt_bytes(s.touched_bytes[i]),
            bshare(s.touched_bytes[i])
        );
    }
    println!();
    if let Some(t) = s.min_mtime {
        println!("  oldest mtime      : {}", ts(t));
        println!("  newest mtime      : {}", ts(s.max_mtime.unwrap()));
        println!("  oldest atime      : {}", ts(s.min_atime.unwrap()));
        println!("  newest atime      : {}", ts(s.max_atime.unwrap()));
    }
    println!(
        "  atime == mtime    : {} ({:.2}%)",
        s.atime_eq_mtime,
        pct(s.atime_eq_mtime)
    );
    println!(
        "  write-once (>600d, never read since write): {} ({:.2}%)",
        s.write_once,
        pct(s.write_once)
    );
    println!(
        "  never read since write (atime<=mtime): {} ({:.2}%), {}",
        s.never_read,
        pct(s.never_read),
        fmt_bytes(s.never_read_size)
    );
    println!();

    println!("== abandonment (by last touched = max(atime, mtime)) ==");
    for (i, label) in ABANDON_LABELS.iter().enumerate() {
        println!(
            "  {:>6} : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
            label,
            s.abandon_files[i],
            pct(s.abandon_files[i]),
            fmt_bytes(s.abandon_size[i]),
            bshare(s.abandon_size[i])
        );
    }
    let over1y_f: u64 = s.abandon_files[1..].iter().sum();
    let over1y_b: u64 = s.abandon_size[1..].iter().sum();
    let over2y_f: u64 = s.abandon_files[2..].iter().sum();
    let over2y_b: u64 = s.abandon_size[2..].iter().sum();
    println!();
    println!(
        "  untouched >1y : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
        over1y_f,
        pct(over1y_f),
        fmt_bytes(over1y_b),
        bshare(over1y_b)
    );
    println!(
        "  untouched >2y : {:>15} ({:6.2}%)  {:>12} ({:6.2}%)",
        over2y_f,
        pct(over2y_f),
        fmt_bytes(over2y_b),
        bshare(over2y_b)
    );
    println!();

    println!("== totals ==");
    println!("  logical size   : {}", fmt_bytes(s.total_size));
    println!("  disk usage     : {}", fmt_bytes(s.total_disk));
    println!("  avg file       : {:.1} KiB", s.total_size as f64 / n / 1024.0);
    if s.total_disk > s.total_size {
        let ovh = 100.0 * (s.total_disk - s.total_size) as f64
            / s.total_size as f64;
        println!(
            "  slack/overhead : {} ({:.1}%)",
            fmt_bytes(s.total_disk - s.total_size),
            ovh
        );
    } else if s.total_size > s.total_disk {
        println!(
            "  sparse savings : {}",
            fmt_bytes(s.total_size - s.total_disk)
        );
    }
    println!(
        "  largest file   : {}  {}",
        fmt_bytes(s.largest.1),
        s.largest.0
    );
    println!();

    if let Some(h) = &s.hardlinks {
        println!(
            "== hardlinks (dedup by inode, per volume: {} volumes) ==",
            h.volumes()
        );
        println!(
            "  unique files        : {:>15} ({:6.2}% of files)",
            h.unique_files,
            pct(h.unique_files)
        );
        println!(
            "  additional links    : {:>15} ({:6.2}% of files)",
            h.linked_files,
            pct(h.linked_files)
        );
        println!(
            "  unique logical size : {:>12} ({:6.2}% of logical)",
            fmt_bytes(h.unique_size),
            bshare(h.unique_size)
        );
        println!(
            "  unique disk usage   : {:>12} ({:6.2}% of disk)",
            fmt_bytes(h.unique_disk),
            if s.total_disk > 0 {
                100.0 * h.unique_disk as f64 / s.total_disk as f64
            } else {
                0.0
            }
        );
        println!();
        println!(
            "  saved by hardlinks  : {:>12} logical ({:6.2}% of logical)",
            fmt_bytes(h.linked_size),
            bshare(h.linked_size)
        );
        println!(
            "  saved by hardlinks  : {:>12} disk    ({:6.2}% of disk)",
            fmt_bytes(h.linked_disk),
            if s.total_disk > 0 {
                100.0 * h.linked_disk as f64 / s.total_disk as f64
            } else {
                0.0
            }
        );
        println!();
    }

    println!("== small-file waste (files < 1MB) ==");
    let small_f = s.thr_lt[3];
    let small_b = s.thr_size[3];
    if small_f > 0 {
        let small_disk: u64 = s.band_disk[..4].iter().sum();
        println!(
            "  files < 1MB        : {} ({:.2}% of files, {:.2}% of bytes)",
            small_f,
            pct(small_f),
            bshare(small_b)
        );
        println!("  their disk usage   : {}", fmt_bytes(small_disk));
        if small_disk > small_b {
            let waste = small_disk - small_b;
            println!(
                "  block slack waste : {} ({:.1}% of their disk usage)",
                fmt_bytes(waste),
                100.0 * waste as f64 / small_disk as f64
            );
        }
    }
    println!();

    println!("== data quality ==");
    println!("  future mtimes (>now+1d) : {}", s.future_mtime);
    println!("  epoch mtimes (<=0)      : {}", s.epoch_mtime);
    println!(
        "  sparse files (disk<size): {} (saves {})",
        s.sparse,
        fmt_bytes(s.sparse_saved)
    );
    println!();

    println!("== junk / cleanup candidates ==");
    println!(
        "  junk files (.DS_Store, .tmp, .bak, .nfs*, ...) : {}",
        s.junk
    );
    println!("  junk bytes                          : {}", fmt_bytes(s.junk_size));
    println!();

    println!("== permissions ==");
    println!(
        "  world-writable  : {} ({})",
        s.world_writable,
        fmt_bytes(s.world_writable_size)
    );
    println!("  executables     : {}", s.execs);
    println!("  mode 000        : {}", s.no_perm);
    println!();

    println!("== paths ==");
    println!("  max depth       : {}", s.max_depth);
    println!("  >255-byte paths : {}", s.long_paths);
    println!("  longest path    : {} bytes", s.max_path_len);
    println!("                   {}", truncate_chars(&s.longest_path, 100));
    println!("  depth histogram (file count):");
    for (d, &cnt) in s.depth_hist.iter().enumerate() {
        if cnt > 0 {
            println!("    depth {:>2} : {:>15}", d, cnt);
        }
    }
    println!();

    println!("== top {} owners by abandoned bytes (>1y untouched) ==", top);
    println!("  {:>8} : {:>12} ({:>6})  {:>12}  files", "uid", "abandoned", "abn%", "total");
    let mut owners: Vec<_> = s
        .owners
        .iter()
        .filter(|(_, o)| o.abandon_bytes > 0)
        .collect();
    owners.sort_by(|a, b| b.1.abandon_bytes.cmp(&a.1.abandon_bytes));
    for (uid, o) in owners.into_iter().take(top) {
        let ratio = if o.bytes > 0 {
            100.0 * o.abandon_bytes as f64 / o.bytes as f64
        } else {
            0.0
        };
        println!(
            "  {:>8} : {:>12} ({:6.1}%)  {:>12}  {}",
            uid, fmt_bytes(o.abandon_bytes), ratio, fmt_bytes(o.bytes), o.files
        );
    }
    println!();

    println!("== top {} projects by abandoned bytes (>1y untouched) ==", top);
    println!("  {:<44} : {:>12} ({:>6})  {:>12}", "project", "abandoned", "abn%", "total");
    let mut projects: Vec<_> = s
        .projects
        .iter()
        .filter(|(_, p)| p.abandon_bytes > 0)
        .collect();
    projects.sort_by(|a, b| b.1.abandon_bytes.cmp(&a.1.abandon_bytes));
    for (proj, p) in projects.into_iter().take(top) {
        let ratio = if p.bytes > 0 {
            100.0 * p.abandon_bytes as f64 / p.bytes as f64
        } else {
            0.0
        };
        println!(
            "  {:<44} : {:>12} ({:6.1}%)  {:>12}",
            truncate_chars(proj, 44),
            fmt_bytes(p.abandon_bytes),
            ratio,
            fmt_bytes(p.bytes)
        );
    }
    println!();

    println!("== top {} extensions by files ==", top);
    let mut exts: Vec<_> = s.ext_counts.iter().collect();
    exts.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (ext, (files, bytes)) in exts.into_iter().take(top) {
        println!(
            "  .{:<12} : {:>15} ({:6.2}%)  {:>12}",
            truncate_chars(ext, 12),
            files,
            pct(*files),
            fmt_bytes(*bytes)
        );
    }
    println!();

    println!("== top {} extensions by bytes ==", top);
    let mut exts: Vec<_> = s.ext_counts.iter().collect();
    exts.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    for (ext, (files, bytes)) in exts.into_iter().take(top) {
        println!(
            "  .{:<12} : {:>12} ({:6.2}%)  {:>15} files",
            truncate_chars(ext, 12),
            fmt_bytes(*bytes),
            bshare(*bytes),
            files
        );
    }
    println!();

    println!("== top {} second-level dirs (volume/project) ==", top);
    println!("  {:<24} : {:>15} files  {:>12}", "dir", "total", "bytes");
    let mut dirs: Vec<_> = s.projects.iter().collect();
    dirs.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes));
    for (proj, p) in dirs.into_iter().take(top) {
        println!(
            "  {:<24} : {:>15}  {:>12}",
            truncate_chars(proj, 24),
            p.files,
            fmt_bytes(p.bytes)
        );
    }
    println!();
    println!("scan date: {}", ts(now));
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn fmt_size(n: u64) -> String {
    match n {
        0 => "0".into(),
        1..=1023 => format!("{}B", n),
        _ => {
            let k = n / 1024;
            if k < 1024 {
                format!("{}K", k)
            } else if k < 1024 * 1024 {
                format!("{}M", k / 1024)
            } else {
                format!("{}G", k / (1024 * 1024))
            }
        }
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if n == 0 {
        return "0.00 B".into();
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", v, UNITS[i])
}

fn ts(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "invalid".into())
}
