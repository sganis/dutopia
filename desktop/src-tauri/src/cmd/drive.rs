// desktop/src-tauri/src/cmd/drive.rs
use dutopia::storage::get_all_storage_info;
use serde::Serialize;

#[derive(Serialize)]
pub struct DriveInfo {
    pub path: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[tauri::command]
pub fn list_drives() -> Result<Vec<DriveInfo>, String> {
    #[cfg(windows)]
    let out: Vec<DriveInfo> = get_all_storage_info()
        .map_err(|e| e.to_string())?
        .into_iter()
        .flat_map(|s| {
            let fs = s.filesystem.clone();
            let total = s.total_bytes;
            let used = s.used_bytes;
            s.mount_points
                .into_iter()
                .map(move |mp| DriveInfo {
                    path: mp,
                    filesystem: fs.clone(),
                    total_bytes: total,
                    used_bytes: used,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    #[cfg(not(windows))]
    let out: Vec<DriveInfo> = vec![DriveInfo {
        path: "/".to_string(),
        filesystem: "root".to_string(),
        total_bytes: 0,
        used_bytes: 0,
    }];

    Ok(out)
}
