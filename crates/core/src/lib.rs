//! purimaster-core：磁盘扫描/分析引擎。
//!
//! 平台无关：不得引入 `windows` 等 Win32 依赖（见 docs/decisions/0001-rust-tauri.md），
//! 平台特化逻辑属于 `purimaster-platform`（待建）。
//!
//! ## 设计要点
//!
//! - **不用 jwalk**（2026-11-03 实测）：jwalk 的 `DirEntry::metadata()` 每文件一次
//!   额外系统调用且串行执行，NVMe 上仅 ~3000 文件/秒；`std::fs::read_dir` 的
//!   metadata 在 Windows 上使用目录枚举缓存、零额外系统调用，rayon 并行后达
//!   ~72,000 文件/秒（全盘 C: 144 万文件 20 秒）。
//! - **扁平 arena 索引**（`Vec<NodeRecord>`，u32 下标寻址）：不存绝对路径（只存
//!   名字），内存随目录数线性且常量小；构建全程迭代式，无递归——极深目录树
//!   （数千层）不会栈溢出。
//! - **按需下钻**：一次扫描产出的索引保存在调用方（Tauri 壳），前端只拿根摘要
//!   与第一层子目录，展开时再按 node id 拉取——全盘扫描不再产生几十 MB 的单条
//!   IPC 载荷。
//! - **不可读目录显式标记**（`unreadable`）：区分"空目录"与"大小未知"，
//!   避免用户把无权限目录误判为空。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

use rayon::scope_fifo;
use serde::Serialize;

/// 每聚合多少个文件触发一次进度回调。
const PROGRESS_STEP: u64 = 1024;
/// 根节点的父节点哨兵值。
const NO_PARENT: u32 = u32::MAX;

/// 扫描进度快照。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScanProgress {
    pub files_seen: u64,
    pub bytes_seen: u64,
}

/// 目录节点摘要：IPC 传输用的精简形态（不带绝对路径，前端逐级拼接）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummary {
    pub id: u32,
    /// 目录名；根节点无文件名时为完整路径。
    pub name: String,
    /// 递归总大小（字节）。
    pub size: u64,
    /// 递归文件数。
    pub files: u64,
    /// 递归子目录数。
    pub dirs: u64,
    /// 因权限等原因未能读取：大小未知，区别于空目录。
    pub unreadable: bool,
    pub has_children: bool,
}

/// arena 中的一个目录节点。
struct NodeRecord {
    name: String,
    parent: u32,
    children: Vec<u32>,
    size: u64,
    files: u64,
    dirs: u64,
    unreadable: bool,
}

/// 一次扫描的完整结果索引（扁平存储，不递归）。
pub struct ScanIndex {
    nodes: Vec<NodeRecord>,
    pub files_seen: u64,
    pub bytes_seen: u64,
    /// 因权限不足等原因无法读取而跳过的条目数。
    pub skipped: u64,
}

impl ScanIndex {
    /// 根节点 id（恒为 0）。
    pub fn root_id(&self) -> u32 {
        0
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 单节点摘要。
    pub fn summary(&self, id: u32) -> Option<NodeSummary> {
        self.nodes.get(id as usize).map(|n| NodeSummary {
            id,
            name: n.name.clone(),
            size: n.size,
            files: n.files,
            dirs: n.dirs,
            unreadable: n.unreadable,
            has_children: !n.children.is_empty(),
        })
    }

    /// 直接子目录摘要（已按大小降序）。
    pub fn children(&self, id: u32) -> Vec<NodeSummary> {
        self.nodes
            .get(id as usize)
            .map(|n| n.children.iter().filter_map(|c| self.summary(*c)).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ScanError {
    /// 收到取消信号，扫描已中止。
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Cancelled => write!(f, "扫描已取消"),
            ScanError::Io(e) => write!(f, "扫描出错：{e}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// 单个文件条目（目录直属文件清单用）。
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
}

/// 目录直属文件清单（按大小降序，截断到 limit 条）。
#[derive(Debug, Clone, Serialize)]
pub struct FileListing {
    pub files: Vec<FileEntry>,
    /// 截断前的文件总数。
    pub total: u64,
}

/// 列出 `dir` 下直属文件（不含子目录），按大小降序，最多返回 `limit` 条。
///
/// `total` 为截断前总数，便于前端提示"还有 N 个未显示"。
/// 条目级读取失败会被跳过（展示辅助接口，尽力而为）；目录本身打不开才返回错误。
pub fn list_files(dir: &Path, limit: usize) -> Result<FileListing, ScanError> {
    if !dir.is_dir() {
        return Err(ScanError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("目录不存在：{}", dir.display()),
        )));
    }
    let mut files: Vec<FileEntry> = Vec::new();
    let entries = fs::read_dir(dir).map_err(ScanError::Io)?;
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let Ok(m) = entry.metadata() else { continue };
        files.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            size: m.len(),
        });
    }
    let total = files.len() as u64;
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    files.truncate(limit);
    Ok(FileListing { files, total })
}

