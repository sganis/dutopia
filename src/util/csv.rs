// rs/src/util/csv.rs
use itoa::Buffer;

thread_local! {
    static U32BUF: std::cell::RefCell<Buffer> = std::cell::RefCell::new(Buffer::new());
    static U64BUF: std::cell::RefCell<Buffer> = std::cell::RefCell::new(Buffer::new());
    static I64BUF: std::cell::RefCell<Buffer> = std::cell::RefCell::new(Buffer::new());
}

#[inline]
pub fn push_u32(out: &mut Vec<u8>, v: u32) {
    U32BUF.with(|b| {
        let mut b = b.borrow_mut();
        out.extend_from_slice(b.format(v).as_bytes());
    });
}

#[inline]
pub fn push_u64(out: &mut Vec<u8>, v: u64) {
    U64BUF.with(|b| {
        let mut b = b.borrow_mut();
        out.extend_from_slice(b.format(v).as_bytes());
    });
}

#[inline]
pub fn push_i64(out: &mut Vec<u8>, v: i64) {
    I64BUF.with(|b| {
        let mut b = b.borrow_mut();
        out.extend_from_slice(b.format(v).as_bytes());
    });
}

#[inline]
pub fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while !s.is_empty() && s[0].is_ascii_whitespace() {
        s = &s[1..];
    }
    while !s.is_empty() && s[s.len() - 1].is_ascii_whitespace() {
        s = &s[..s.len() - 1];
    }
    s
}

#[inline]
pub fn parse_int<T>(b: Option<&[u8]>) -> T
where
    T: atoi::FromRadix10SignedChecked + Default,
{
    let s = trim_ascii(b.unwrap_or(b"0"));
    atoi::atoi::<T>(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_u32() {
        let mut out = Vec::new();
        push_u32(&mut out, 0);
        assert_eq!(out, b"0");

        out.clear();
        push_u32(&mut out, 42);
        assert_eq!(out, b"42");

        out.clear();
        push_u32(&mut out, u32::MAX);
        assert_eq!(out, b"4294967295");
    }

    #[test]
    fn test_push_u64() {
        let mut out = Vec::new();
        push_u64(&mut out, 0);
        assert_eq!(out, b"0");

        out.clear();
        push_u64(&mut out, 42);
        assert_eq!(out, b"42");

        out.clear();
        push_u64(&mut out, u64::MAX);
        assert_eq!(out, b"18446744073709551615");
    }

    #[test]
    fn test_push_i64() {
        let mut out = Vec::new();
        push_i64(&mut out, 0);
        assert_eq!(out, b"0");

        out.clear();
        push_i64(&mut out, 42);
        assert_eq!(out, b"42");

        out.clear();
        push_i64(&mut out, -42);
        assert_eq!(out, b"-42");

        out.clear();
        push_i64(&mut out, i64::MAX);
        assert_eq!(out, b"9223372036854775807");

        out.clear();
        push_i64(&mut out, i64::MIN);
        assert_eq!(out, b"-9223372036854775808");
    }

    #[test]
    fn test_csv_formatters_multiple_calls() {
        let mut out = Vec::new();
        push_u32(&mut out, 1);
        out.push(b',');
        push_u64(&mut out, 2);
        out.push(b',');
        push_i64(&mut out, -3);
        assert_eq!(out, b"1,2,-3");
    }

    #[test]
    fn test_trim_ascii() {
        assert_eq!(trim_ascii(b"hello"), b"hello");
        assert_eq!(trim_ascii(b"  hello  "), b"hello");
        assert_eq!(trim_ascii(b"\t\ntest\r\n"), b"test");
        assert_eq!(trim_ascii(b""), b"");
        assert_eq!(trim_ascii(b"   "), b"");
        assert_eq!(trim_ascii(b"\x00test\x00"), b"\x00test\x00");
    }

    #[test]
    fn test_trim_ascii_edge_cases() {
        assert_eq!(trim_ascii(b" "), b"");
        assert_eq!(trim_ascii(b"\t"), b"");
        assert_eq!(trim_ascii(b"\n"), b"");
        assert_eq!(trim_ascii(b"\r"), b"");
        assert_eq!(trim_ascii(b"a"), b"a");
        assert_eq!(trim_ascii(b" a "), b"a");
    }

    #[test]
    fn test_parse_int_u32() {
        assert_eq!(parse_int::<u32>(Some(b"42")), 42u32);
        assert_eq!(parse_int::<u32>(Some(b"  42  ")), 42u32);
        assert_eq!(parse_int::<u32>(Some(b"0")), 0u32);
        assert_eq!(parse_int::<u32>(None), 0u32);
        assert_eq!(parse_int::<u32>(Some(b"")), 0u32);
        assert_eq!(parse_int::<u32>(Some(b"invalid")), 0u32);
    }

    #[test]
    fn test_parse_int_i32() {
        assert_eq!(parse_int::<i32>(Some(b"42")), 42i32);
        assert_eq!(parse_int::<i32>(Some(b"-42")), -42i32);
        assert_eq!(parse_int::<i32>(Some(b"  -42  ")), -42i32);
        assert_eq!(parse_int::<i32>(Some(b"0")), 0i32);
        assert_eq!(parse_int::<i32>(None), 0i32);
    }

    #[test]
    fn test_parse_int_u64() {
        assert_eq!(
            parse_int::<u64>(Some(b"1844674407370955161")),
            1844674407370955161u64
        );
        assert_eq!(parse_int::<u64>(Some(b"0")), 0u64);
    }

    #[test]
    fn test_parse_int_overflow() {
        assert_eq!(parse_int::<u8>(Some(b"256")), 0u8);
        assert_eq!(parse_int::<i8>(Some(b"128")), 0i8);
    }

    #[test]
    fn test_csv_integration() {
        let mut out = Vec::new();

        push_u64(&mut out, 123);
        out.push(b',');
        push_u32(&mut out, 456);
        out.push(b',');
        push_i64(&mut out, -789);
        out.push(b'\n');

        let csv_line = String::from_utf8(out).unwrap();
        assert_eq!(csv_line, "123,456,-789\n");
    }
}
