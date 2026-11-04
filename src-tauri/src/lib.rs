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
use purimaster_rules::{CleanOptions, CleanReport, DeleteMode, JunkScanResult, RuleSet};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager, State};

/// clientId → 取消标志
type TaskMap = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;
/// 最近一次完成的扫描索引（含其 clientId）
type LatestIndex = Arc<Mutex<Option<(String, Arc<ScanIndex>)>>>;
/// 清理规则集（启动时加载；加载失败时记录错误）
type Rules = Arc<Mutex<Option<Arc<RuleSet>>>>;

/// 应用状态：进行中的取消标志 + 最近一次扫描索引 + 清理规则。
/// 字段用 `Arc` 包装，便于把句柄克隆进工作线程（`State` 本身只是借用）。
#[derive(Default)]
struct AppState {
    tasks: TaskMap,
    latest: LatestIndex,
    rules: Rules,
    rules_error: Arc<Mutex<Option<String>>>,
    /// 当前活跃的垃圾扫描 clientId（单飞：新的垃圾扫描会取消旧的，避免并发堆积）
    junk_active: Arc<Mutex<Option<String>>>,
    /// 最近一次完成的垃圾扫描结果：切页不取消扫描，重新进入页面时取回展示
    junk_cache: Arc<Mutex<Option<JunkScanResult>>>,
    /// 最近一次清理报告：清理期间切换页签后，重新进入页面可取回（逐文件结果不丢失）
    last_clean: Arc<Mutex<Option<CleanReport>>>,
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

/// 启动一次垃圾扫描（只读）。
///
/// 事件（均带 `clientId`）：
/// - `junk://complete` `{ result: JunkScanResult }`
/// - `junk://cancelled`
///
/// 取消复用 `cancel_scan`（同一个取消标志注册表）。
/// 单飞：启动新扫描会自动取消仍在运行的旧垃圾扫描。
#[tauri::command]
fn scan_junk(app: AppHandle, state: State<'_, AppState>, client_id: String) -> Result<(), String> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err("非法的 clientId".into());
    }
    let rules = {
        let guard = state.rules.lock().unwrap();
        guard.clone().ok_or_else(|| {
            state
                .rules_error
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "清理规则未加载".to_string())
        })?
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let tasks = state.tasks.clone();
    // 单飞：取消上一条仍在运行的垃圾扫描，避免切页等场景堆积并发扫描
    {
        let mut active = state.junk_active.lock().unwrap();
        if let Some(prev) = active.as_ref()
            && let Some(flag) = tasks.lock().unwrap().get(prev)
        {
            flag.store(true, Ordering::Relaxed);
        }
        *active = Some(client_id.clone());
    }
    tasks
        .lock()
        .unwrap()
        .insert(client_id.clone(), cancel.clone());
    let junk_active = state.junk_active.clone();

    eprintln!("[purimaster] 垃圾扫描 {client_id} 启动");
    let junk_cache = state.junk_cache.clone();
    std::thread::spawn(move || {
        match rules.scan(&cancel) {
            Ok(result) => {
                eprintln!(
                    "[purimaster] 垃圾扫描 {client_id} 完成: {} files, {} bytes",
                    result.total_files, result.total_bytes
                );
                // 持久化到缓存：即使前端已切页（事件无人接收），重新进入页面也能取回
                *junk_cache.lock().unwrap() = Some(result.clone());
                emit(
                    &app,
                    "junk://complete",
                    serde_json::json!({ "clientId": client_id, "result": result }),
                );
            }
            Err(_) => {
                eprintln!("[purimaster] 垃圾扫描 {client_id} 已取消");
                emit(
                    &app,
                    "junk://cancelled",
                    serde_json::json!({ "clientId": client_id }),
                );
            }
        }
        tasks.lock().unwrap().remove(&client_id);
        let mut active = junk_active.lock().unwrap();
        if active.as_deref() == Some(client_id.as_str()) {
            *active = None;
        }
    });

    Ok(())
}

/// 取回最近一次完成的垃圾扫描结果（只读，不清空；随下次扫描完成而更新）。
#[tauri::command]
fn get_junk_cache(state: State<'_, AppState>) -> Option<JunkScanResult> {
    state.junk_cache.lock().unwrap().clone()
}