/// 遍历任务共享上下文。
struct WalkCtx<'a> {
    cancel: &'a AtomicBool,
    skipped: AtomicU64,
    tx: mpsc::Sender<DirRecord>,
}

/// 遍历工作线程上报的单目录记录：直属文件统计 + 可读性。
struct DirRecord {
    path: PathBuf,
    direct_files: u64,
    direct_size: u64,
    /// 目录本身读取失败（如无权限）：大小未知。
    unreadable: bool,
}

/// 扫描 `root` 并按目录聚合文件大小，产出扁平索引。
///
/// - `cancel`：置为 `true` 后扫描尽快中止，返回 [`ScanError::Cancelled`]
/// - `progress`：每聚合 [`PROGRESS_STEP`] 个文件回调一次（在调用线程上执行）
///
/// 遍历为 rayon 多线程并行；聚合、建索引与进度回调在调用线程增量完成。
pub fn scan_space(
    root: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(ScanProgress),
) -> Result<ScanIndex, ScanError> {
    if !root.is_dir() {
        return Err(ScanError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("目录不存在：{}", root.display()),
        )));
    }
    let root = root.to_path_buf();

    let (tx, rx) = mpsc::channel::<DirRecord>();
    let mut skipped_total: u64 = 0;

    // 聚合状态：路径 → node id 的临时映射（扫描结束即丢弃，不留内存）
    let mut path_ids: HashMap<PathBuf, u32> = HashMap::new();
    let mut nodes: Vec<NodeRecord> = Vec::new();
    // 先插入根节点，保证 root_id = 0
    path_ids.insert(root.clone(), 0);
    nodes.push(NodeRecord {
        name: dir_name(&root),
        parent: NO_PARENT,
        children: Vec::new(),
        size: 0,
        files: 0,
        dirs: 0,
        unreadable: false,
    });

    let mut files_seen: u64 = 0;
    let mut bytes_seen: u64 = 0;
    let mut last_progress_files: u64 = 0;
    let mut root_unreadable = false;
    let mut walker_panicked = false;

    std::thread::scope(|s| {
        let walker = s.spawn(|| {
            let ctx = WalkCtx {
                cancel,
                skipped: AtomicU64::new(0),
                tx,
            };
            scope_fifo(|scope| walk_dir(root.clone(), &ctx, scope));
            ctx.skipped.load(Ordering::Relaxed)
        });

        // 消费线程：为目录链分配 arena id、聚合统计、回调进度
        for rec in rx {
            if rec.path == root && rec.unreadable {
                root_unreadable = true;
            }
            files_seen += rec.direct_files;
            bytes_seen += rec.direct_size;

            // 收集自身与祖先（含扫描根）的节点 id 链：ids[0]=自身，末尾=根
            let mut ids: Vec<u32> = Vec::new();
            for anc in rec.path.ancestors().take_while(|p| p.starts_with(&root)) {
                ids.push(get_or_create(anc, &mut path_ids, &mut nodes));
            }
            // 建立父链（根节点的祖先链只有自己，无需链接）
            for w in 1..ids.len() {
                nodes[ids[w - 1] as usize].parent = ids[w];
            }
            // 自身：直属文件统计 + 可读性
            {
                let own = &mut nodes[ids[0] as usize];
                own.size += rec.direct_size;
                own.files += rec.direct_files;
                if rec.unreadable {
                    own.unreadable = true;
                }
            }
            // 祖先：累计大小/文件数，且 dirs +1
            for &id in &ids[1..] {
                let anc = &mut nodes[id as usize];
                anc.size += rec.direct_size;
                anc.files += rec.direct_files;
                anc.dirs += 1;
            }

            if files_seen - last_progress_files >= PROGRESS_STEP {
                last_progress_files = files_seen;
                progress(ScanProgress {
                    files_seen,
                    bytes_seen,
                });
            }
        }

        match walker.join() {
            Ok(n) => skipped_total = n,
            // 遍历线程异常终止：不允许静默返回部分结果，转为显式错误
            Err(_) => walker_panicked = true,
        }
    });

    if walker_panicked {
        return Err(ScanError::Io(std::io::Error::other(
            "扫描线程异常退出（内部错误）",
        )));
    }

    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    if root_unreadable {
        return Err(ScanError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("无法读取目录（权限不足）：{}", root.display()),
        )));
    }

    // 建子节点列表（迭代，无递归）并排序
    for i in 1..nodes.len() {
        let p = nodes[i].parent;
        if p != NO_PARENT && (p as usize) < nodes.len() {
            nodes[p as usize].children.push(i as u32);
        }
    }
    for i in 0..nodes.len() {
        let mut ch = std::mem::take(&mut nodes[i].children);
        ch.sort_unstable_by(|&a, &b| {
            let na = &nodes[a as usize];
            let nb = &nodes[b as usize];
            nb.size.cmp(&na.size).then_with(|| na.name.cmp(&nb.name))
        });
        nodes[i].children = ch;
    }

    // 收尾进度：无论规模大小都保证调用方拿到最终计数
    progress(ScanProgress {
        files_seen,
        bytes_seen,
    });

    Ok(ScanIndex {
        nodes,
        files_seen,
        bytes_seen,
        skipped: skipped_total,
    })
}

