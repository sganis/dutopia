// rs/src/util/row.rs

pub struct Row {
    pub dev: u64,
    pub ino: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,
    pub atime: i64,
    pub mtime: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_creation() {
        let row = Row {
            dev: 123,
            ino: 456,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            size: 1024,
            blocks: 2,
            atime: 1640995200,
            mtime: 1640995200,
        };

        assert_eq!(row.dev, 123);
        assert_eq!(row.ino, 456);
        assert_eq!(row.mode, 0o644);
        assert_eq!(row.uid, 1000);
        assert_eq!(row.gid, 1000);
        assert_eq!(row.size, 1024);
        assert_eq!(row.blocks, 2);
        assert_eq!(row.atime, 1640995200);
        assert_eq!(row.mtime, 1640995200);
    }
}