/// 清理选中的规则（ADR-0003：删除时重新枚举并逐文件重校验）。
///
/// 事件：`clean://complete` `{ report: CleanReport }`。
/// `permanent=false` 时移入回收站（默认）；`dry_run=true` 时只做安全检查不删除。
/// 报告中的 `report.cancelled` 表示被取消（已完成部分仍计入报告）。
///
/// 取消复用 `cancel_scan`。
#[tauri::command]
fn clean_junk(
    app: AppHandle,
    state: State<'_, AppState>,
    client_id: String,
    rule_ids: Vec<String>,
    permanent: bool,
    dry_run: bool,
) -> Result<(), String> {
    if client_id.is_empty() || client_id.len() > 128 {
        return Err("非法的 clientId".into());
    }
    if rule_ids.is_empty() {
        return Err("未选择任何清理规则".into());
    }
    let rules = {
        let guard = state.rules.lock().unwrap();
        guard.clone().ok_or_else(|| {
            state
                .rules_error
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "清理规则未加载".to_string())
        })?
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let tasks = state.tasks.clone();
    tasks
        .lock()
        .unwrap()
        .insert(client_id.clone(), cancel.clone());

    let mode = if permanent {
        DeleteMode::Permanent
    } else {
        DeleteMode::Trash
    };
    eprintln!(
        "[purimaster] 清理 {client_id} 启动: rules={rule_ids:?}, mode={mode:?}, dry_run={dry_run}"
    );
    let last_clean = state.last_clean.clone();
    std::thread::spawn(move || {
        let report = rules.clean(&rule_ids, &cancel, CleanOptions { mode, dry_run });
        eprintln!(
            "[purimaster] 清理 {client_id} 完成: deleted={} failed={} cancelled={}",
            report.deleted_files, report.failed_count, report.cancelled
        );
        // 先持久化报告：即使前端已切页/监听已注销，重新进入页面仍能取回
        *last_clean.lock().unwrap() = Some(report.clone());
        emit(
            &app,
            "clean://complete",
            serde_json::json!({ "clientId": client_id, "report": report }),
        );
        tasks.lock().unwrap().remove(&client_id);
    });

    Ok(())
}

/// 取走最近一次清理报告（取回后清空，避免重复弹出）。
///
/// 用于清理期间切换页签的场景：清理不在切页时取消（用户已确认删除），
/// 报告由下次挂载的页面通过本命令取回。
#[tauri::command]
fn take_clean_report(state: State<'_, AppState>) -> Option<CleanReport> {
    state.last_clean.lock().unwrap().take()
}

/// 解析规则目录：优先打包资源目录；开发模式（仅 debug 构建）从可执行文件向上最多找 4 层，
/// 防止安装目录异常时误加载无关祖先目录中的 `rules/junk.toml`。
fn resolve_rules_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    if let Ok(dir) = app.path().resolve("rules", BaseDirectory::Resource)
        && dir.join("junk.toml").is_file()
    {
        return Some(dir);
    }

    #[cfg(debug_assertions)]
    {
        let exe = std::env::current_exe().ok()?;
        let mut dir = exe.parent();
        for _ in 0..4 {
            let d = dir?;
            let candidate = d.join("rules");
            if candidate.join("junk.toml").is_file() {
                return Some(candidate);
            }
            dir = d.parent();
        }
    }
    None
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
        .setup(|app| {
            let state = app.state::<AppState>();
            match resolve_rules_dir(&app.handle().clone()) {
                Some(dir) => match RuleSet::load_dir(&dir) {
                    Ok(set) => {
                        eprintln!(
                            "[purimaster] 规则加载成功（{}）：{} 条规则 / {} 个分类",
                            dir.display(),
                            set.rules().len(),
                            set.categories().len()
                        );
                        *state.rules.lock().unwrap() = Some(Arc::new(set));
                    }
                    Err(e) => {
                        let msg = format!("{e}");
                        eprintln!("[purimaster] 规则加载失败（{}）：{msg}", dir.display());
                        *state.rules_error.lock().unwrap() = Some(msg);
                    }
                },
                None => {
                    let msg = "未找到规则目录（rules/junk.toml）".to_string();
                    eprintln!("[purimaster] {msg}");
                    *state.rules_error.lock().unwrap() = Some(msg);
                }
            }
            Ok(())
        })
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
            scan_junk,
            clean_junk,
            take_clean_report,
            get_junk_cache,
            ipc_hello
        ])
        .run(tauri::generate_context!())
        .expect("Purimaster 启动失败");
}