/// 取路径的目录名；根目录（如 `C:\`）无 file_name 时返回完整路径。
fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// 慢目录追踪开关（环境变量 `PURIMASTER_TRACE_SLOW`，诊断用，进程内只读取一次）。
fn trace_slow_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("PURIMASTER_TRACE_SLOW").is_some())
}

/// 取得（必要时创建）路径对应的 node id。
fn get_or_create(
    path: &Path,
    path_ids: &mut HashMap<PathBuf, u32>,
    nodes: &mut Vec<NodeRecord>,
) -> u32 {
    if let Some(&id) = path_ids.get(path) {
        return id;
    }
    let id = nodes.len() as u32;
    nodes.push(NodeRecord {
        name: dir_name(path),
        parent: NO_PARENT,
        children: Vec::new(),
        size: 0,
        files: 0,
        dirs: 0,
        unreadable: false,
    });
    path_ids.insert(path.to_path_buf(), id);
    id
}

/// 并行遍历单个目录：直属文件记入 `DirRecord`，子目录作为新任务派发。
///
/// - 取消：每处理一个条目前检查；取消后立即返回（父链上的任务同样逐层退出）
/// - 读取失败的目录/条目计入 skipped；目录本身读不了时 `unreadable = true`
/// - 符号链接/联接点不入树不计大小（`file_type().is_dir()` 对其返回 false），避免环路
fn walk_dir<'env>(path: PathBuf, ctx: &'env WalkCtx<'env>, scope: &rayon::ScopeFifo<'env>) {
    if ctx.cancel.load(Ordering::Relaxed) {
        return;
    }
    // 慢目录追踪（诊断用）：PURIMASTER_TRACE_SLOW=1 时，单目录遍历超过 2 秒打印路径
    let trace_slow = trace_slow_enabled();
    let started = std::time::Instant::now();
    let mut rec = DirRecord {
        path: path.clone(),
        direct_files: 0,
        direct_size: 0,
        unreadable: false,
    };
    match fs::read_dir(&path) {
        Ok(entries) => {
            for entry in entries {
                if ctx.cancel.load(Ordering::Relaxed) {
                    return; // 不发送 rec：已取消，结果将被丢弃
                }
                let Ok(entry) = entry else {
                    ctx.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Ok(ft) = entry.file_type() else {
                    ctx.skipped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                if ft.is_dir() {
                    let child = entry.path();
                    scope.spawn_fifo(|scope| walk_dir(child, ctx, scope));
                } else if ft.is_file() {
                    // std 的 DirEntry::metadata 在 Windows 上零额外系统调用
                    match entry.metadata() {
                        Ok(m) => {
                            rec.direct_files += 1;
                            rec.direct_size += m.len();
                        }
                        Err(_) => {
                            ctx.skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        Err(_) => {
            // 目录本身读不了（如无权限）：大小未知，标记 unreadable，仍发送 rec
            rec.unreadable = true;
            ctx.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }
    if trace_slow && started.elapsed().as_secs_f32() > 2.0 {
        eprintln!(
            "[slow-dir {:.1}s] {}",
            started.elapsed().as_secs_f32(),
            path.display()
        );
    }
    let _ = ctx.tx.send(rec);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    fn scan(dir: &Path, cancel: &AtomicBool) -> Result<ScanIndex, ScanError> {
        scan_space(dir, cancel, &mut |_| {})
    }

    /// 便捷取根摘要
    fn root_of(idx: &ScanIndex) -> NodeSummary {
        idx.summary(idx.root_id()).unwrap()
    }

    #[test]
    fn aggregates_sizes_by_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), [0u8; 100]).unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("b.bin"), [0u8; 300]).unwrap();
        fs::write(dir.path().join("sub").join("c.txt"), [0u8; 50]).unwrap();

        let idx = scan(dir.path(), &AtomicBool::new(false)).unwrap();
        let root = root_of(&idx);
        assert_eq!(root.files, 3);
        assert_eq!(root.dirs, 1);
        assert_eq!(root.size, 450);
        assert_eq!(idx.files_seen, 3);
        assert_eq!(idx.bytes_seen, 450);
        assert!(!root.unreadable);

        let children = idx.children(idx.root_id());
        let sub = children.iter().find(|c| c.name == "sub").unwrap();
        assert_eq!(sub.size, 350);
        assert_eq!(sub.files, 2);
        assert_eq!(sub.dirs, 0);
        assert!(!sub.has_children);
    }

    #[test]
    fn keeps_empty_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();

        let idx = scan(dir.path(), &AtomicBool::new(false)).unwrap();
        assert_eq!(root_of(&idx).dirs, 1);
        let children = idx.children(idx.root_id());
        let empty = children.iter().find(|c| c.name == "empty").unwrap();
        assert_eq!(empty.size, 0);
        assert!(!empty.unreadable); // 空目录 ≠ 不可读
        assert!(!empty.has_children);
    }

    #[test]
    fn children_sorted_by_size_desc() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("small")).unwrap();
        fs::write(dir.path().join("small").join("x"), [0u8; 10]).unwrap();
        fs::create_dir(dir.path().join("large")).unwrap();
        fs::write(dir.path().join("large").join("y"), [0u8; 1000]).unwrap();

        let idx = scan(dir.path(), &AtomicBool::new(false)).unwrap();
        let names: Vec<String> = idx
            .children(idx.root_id())
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["large", "small"]);
    }

    #[test]
    fn cancel_stops_scan() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), [0u8; 1]).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            scan(dir.path(), &cancel),
            Err(ScanError::Cancelled)
        ));
    }

    #[test]
    fn missing_dir_is_error() {
        assert!(matches!(
            scan(
                Path::new("Z:\\__no_such_dir_purimaster__"),
                &AtomicBool::new(false)
            ),
            Err(ScanError::Io(_))
        ));
    }

    #[test]
    fn deep_tree_does_not_overflow_stack() {
        // 没有任何递归构建步骤；300 层足以覆盖原先 build_node 的浅层风险
        let dir = tempfile::tempdir().unwrap();
        let mut cur = dir.path().to_path_buf();
        for _ in 0..300 {
            cur = cur.join("d");
            fs::create_dir(&cur).unwrap();
        }
        fs::write(cur.join("leaf.txt"), [0u8; 7]).unwrap();

        let idx = scan(dir.path(), &AtomicBool::new(false)).unwrap();
        assert_eq!(idx.files_seen, 1);
        assert_eq!(idx.bytes_seen, 7);
        assert_eq!(root_of(&idx).dirs, 300);
    }

    #[test]
    fn list_files_sorted_desc_and_files_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("big.bin"), [0u8; 1000]).unwrap();
        fs::write(dir.path().join("small.txt"), [0u8; 10]).unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let listing = list_files(dir.path(), 100).unwrap();
        let names: Vec<&str> = listing.files.iter().map(|f| f.name.as_str()).collect();
        // 只含文件、不含子目录，按大小降序
        assert_eq!(names, ["big.bin", "small.txt"]);
        assert_eq!(listing.files[0].size, 1000);
        assert_eq!(listing.total, 2);
    }

    #[test]
    fn list_files_respects_limit_and_reports_total() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10u8 {
            fs::write(
                dir.path().join(format!("f{i}.bin")),
                vec![0u8; i as usize + 1],
            )
            .unwrap();
        }
        let listing = list_files(dir.path(), 3).unwrap();
        assert_eq!(listing.files.len(), 3);
        assert_eq!(listing.total, 10);
        // 截断后仍是最大的三个：9、8、7 字节
        assert_eq!(listing.files[0].size, 10);
        assert_eq!(listing.files[2].size, 8);
    }

    #[test]
    fn list_files_missing_dir_is_error() {
        assert!(matches!(
            list_files(Path::new("Z:\\__no_such_dir_purimaster__"), 100),
            Err(ScanError::Io(_))
        ));
    }

    #[test]
    fn progress_reports_final_count_for_small_scans() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), [0u8; 10]).unwrap();

        let mut calls = Vec::new();
        let idx = scan_space(dir.path(), &AtomicBool::new(false), &mut |p| calls.push(p)).unwrap();
        assert_eq!(idx.files_seen, 1);
        // 小扫描也要收到收尾进度，保证前端能拿到最终计数
        assert_eq!(calls.last().map(|p| p.files_seen), Some(1));
    }
}
