mod jobs;

use jobs::Job;

#[tauri::command]
fn get_launchd_jobs() -> Vec<Job> {
    jobs::list_launchd_jobs()
}

#[tauri::command]
fn get_cron_jobs() -> Vec<Job> {
    jobs::list_cron_jobs()
}

#[tauri::command]
fn set_enabled(label: String, path: String, scope: String, enable: bool) -> Result<String, String> {
    jobs::set_job_enabled(&label, &path, &scope, enable)
}

#[tauri::command]
fn delete_job(label: String, path: String, scope: String) -> Result<String, String> {
    jobs::delete_job(&label, &path, &scope)
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    jobs::reveal_in_finder(&path)
}

#[tauri::command]
fn open_file(path: String) -> Result<(), String> {
    jobs::open_path(&path)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_launchd_jobs,
            get_cron_jobs,
            set_enabled,
            delete_job,
            reveal,
            open_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
