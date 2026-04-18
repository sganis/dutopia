// desktop/src-tauri/src/lib.rs
mod cmd;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let app_state = state::init(&handle).expect("failed to init app state");
            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd::list_drives,
            cmd::scan,
            cmd::cancel_scan,
            cmd::get_recent_paths,
            cmd::set_recent_paths,
            cmd::get_users,
            cmd::get_folders,
            cmd::get_files,
            cmd::reveal_in_path,
            cmd::open_terminal,
            cmd::delete_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
