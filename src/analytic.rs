// rs/src/analytic.rs
//
// Read-only analytics queries layered on the schema produced by `dudb`. Used
// by `duapi`'s MCP route. Kept separate from `db.rs` so neither file blows
// past the 600-line cap.
//
// Path semantics: when `path` is `Some(p)`, queries scope to rows at that
// exact node (which `dusum` has already rolled up to include the subtree).
// When `path` is `None`, queries scope to *platform roots* — children of the
// synthetic root row (`full_path = ''`). On Linux that is `/`; on Windows it
// is the union of drive roots and UNC server entries. This avoids the
// double-counting that would happen if we summed every ancestor row.
use crate::db::DbPool;
use anyhow::{Context, Result};
use rusqlite::{ToSql, params_from_iter};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserTotal {
    pub user: String,
    pub disk: u64,
    pub size: u64,
    pub count: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FolderTotal {
    pub path: String,
    pub disk: u64,
    pub size: u64,
    pub count: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColdFolder {
    pub path: String,
    pub age0_disk: u64,
    pub age1_disk: u64,
    pub age2_disk: u64,
    pub total_disk: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Summary {
    pub count: u64,
    pub size: u64,
    pub disk: u64,
    pub linked: u64,
    pub atime_min: i64,
    pub atime_max: i64,
    pub mtime_min: i64,
    pub mtime_max: i64,
}

/// `WHERE` clause that scopes `s.path_id` to either one exact path or the
/// children of the synthetic root (platform roots). Returns the SQL fragment
/// and the bound parameter values.
fn path_scope(path: Option<&str>) -> (String, Vec<Box<dyn ToSql>>) {
    match path {
        Some(p) => (
            "p.full_path = ?".to_string(),
            vec![Box::new(p.to_string())],
        ),
        None => (
            "p.parent_id = (SELECT id FROM paths WHERE full_path = '')".to_string(),
            vec![],
        ),
    }
}

pub fn top_consumers(
    pool: &DbPool,
    path: Option<&str>,
    limit: u32,
) -> Result<Vec<UserTotal>> {
    let conn = pool.get().context("acquiring connection")?;
    let (scope_sql, mut params) = path_scope(path);
    let sql = format!(
        "SELECT u.name,
                COALESCE(SUM(s.disk_bytes), 0)  AS disk,
                COALESCE(SUM(s.file_size), 0)   AS size,
                COALESCE(SUM(s.file_count), 0)  AS count
         FROM   stats s
         JOIN   paths p ON p.id = s.path_id
         JOIN   users u ON u.id = s.user_id
         WHERE  {scope_sql}
         GROUP  BY u.name
         ORDER  BY disk DESC
         LIMIT  ?"
    );
    params.push(Box::new(limit as i64));
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_from_iter(refs), |r| {
        Ok(UserTotal {
            user: r.get(0)?,
            disk: r.get::<_, i64>(1)? as u64,
            size: r.get::<_, i64>(2)? as u64,
            count: r.get::<_, i64>(3)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting top_consumers rows")
}

pub fn largest_folders(
    pool: &DbPool,
    path: Option<&str>,
    limit: u32,
) -> Result<Vec<FolderTotal>> {
    let conn = pool.get().context("acquiring connection")?;
    // For "largest under path" we want children of the requested path, not the
    // path itself. So the parent filter is by `parent.full_path` either way.
    let (parent_sql, mut params) = match path {
        Some(p) => (
            "parent.full_path = ?".to_string(),
            vec![Box::new(p.to_string()) as Box<dyn ToSql>],
        ),
        None => (
            "parent.full_path = ''".to_string(),
            vec![],
        ),
    };
    let sql = format!(
        "SELECT p.full_path,
                COALESCE(SUM(s.disk_bytes), 0)  AS disk,
                COALESCE(SUM(s.file_size), 0)   AS size,
                COALESCE(SUM(s.file_count), 0)  AS count
         FROM   paths parent
         JOIN   paths p ON p.parent_id = parent.id
         JOIN   stats s ON s.path_id   = p.id
         WHERE  {parent_sql}
         GROUP  BY p.full_path
         ORDER  BY disk DESC
         LIMIT  ?"
    );
    params.push(Box::new(limit as i64));
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_from_iter(refs), |r| {
        Ok(FolderTotal {
            path: r.get(0)?,
            disk: r.get::<_, i64>(1)? as u64,
            size: r.get::<_, i64>(2)? as u64,
            count: r.get::<_, i64>(3)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting largest_folders rows")
}

pub fn cold_data(
    pool: &DbPool,
    path: Option<&str>,
    limit: u32,
) -> Result<Vec<ColdFolder>> {
    let conn = pool.get().context("acquiring connection")?;
    let (parent_sql, mut params) = match path {
        Some(p) => (
            "parent.full_path = ?".to_string(),
            vec![Box::new(p.to_string()) as Box<dyn ToSql>],
        ),
        None => (
            "parent.full_path = ''".to_string(),
            vec![],
        ),
    };
    // Heuristic: a folder is "cold" when age=2 dominates and age=0 is
    // negligible. Thresholds picked to match the doc's wording — tunable, but
    // not surfaced as args until we have real-world data to calibrate against.
    let sql = format!(
        "SELECT p.full_path,
                COALESCE(SUM(CASE WHEN s.age=0 THEN s.disk_bytes ELSE 0 END), 0) AS d0,
                COALESCE(SUM(CASE WHEN s.age=1 THEN s.disk_bytes ELSE 0 END), 0) AS d1,
                COALESCE(SUM(CASE WHEN s.age=2 THEN s.disk_bytes ELSE 0 END), 0) AS d2,
                COALESCE(SUM(s.disk_bytes), 0)                                   AS total
         FROM   paths parent
         JOIN   paths p ON p.parent_id = parent.id
         JOIN   stats s ON s.path_id   = p.id
         WHERE  {parent_sql}
         GROUP  BY p.full_path
         HAVING total > 0
            AND d2 * 1.0 / total > 0.9
            AND d0 * 1.0 / total < 0.05
         ORDER  BY d2 DESC
         LIMIT  ?"
    );
    params.push(Box::new(limit as i64));
    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(params_from_iter(refs), |r| {
        Ok(ColdFolder {
            path: r.get(0)?,
            age0_disk: r.get::<_, i64>(1)? as u64,
            age1_disk: r.get::<_, i64>(2)? as u64,
            age2_disk: r.get::<_, i64>(3)? as u64,
            total_disk: r.get::<_, i64>(4)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collecting cold_data rows")
}

pub fn summary(
    pool: &DbPool,
    path: Option<&str>,
    users: &[String],
    age: Option<u8>,
) -> Result<Summary> {
    let conn = pool.get().context("acquiring connection")?;
    let (scope_sql, mut params) = path_scope(path);

    let mut sql = format!(
        "SELECT COALESCE(SUM(s.file_count),  0),
                COALESCE(SUM(s.file_size),   0),
                COALESCE(SUM(s.disk_bytes),  0),
                COALESCE(SUM(s.linked_size), 0),
                COALESCE(MIN(s.atime),       0),
                COALESCE(MAX(s.atime),       0),
                COALESCE(MIN(s.mtime),       0),
                COALESCE(MAX(s.mtime),       0)
         FROM   stats s
         JOIN   paths p ON p.id = s.path_id
         JOIN   users u ON u.id = s.user_id
         WHERE  {scope_sql}"
    );
    if let Some(a) = age {
        sql.push_str(" AND s.age = ?");
        params.push(Box::new(a as i64));
    }
    if !users.is_empty() {
        sql.push_str(" AND u.name IN (");
        for (i, u) in users.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
            params.push(Box::new(u.clone()));
        }
        sql.push(')');
    }

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let row = stmt.query_row(params_from_iter(refs), |r| {
        Ok(Summary {
            count: r.get::<_, i64>(0)? as u64,
            size: r.get::<_, i64>(1)? as u64,
            disk: r.get::<_, i64>(2)? as u64,
            linked: r.get::<_, i64>(3)? as u64,
            atime_min: r.get(4)?,
            atime_max: r.get(5)?,
            mtime_min: r.get(6)?,
            mtime_max: r.get(7)?,
        })
    })?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{open_pool, test_support::build_test_db};

    fn pool() -> (crate::db::test_support::TempDb, DbPool) {
        let t = build_test_db();
        let p = open_pool(&t.path).unwrap();
        (t, p)
    }

    #[test]
    fn top_consumers_at_root_orders_by_disk_desc() {
        let (_t, p) = pool();
        // Fixture has alice@/ disk=100 and bob@/ disk=50 at platform root `/`.
        // `/docs` is NOT included (it's a child, not a platform root).
        let v = top_consumers(&p, None, 10).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].user, "alice");
        assert_eq!(v[0].disk, 100);
        assert_eq!(v[1].user, "bob");
        assert_eq!(v[1].disk, 50);
    }

    #[test]
    fn top_consumers_under_specific_path_includes_subtree() {
        let (_t, p) = pool();
        // `/docs` row: alice age=2 disk=300.
        let v = top_consumers(&p, Some("/docs"), 10).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].user, "alice");
        assert_eq!(v[0].disk, 300);
    }

    #[test]
    fn top_consumers_unknown_path_is_empty() {
        let (_t, p) = pool();
        let v = top_consumers(&p, Some("/nope"), 10).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn largest_folders_at_root_returns_platform_root() {
        let (_t, p) = pool();
        let v = largest_folders(&p, None, 10).unwrap();
        // Synthetic-root child on Linux fixture is `/`.
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "/");
        // Sum of alice (100) + bob (50) at `/`.
        assert_eq!(v[0].disk, 150);
    }

    #[test]
    fn largest_folders_under_unix_root_returns_docs() {
        let (_t, p) = pool();
        let v = largest_folders(&p, Some("/"), 10).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "/docs");
        assert_eq!(v[0].disk, 300);
    }

    #[test]
    fn cold_data_finds_age2_dominated_folder() {
        let (_t, p) = pool();
        // `/docs` is 100% age=2 → should match.
        let v = cold_data(&p, Some("/"), 10).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].path, "/docs");
        assert_eq!(v[0].age2_disk, 300);
        assert_eq!(v[0].age0_disk, 0);
        assert_eq!(v[0].total_disk, 300);
    }

    #[test]
    fn cold_data_skips_mixed_age_folder() {
        let (_t, p) = pool();
        // At platform root, `/` mixes age=0 (alice) and age=1 (bob); no age=2
        // → no row passes the threshold.
        let v = cold_data(&p, None, 10).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn summary_at_root_aggregates_platform_roots() {
        let (_t, p) = pool();
        let s = summary(&p, None, &[], None).unwrap();
        // alice@/ : count=2 size=200 disk=100
        // bob@/   : count=1 size=50  disk=50
        assert_eq!(s.count, 3);
        assert_eq!(s.size, 250);
        assert_eq!(s.disk, 150);
    }

    #[test]
    fn summary_user_filter_limits_aggregation() {
        let (_t, p) = pool();
        let s = summary(&p, None, &["alice".into()], None).unwrap();
        assert_eq!(s.count, 2);
        assert_eq!(s.size, 200);
        assert_eq!(s.disk, 100);
    }

    #[test]
    fn summary_age_filter_limits_aggregation() {
        let (_t, p) = pool();
        // age=1 at `/` is bob only.
        let s = summary(&p, None, &[], Some(1)).unwrap();
        assert_eq!(s.count, 1);
        assert_eq!(s.disk, 50);
    }

    #[test]
    fn summary_under_specific_path_uses_rolled_up_row() {
        let (_t, p) = pool();
        let s = summary(&p, Some("/docs"), &[], None).unwrap();
        assert_eq!(s.count, 3);
        assert_eq!(s.size, 600);
        assert_eq!(s.disk, 300);
        assert_eq!(s.linked, 300);
    }
}
