// rs/src/util/platform.rs
use std::path::Path;

#[cfg(windows)]
pub fn get_rid(path: &Path) -> std::io::Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use std::{io, iter, ptr};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, IsValidSid, OWNER_SECURITY_INFORMATION,
    };

    let path_len = path.as_os_str().len();
    if path_len > 32767 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Path too long"));
    }

    let mut wide = Vec::with_capacity(path_len + 1);

    match std::panic::catch_unwind(|| {
        wide.extend(path.as_os_str().encode_wide().chain(iter::once(0)));
        wide
    }) {
        Ok(wide_path) => wide = wide_path,
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "Failed to allocate memory for path conversion",
            ));
        }
    }

    let mut p_owner_sid: *mut core::ffi::c_void = ptr::null_mut();
    let mut p_sd: *mut core::ffi::c_void = ptr::null_mut();

    let err = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut p_owner_sid as *mut _ as *mut _,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut p_sd,
        )
    };

    if err != 0 {
        return Err(io::Error::from_raw_os_error(err as i32));
    }

    let rid = unsafe {
        if IsValidSid(p_owner_sid) == 0 {
            LocalFree(p_sd as *mut _);
            return Err(io::Error::new(io::ErrorKind::Other, "Invalid SID"));
        }
        let count = *GetSidSubAuthorityCount(p_owner_sid) as u32;
        if count == 0 {
            LocalFree(p_sd as *mut _);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "SID has no subauthorities",
            ));
        }
        let p_last = GetSidSubAuthority(p_owner_sid, count - 1);
        let val = *p_last as u32;
        LocalFree(p_sd as *mut _);
        val
    };

    Ok(rid)
}

pub fn fs_used_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use libc::{statvfs, statvfs as statvfs_t};
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let p = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { statvfs(p.as_ptr(), &mut s as *mut statvfs_t) };
        if rc != 0 {
            return None;
        }

        let bsize = if s.f_frsize != 0 {
            s.f_frsize
        } else {
            s.f_bsize
        } as u64;
        let used_blocks = s.f_blocks.saturating_sub(s.f_bfree) as u64;
        return Some(used_blocks.saturating_mul(bsize));
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut free_avail: u64 = 0;
        let mut total: u64 = 0;
        let mut free_total: u64 = 0;

        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_avail as *mut u64,
                &mut total as *mut u64,
                &mut free_total as *mut u64,
            )
        };
        if ok == 0 {
            return None;
        }

        return Some(total.saturating_sub(free_total));
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_fs_used_bytes_unix() {
        let result = fs_used_bytes(Path::new("/"));
        assert!(result.is_some());

        let result = fs_used_bytes(Path::new("/non/existent/path"));
        assert!(result.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn test_fs_used_bytes_windows() {
        let result = fs_used_bytes(Path::new("C:\\"));
        let _ = result;

        let result = fs_used_bytes(Path::new("."));
        assert!(result.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn test_get_rid_windows() {
        let result = get_rid(Path::new("."));
        match result {
            Ok(rid) => {
                assert!(rid > 0);
            }
            Err(_) => {}
        }

        let result = get_rid(Path::new("C:\\non\\existent\\path"));
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_get_rid_path_too_long() {
        let long_path = "C:\\".to_string() + &"a\\".repeat(10000);
        let result = get_rid(Path::new(&long_path));
        assert!(result.is_err());
    }
}
