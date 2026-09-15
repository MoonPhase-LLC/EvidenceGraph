// No IPC commands are exposed yet. The backend process, its supervision, and
// its authenticated IPC surface are introduced in S1-03/S1-04, not here.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
