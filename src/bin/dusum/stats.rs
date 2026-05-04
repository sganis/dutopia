// rs/src/bin/dusum/stats.rs
use anyhow::Result;

#[derive(Copy, Clone, Debug)]
pub struct AgeCfg {
    pub young: i64,
    pub old: i64,
}

impl Default for AgeCfg {
    fn default() -> Self {
        Self { young: 60, old: 600 }
    }
}

impl AgeCfg {
    pub fn from_args(age: &Option<(i64, i64)>) -> Self {
        let mut cfg = AgeCfg::default();
        if let Some((a, b)) = age {
            cfg.young = *a;
            cfg.old = *b;
        }
        cfg
    }
}

pub fn parse_age_pair(s: &str) -> Result<(i64, i64), String> {
    let mut it = s.split(',');
    let a = it
        .next()
        .ok_or("expected two comma-separated integers, e.g. 60,600")?;
    let b = it
        .next()
        .ok_or("expected two comma-separated integers, e.g. 60,600")?;
    if it.next().is_some() {
        return Err("expected exactly two values: YOUNG,OLD".into());
    }
    let a: i64 = a.trim().parse().map_err(|_| "YOUNG must be an integer")?;
    let b: i64 = b.trim().parse().map_err(|_| "OLD must be an integer")?;
    if a <= 0 || b <= 0 || a >= b {
        return Err("must be positive and increasing (e.g. 60,600)".into());
    }
    Ok((a, b))
}

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

/// Sanitize mtime: if it's more than 1 day in the future, set to 0
pub fn sanitize_mtime(now_ts: i64, mtime_ts: i64) -> i64 {
    const ONE_DAY_SECS: i64 = 86_400;
    if mtime_ts > now_ts + ONE_DAY_SECS {
        0
    } else {
        mtime_ts
    }
}

/// Bucket age in days using configurable thresholds:
/// 0: recent (< young)
/// 1: not too old (>= young and < old)
/// 2: old (>= old or invalid/unknown)
pub fn age_bucket(now_ts: i64, mtime_ts: i64, cfg: AgeCfg) -> u8 {
    if mtime_ts <= 0 {
        return 2;
    }
    let age_secs = now_ts.saturating_sub(mtime_ts);
    let days = age_secs / 86_400;
    if days < cfg.young {
        0
    } else if days < cfg.old {
        1
    } else {
        2
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

    #[test]
    fn age_bucket_categorizes_correctly() {
        let cfg = AgeCfg { young: 60, old: 600 };
        let now = 1_000_000_000;

        assert_eq!(age_bucket(now, now - 30 * 86_400, cfg), 0);
        assert_eq!(age_bucket(now, now - 59 * 86_400, cfg), 0);
        assert_eq!(age_bucket(now, now - 60 * 86_400, cfg), 1);
        assert_eq!(age_bucket(now, now - 100 * 86_400, cfg), 1);
        assert_eq!(age_bucket(now, now - 599 * 86_400, cfg), 1);
        assert_eq!(age_bucket(now, now - 600 * 86_400, cfg), 2);
        assert_eq!(age_bucket(now, now - 700 * 86_400, cfg), 2);
        assert_eq!(age_bucket(now, 0, cfg), 2);
        assert_eq!(age_bucket(now, -1, cfg), 2);
    }

    #[test]
    fn age_bucket_boundary_conditions() {
        let cfg = AgeCfg { young: 60, old: 600 };
        let now = 1_000_000_000;

        assert_eq!(age_bucket(now, now - 60 * 86_400, cfg), 1);
        assert_eq!(age_bucket(now, now - 60 * 86_400 + 1, cfg), 0);
        assert_eq!(age_bucket(now, now - 600 * 86_400, cfg), 2);
        assert_eq!(age_bucket(now, now - 600 * 86_400 + 1, cfg), 1);
    }

    #[test]
    fn sanitize_mtime_handles_future_dates() {
        let now = 1_000_000_000;

        assert_eq!(sanitize_mtime(now, now - 1000), now - 1000);
        assert_eq!(sanitize_mtime(now, now + 3600), now + 3600);
        assert_eq!(sanitize_mtime(now, now + 86_399), now + 86_399);
        assert_eq!(sanitize_mtime(now, now + 86_400), now + 86_400);
        assert_eq!(sanitize_mtime(now, now + 86_401), 0);
        assert_eq!(sanitize_mtime(now, now + 2 * 86_400), 0);
        assert_eq!(sanitize_mtime(now, now + 365 * 86_400), 0);
    }

    #[test]
    fn age_cfg_default_values() {
        let cfg = AgeCfg::default();
        assert_eq!(cfg.young, 60);
        assert_eq!(cfg.old, 600);
    }

    #[test]
    fn age_cfg_from_args_uses_provided_values() {
        let args = Some((30, 365));
        let cfg = AgeCfg::from_args(&args);
        assert_eq!(cfg.young, 30);
        assert_eq!(cfg.old, 365);
    }

    #[test]
    fn age_cfg_from_args_uses_defaults_when_none() {
        let cfg = AgeCfg::from_args(&None);
        assert_eq!(cfg.young, 60);
        assert_eq!(cfg.old, 600);
    }
}
