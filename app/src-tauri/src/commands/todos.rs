//! The global manual todo list (#335): boot-time load and wholesale save.

use std::sync::Arc;

use serde_json::Value;

use crate::db::Database;

/// Every stored todo, in saved order, as the opaque JSON the frontend
/// wrote. Unparseable rows are skipped with a warning.
#[tauri::command]
pub(crate) async fn db_load_global_todos(
    db: tauri::State<'_, Arc<Database>>,
) -> Result<Vec<Value>, String> {
    let db = Arc::clone(&db);
    tauri::async_runtime::spawn_blocking(move || db.load_global_todos())
        .await
        .map_err(|e| e.to_string())?
}

/// Replaces the stored list wholesale. An empty list is a legitimate
/// save, unlike rooms: the frontend only saves after a successful load.
#[tauri::command]
pub(crate) async fn db_save_global_todos(
    todos: Vec<Value>,
    db: tauri::State<'_, Arc<Database>>,
) -> Result<(), String> {
    let db = Arc::clone(&db);
    let seq = db.next_save_seq();
    tauri::async_runtime::spawn_blocking(move || db.save_global_todos_seq(&todos, seq))
        .await
        .map_err(|e| e.to_string())?
}
