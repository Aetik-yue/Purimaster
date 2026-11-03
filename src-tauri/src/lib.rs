//! Tauri 壳层：只做 IPC 胶水与任务/索引生命周期管理，领域逻辑在 purimaster-core。
//!
//! IPC 约定（2026-11-03 代码审查后的定案）：
//! - 前端**先**生成 `clientId` 再调用 `start_scan`，所有事件都携带 `clientId`，
//!   彻底避免"事件先于 invoke 返回而无法关联"的竞态。
//! - `scan://complete` 只携带根摘要 + 第一层子目录（几 KB），更深层由
//!   `dir_children(clientId, nodeId)` 按需拉取，避免全盘扫描产生几十 MB 的单条载荷。
//! - 扫描产出的索引（`ScanIndex`）保存在壳层状态中，只保留最近一次。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use purimaster_core::{FileListing, NodeSummary, ScanError, ScanIndex, ScanProgress, scan_space};
use tauri::{AppHandle, Emitter, Manager, State};

/// clientId → 取消标志
type TaskMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;
/// 最近一次完成的扫描索引（含其 clientId）
type LatestIndex = Arc<Mutex<Option<(String, Arc<ScanIndex>)>>>;

/// 应用状态：进行中的取消标志 + 最近一次扫描索引。
/// 字段用 `Arc` 包装，便于把句柄克隆进工作线程（`State` 本身只是借用）。
#[derive(Default)]
struct AppState {
    tasks: TaskMap,
    latest: LatestIndex,
}

/// 统一的事件发送：失败必须可见（stderr），不允许静默吞掉。
fn emit(app: &AppHandle, event: &str, payload: serde_json::Value) {
    if let Err(e) = app.emit(event, payload) {
        eprintln!("[purimaster] 事件 {event} 发送失败: {e}");
    }
}

/// IPC 事件通路自检：前端注册完监听后调用，本命令回发一个 `scan://hello` 事件。
#[tauri::command]
fn ipc_hello(app: AppHandle) {
    emit(&app, "scan://hello", serde_json::json!({ "ok": true }));
}

/// 启动一次空间扫描。
///
/// 事件（均带 `clientId`）：
/// - `scan://progress` `{ filesSeen, bytesSeen }`
/// - `scan://complete` `{ filesSeen, bytesSeen, skipped, root, rootChildren }`
/// - `scan://cancelled`
/// - `scan://failed` `{ message }`
#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    client_id: String,
    root: String,
) -> Result<(), String> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err("非法的 clientId".into());
    }
    let path = std::path::PathBuf::from(&root);
    if !path.is_dir() {
        return Err(format!("目录不存在：{root}"));
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let tasks = state.tasks.clone();
    let latest = state.latest.clone();
    tasks
        .lock()
        .unwrap()
        .insert(client_id.clone(), cancel.clone());

    eprintln!("[purimaster] 扫描任务 {client_id} 启动: {root}");
    std::thread::spawn(move || {
        let mut last_emit = Instant::now() - Duration::from_secs(1);
        let mut progress = |p: ScanProgress| {
            // 节流：事件最多 ~6.6 次/秒，避免高频推送卡 UI
            if last_emit.elapsed() >= Duration::from_millis(150) {
                last_emit = Instant::now();
                emit(
                    &app,
                    "scan://progress",
                    serde_json::json!({
                        "clientId": client_id,
                        "filesSeen": p.files_seen,
                        "bytesSeen": p.bytes_seen,
                    }),
                );
            }
        };

        match scan_space(&path, &cancel, &mut progress) {
            Ok(index) => {
                let files_seen = index.files_seen;
                let bytes_seen = index.bytes_seen;
                let skipped = index.skipped;
                let root_summary = index.summary(index.root_id()).unwrap();
                let root_children = index.children(index.root_id());
                eprintln!(
                    "[purimaster] 扫描任务 {client_id} 完成: {files_seen} files, {} dirs, {skipped} skipped",
                    root_summary.dirs
                );
                *latest.lock().unwrap() = Some((client_id.clone(), Arc::new(index)));
                emit(
                    &app,
                    "scan://complete",
                    serde_json::json!({
                        "clientId": client_id,
                        "filesSeen": files_seen,
                        "bytesSeen": bytes_seen,
                        "skipped": skipped,
                        "root": root_summary,
                        "rootChildren": root_children,
                    }),
                );
            }
            Err(ScanError::Cancelled) => {
                eprintln!("[purimaster] 扫描任务 {client_id} 已取消");
                emit(
                    &app,
                    "scan://cancelled",
                    serde_json::json!({ "clientId": client_id }),
                );
            }
            Err(ScanError::Io(e)) => {
                eprintln!("[purimaster] 扫描任务 {client_id} 失败: {e}");
                emit(
                    &app,
                    "scan://failed",
                    serde_json::json!({ "clientId": client_id, "message": e.to_string() }),
                );
            }
        }

        tasks.lock().unwrap().remove(&client_id);
    });

    Ok(())
}

/// 请求取消一次扫描。实际中止是异步的，以 `scan://cancelled` 事件为准。
#[tauri::command]
fn cancel_scan(state: State<'_, AppState>, client_id: String) {
    if let Some(flag) = state.tasks.lock().unwrap().get(&client_id) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// 按需拉取某节点的直接子目录（按大小降序）。
#[tauri::command]
fn dir_children(
    state: State<'_, AppState>,
    client_id: String,
    node_id: u32,
) -> Result<Vec<NodeSummary>, String> {
    let guard = state.latest.lock().unwrap();
    match guard.as_ref() {
        Some((id, index)) if id == &client_id => Ok(index.children(node_id)),
        _ => Err("扫描结果已失效，请重新扫描".into()),
    }
}

/// 列出目录直属文件清单（按大小降序，最多 `limit` 条，默认 200）。
///
/// async + spawn_blocking：枚举在后台线程执行，不阻塞 WebView 主线程。
#[tauri::command]
async fn list_files(dir: String, limit: Option<u32>) -> Result<FileListing, String> {
    let limit = limit.unwrap_or(200) as usize;
    tauri::async_runtime::spawn_blocking(move || {
        purimaster_core::list_files(Path::new(&dir), limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .on_window_event(|window, event| {
            // 关闭窗口时统一发出取消信号，避免扫描线程在退出期间继续阻塞
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                let state = window.app_handle().state::<AppState>();
                for flag in state.tasks.lock().unwrap().values() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            start_scan,
            cancel_scan,
            dir_children,
            list_files,
            ipc_hello
        ])
        .run(tauri::generate_context!())
        .expect("Purimaster 启动失败");
}
