// rs/src/bin/dusum/stats.rs
//
// Age bucketing moved to `dutopia::util::age` so `dudb` per-file ingest
// buckets identically; re-exported here to keep dusum's imports stable.
pub use dutopia::util::{age_bucket, parse_age_pair, sanitize_mtime, AgeCfg};

#[derive(Default, Clone, Debug, PartialEq)]
pub struct UserStats {
    pub file_count: u64,
    pub file_size: u64,
    pub disk_size: u64,
    pub linked_size: u64,
    pub latest_atime: i64,
    pub latest_mtime: i64,
}

impl UserStats {
    pub fn update(
        &mut self,
        size: u64,
        disk: u64,
        linked: u64,
        atime_secs: i64,
        mtime_secs: i64,
    ) {
        self.file_count = self.file_count.saturating_add(1);
        self.file_size = self.file_size.saturating_add(size);
        self.disk_size = self.disk_size.saturating_add(disk);
        self.linked_size = self.linked_size.saturating_add(linked);
        if atime_secs > self.latest_atime {
            self.latest_atime = atime_secs;
        }
        if mtime_secs > self.latest_mtime {
            self.latest_mtime = mtime_secs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userstats_update_accumulates_correctly() {
        let mut stats = UserStats::default();
        stats.update(100, 100, 0, 1000, 2000);
        stats.update(200, 0, 200, 3000, 4000);

        assert_eq!(stats.file_count, 2);
        assert_eq!(stats.file_size, 300);
        assert_eq!(stats.disk_size, 100);
        assert_eq!(stats.linked_size, 200);
        assert_eq!(stats.latest_atime, 3000);
        assert_eq!(stats.latest_mtime, 4000);
    }

    #[test]
    fn userstats_update_keeps_latest_times() {
        let mut stats = UserStats::default();
        stats.update(100, 100, 0, 5000, 6000);
        stats.update(200, 200, 0, 3000, 8000);
        stats.update(300, 300, 0, 7000, 4000);

        assert_eq!(stats.latest_atime, 7000);
        assert_eq!(stats.latest_mtime, 8000);
    }
}
