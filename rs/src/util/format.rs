// rs/src/util/format.rs
use std::time::Duration;
use colored::Colorize;
use std::sync::atomic::{AtomicUsize, Ordering};

static SPINNER: [&str; 4] = ["/", "-", "\\", "|"];
static FRAME: AtomicUsize = AtomicUsize::new(0);

pub fn spinner() -> &'static str {
    let i = FRAME.fetch_add(1, Ordering::Relaxed) % SPINNER.len();
    SPINNER[i]
}

pub fn print_about() {
    #[cfg(windows)]
    colored::control::set_virtual_terminal(true).unwrap_or(());

    println!("{}", "-".repeat(44).bright_cyan());
    println!(
        "{}",
        format!("Dutopia      : Superfast filesystem analyzer").bright_cyan()
    );
    println!(
        "{}",
        format!("Version      : {}", env!("CARGO_PKG_VERSION")).bright_cyan()
    );
    println!(
        "{}",
        format!("Built        : {}", env!("BUILD_DATE")).bright_cyan()
    );
    println!("{}", "-".repeat(44).bright_cyan());
}

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{:.1}s", duration.as_secs_f64())
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!(
            "{}h {:02}m {:02}s",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    }
}

pub fn human_count(n: u64) -> String {
    const UNITS: [&str; 5] = ["", "K", "M", "B", "T"];
    let mut val = n as f64;
    let mut unit = 0;

    while val >= 1000.0 && unit < UNITS.len() - 1 {
        val /= 1000.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{}", n)
    } else {
        format!("{:.1}{}", val, UNITS[unit])
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;

    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{}{}", size as u64, UNITS[unit])
    } else {
        format!("{:.1}{}", size, UNITS[unit])
    }
}

pub fn get_hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|s| s.into_string().ok())
        .unwrap_or_else(|| "noname".to_string())
}

/// Print a colorized progress bar like: [====>-----] 42%
pub fn progress_bar(pct: f64, width: usize) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let body_len = filled.saturating_sub(1);
    let has_head = (filled > 0) as usize;
    let tail_len = width.saturating_sub(body_len + has_head);
    let mut bar = String::with_capacity(width + 8);
    bar.push('[');

    if body_len > 0 {
        bar.push_str(&"=".repeat(body_len).bright_cyan().to_string());
    }
    if has_head == 1 {
        bar.push_str(&">".bright_cyan().to_string());
    }
    if tail_len > 0 {
        bar.push_str(&"-".repeat(tail_len).bright_black().to_string());
    }
    bar.push(']');

    bar
}

