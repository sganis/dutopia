// src/util/age.rs
//
// Age-bucket configuration and bucketing, shared by `dusum` (folder
// aggregation) and `dudb` (per-file ingest). Both must bucket identically or
// /api/files?age=N returns a different set of files than the aggregated
// bucket-N stats shown on /api/folders for the same folder.

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

    #[test]
    fn parse_age_pair_valid_and_invalid() {
        assert_eq!(parse_age_pair("60,600").unwrap(), (60, 600));
        assert_eq!(parse_age_pair(" 30 , 365 ").unwrap(), (30, 365));
        assert!(parse_age_pair("60").is_err());
        assert!(parse_age_pair("60,600,900").is_err());
        assert!(parse_age_pair("600,60").is_err());
        assert!(parse_age_pair("0,600").is_err());
        assert!(parse_age_pair("x,y").is_err());
    }
}
