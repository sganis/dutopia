// rs/src/util/mod.rs

mod csv;
mod format;
mod path;
mod platform;
mod row;

// Re-export everything for backward compatibility
pub use csv::{parse_int, push_i64, push_u32, push_u64, trim_ascii};
pub use format::{
    format_duration, get_hostname, human_bytes, human_count, parse_file_hint, print_about,
    progress_bar, spinner,
};
pub use path::{is_volume_root, should_skip, strip_verbatim_prefix};
pub use platform::fs_used_bytes;
pub use row::Row;

#[cfg(windows)]
pub use platform::get_rid;