pub fn parse_file_hint(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let mut num = String::new();
    let mut unit = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else if !ch.is_whitespace() {
            unit.push(ch);
        }
    }

    let val: f64 = num.parse().ok()?;
    let mul: f64 = match unit.as_str() {
        "" => 1.0,
        "k" => 1_000.0,
        "m" => 1_000_000.0,
        "g" => 1_000_000_000.0,
        _ => return None,
    };

    Some((val * mul) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    #[test]
    fn test_spinner() {
        FRAME.store(0, Ordering::Relaxed);

        assert_eq!(spinner(), "/");
        assert_eq!(spinner(), "-");
        assert_eq!(spinner(), "\\");
        assert_eq!(spinner(), "|");
        assert_eq!(spinner(), "/");
    }

    #[test]
    fn test_spinner_thread_safety() {
        use std::thread;

        let handles: Vec<_> = (0..10)
            .map(|_| {
                thread::spawn(|| {
                    for _ in 0..100 {
                        let s = spinner();
                        assert!(SPINNER.contains(&s));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0.0s");
        assert_eq!(format_duration(Duration::from_secs(1)), "1.0s");
        assert_eq!(format_duration(Duration::from_secs(30)), "30.0s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1m 00s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h 00m 00s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 01m 01s");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h 00m 00s");
    }

    #[test]
    fn test_format_duration_fractional() {
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::from_millis(500)), "0.5s");
        assert_eq!(format_duration(Duration::from_micros(100000)), "0.1s");
    }

    #[test]
    fn test_human_count_basic() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(1), "1");
        assert_eq!(human_count(999), "999");
    }

    #[test]
    fn test_human_count_thousands() {
        assert_eq!(human_count(1000), "1.0K");
        assert_eq!(human_count(1500), "1.5K");
        assert_eq!(human_count(999999), "1000.0K");
    }

    #[test]
    fn test_human_count_millions() {
        assert_eq!(human_count(1000000), "1.0M");
        assert_eq!(human_count(1500000), "1.5M");
        assert_eq!(human_count(999999999), "1000.0M");
    }

    #[test]
    fn test_human_count_billions() {
        assert_eq!(human_count(1000000000), "1.0B");
        assert_eq!(human_count(1500000000), "1.5B");
    }

    #[test]
    fn test_human_count_trillions() {
        assert_eq!(human_count(1000000000000), "1.0T");
        assert_eq!(
            human_count(u64::MAX),
            format!("{:.1}T", u64::MAX as f64 / 1e12)
        );
    }

    #[test]
    fn test_human_bytes_basic() {
        assert_eq!(human_bytes(0), "0B");
        assert_eq!(human_bytes(1), "1B");
        assert_eq!(human_bytes(1023), "1023B");
    }

    #[test]
    fn test_human_bytes_kilobytes() {
        assert_eq!(human_bytes(1024), "1.0KB");
        assert_eq!(human_bytes(1536), "1.5KB");
        assert_eq!(human_bytes(1048575), "1024.0KB");
    }

    #[test]
    fn test_human_bytes_megabytes() {
        assert_eq!(human_bytes(1048576), "1.0MB");
        assert_eq!(human_bytes(1572864), "1.5MB");
    }

    #[test]
    fn test_human_bytes_gigabytes() {
        assert_eq!(human_bytes(1073741824), "1.0GB");
        assert_eq!(human_bytes(1610612736), "1.5GB");
    }

    #[test]
    fn test_human_bytes_terabytes() {
        assert_eq!(human_bytes(1099511627776), "1.0TB");
        assert_eq!(
            human_bytes(u64::MAX),
            format!("{:.1}TB", u64::MAX as f64 / (1024.0_f64.powi(4)))
        );
    }

    #[test]
    fn test_get_hostname() {
        let hostname = get_hostname();
        assert!(!hostname.is_empty());
        assert!(hostname.len() <= 255);
    }

    #[test]
    fn test_progress_bar_edge_cases() {
        let bar = progress_bar(-10.0, 5);
        assert!(bar.starts_with('['));
        assert!(bar.ends_with(']'));

        let bar = progress_bar(150.0, 5);
        assert!(bar.starts_with('['));
        assert!(bar.ends_with(']'));

        let bar = progress_bar(50.0, 0);
        assert_eq!(bar, "[]");

        let bar = progress_bar(50.0, 1);
        assert!(bar.starts_with('['));
        assert!(bar.ends_with(']'));
    }

    #[test]
    fn test_parse_file_hint_basic() {
        assert_eq!(parse_file_hint("100"), Some(100));
        assert_eq!(parse_file_hint("0"), Some(0));
        assert_eq!(parse_file_hint("1234567"), Some(1234567));
    }

    #[test]
    fn test_parse_file_hint_units() {
        assert_eq!(parse_file_hint("10k"), Some(10_000));
        assert_eq!(parse_file_hint("10K"), Some(10_000));
        assert_eq!(parse_file_hint("2m"), Some(2_000_000));
        assert_eq!(parse_file_hint("2M"), Some(2_000_000));
        assert_eq!(parse_file_hint("1g"), Some(1_000_000_000));
        assert_eq!(parse_file_hint("1G"), Some(1_000_000_000));
    }

    #[test]
    fn test_parse_file_hint_decimals() {
        assert_eq!(parse_file_hint("1.5k"), Some(1_500));
        assert_eq!(parse_file_hint("2.5m"), Some(2_500_000));
        assert_eq!(parse_file_hint("0.5g"), Some(500_000_000));
    }

    #[test]
    fn test_parse_file_hint_whitespace() {
        assert_eq!(parse_file_hint("  10k  "), Some(10_000));
        assert_eq!(parse_file_hint("10 k"), Some(10_000));
        assert_eq!(parse_file_hint("\t2m\n"), Some(2_000_000));
    }

    #[test]
    fn test_parse_file_hint_invalid() {
        assert_eq!(parse_file_hint("invalid"), None);
        assert_eq!(parse_file_hint("10x"), None);
        assert_eq!(parse_file_hint(""), None);
        assert_eq!(parse_file_hint("k"), None);
        assert_eq!(parse_file_hint("10t"), None);
    }

    #[test]
    fn test_print_about() {
        print_about();
    }

    #[test]
    fn test_formatting_consistency() {
        let test_values = [0, 1, 1000, 1024, 1000000, 1048576];

        for &val in &test_values {
            let count_str = human_count(val);
            let bytes_str = human_bytes(val);

            assert!(!count_str.is_empty());
            assert!(!bytes_str.is_empty());

            if val > 0 {
                assert!(
                    bytes_str.ends_with('B')
                        || bytes_str.chars().last().unwrap().is_ascii_alphabetic()
                );
            }
        }
    }
}
