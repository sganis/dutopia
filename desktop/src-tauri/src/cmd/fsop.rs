// desktop/src-tauri/src/cmd/fsop.rs
//
// Platform-specific filesystem action commands: reveal in native file
// manager, open a terminal at a path, and move-to-trash delete.

use std::path::PathBuf;
use std::process::Command;

#[tauri::command]
pub fn reveal_in_path(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);

    #[cfg(target_os = "windows")]
    {
        // `explorer /select,<path>` opens Explorer with the item highlighted.
        Command::new("explorer")
            .arg(format!("/select,{}", p.display()))
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        // xdg-open can't highlight a file. Fall back to opening the containing dir.
        let target = if p.is_dir() {
            p.clone()
        } else {
            p.parent().unwrap_or(&p).to_path_buf()
        };
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = p;
        Err("reveal_in_path: unsupported platform".to_string())
    }
}

#[tauri::command]
pub fn open_terminal(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let dir = if p.is_dir() {
        p.clone()
    } else {
        p.parent().unwrap_or(&p).to_path_buf()
    };

    #[cfg(target_os = "windows")]
    {
        // Try Windows Terminal; fall back to cmd.exe.
        if Command::new("wt")
            .args(["-d", &dir.display().to_string()])
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        Command::new("cmd")
            .args(["/C", "start", "cmd"])
            .current_dir(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .args(["-a", "Terminal", &dir.display().to_string()])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        // Try $TERMINAL, then common defaults.
        let candidates: Vec<String> = std::env::var("TERMINAL")
            .ok()
            .into_iter()
            .chain([
                "x-terminal-emulator".to_string(),
                "gnome-terminal".to_string(),
                "konsole".to_string(),
                "xterm".to_string(),
            ])
            .collect();
        for term in candidates {
            if Command::new(&term).current_dir(&dir).spawn().is_ok() {
                return Ok(());
            }
        }
        Err("No terminal emulator found".to_string())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = dir;
        Err("open_terminal: unsupported platform".to_string())
    }
}

#[tauri::command]
pub async fn delete_path(path: String) -> Result<(), String> {
    // trash::delete is blocking and can be slow on Windows for large folders
    // (the shell API enumerates every child). Run it off the Tokio runtime
    // threads so IPC + UI events keep flowing.
    tokio::task::spawn_blocking(move || trash::delete(&path).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
