//! EBIDownload Tauri App
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ebidownload::app::*;
use ebidownload::logger::init_logging;
use tauri::Emitter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut log_rx = init_logging().expect("failed to initialize logging");

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_config_command,
            load_config_command,
            save_config_command,
            fetch_metadata_command,
            start_download_command,
            start_upload_command,
            cancel_download_command,
            cancel_upload_command,
            check_deps_command,
            install_deps_command,
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            tokio::spawn(async move {
                while let Some(entry) = log_rx.recv().await {
                    let _ = app_handle.emit("app-log", entry);
                }
            });
            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn main() {
    run();
}
