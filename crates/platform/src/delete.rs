//! 安全删除原语（ADR-0003）。
//!
//! 全项目**唯一**允许执行删除的模块。校验顺序（全部通过才动手）：
//! 1. 目标根本身不得是链接/联接点（防整目录重定向）
//! 2. `symlink_metadata` 确认为普通文件（链接/目录/特殊文件拒绝）
//! 3. 目标根重新过系统关键路径保护检查（防扫描后目标被替换）
//! 4. 文件 `canonicalize` 后的真实路径必须位于目标根真实路径之下（防链接/`..` 替换）
//! 5. 文件真实路径不得命中保护路径
//! 6. 回收站模式：预检该卷是否真的支持回收（网络位置/回收站禁用/卷级
//!    NukeOnDelete 策略/超出配额 → 拒绝，避免 Shell 静默永久删除被误报为
//!    "已移入回收站"，2026-11-04 审查修复；NukeOnDelete 为 2026-11-06 补查）
//! 7. `dry_run` 时到第 6 步为止，不执行删除
//! 8. 删除前最后复检一次原路径的真实路径未变化，然后删除 `canon_file`
//!
//! 残留风险（ADR-0003 已记录）：删除调用与校验之间仍存在极小的时间窗（TOCTOU），
//! 需要同用户权限的并发进程精确抢跑；句柄级删除（按句柄而非路径删除）留待后续版本。
//!
//! 回收站删除使用 `trash` crate（Windows 上封装 Shell 文件操作 API）。

use std::fs;
use std::path::Path;

/// 删除方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// 移入回收站（默认，可恢复）
    Trash,
    /// 永久删除（不可恢复，必须经 UI 二次确认）
    Permanent,
}

impl DeleteMode {
    /// 报告展示用的稳定标识
    pub fn as_str(self) -> &'static str {
        match self {
            DeleteMode::Trash => "trash",
            DeleteMode::Permanent => "permanent",
        }
    }
}

#[derive(Debug)]
pub enum DeleteError {
    /// 安全校验未通过（拒绝删除）
    Rejected(String),
    /// 文件系统/回收站操作失败
    Io(String),
}

impl std::fmt::Display for DeleteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeleteError::Rejected(m) => write!(f, "拒绝删除：{m}"),
            DeleteError::Io(m) => write!(f, "删除失败：{m}"),
        }
    }
}

impl std::error::Error for DeleteError {}

/// 安全删除单个文件，返回文件大小（字节）。
///
/// - `root`：该文件所属的清理目标目录（必须仍通过保护检查）
/// - `dry_run`：只做全部安全检查，不实际删除
pub fn delete_file(
    root: &Path,
    file: &Path,
    mode: DeleteMode,
    dry_run: bool,
) -> Result<u64, DeleteError> {
    // 1) 目标根不得是链接/联接点：否则整棵删除会被重定向到用户未预览的位置
    let root_meta = fs::symlink_metadata(root)
        .map_err(|e| DeleteError::Io(format!("无法读取目标根属性：{e}")))?;
    if root_meta.file_type().is_symlink() {
        return Err(DeleteError::Rejected(
            "清理目标本身是链接或联接点，拒绝清理".into(),
        ));
    }

    // 2) 普通文件（不跟随链接）
    let meta = fs::symlink_metadata(file)
        .map_err(|e| DeleteError::Io(format!("无法读取文件属性：{e}")))?;
    if !meta.file_type().is_file() {
        return Err(DeleteError::Rejected(
            "不是普通文件（链接、目录或特殊文件）".into(),
        ));
    }
    let size = meta.len();

    // 3) 目标根重新校验（ADR-0003：不信任扫描阶段的结果）
    super::check_target(root).map_err(DeleteError::Rejected)?;

    // 4) 真实路径必须位于目标根真实路径之下
    let canon_file =
        fs::canonicalize(file).map_err(|e| DeleteError::Io(format!("无法解析文件路径：{e}")))?;
    let canon_root =
        fs::canonicalize(root).map_err(|e| DeleteError::Io(format!("无法解析目标路径：{e}")))?;
    if !canon_file.starts_with(&canon_root) {
        return Err(DeleteError::Rejected("文件不在清理目标范围内".into()));
    }

    // 5) 文件真实路径不得命中保护路径（目标内嵌联接点的兜底）
    if let Err(e) = super::check_target(&canon_file) {
        return Err(DeleteError::Rejected(e));
    }

    delete_checked_canonical(file, &canon_file, size, mode, dry_run)
}

/// 安全删除单个"用户点名"的文件（空间分析手动勾选删除用），返回文件大小。
///
/// 与 [`delete_file`] 的差别：没有批量清理的"目标根"上下文，因此跳过根相关校验
/// （步骤 1/3/4），但**逐文件安全检查一项不少**——普通文件确认、真实路径保护检查、
/// 回收站预检、删除前复检全部保留。受保护路径（系统/个人目录/OneDrive）下的文件
/// 依然会被拒绝。
pub fn delete_file_standalone(
    file: &Path,
    mode: DeleteMode,
    dry_run: bool,
) -> Result<u64, DeleteError> {
    // 普通文件（不跟随链接）
    let meta = fs::symlink_metadata(file)
        .map_err(|e| DeleteError::Io(format!("无法读取文件属性：{e}")))?;
    if !meta.file_type().is_file() {
        return Err(DeleteError::Rejected(
            "不是普通文件（链接、目录或特殊文件）".into(),
        ));
    }
    let size = meta.len();

    // 真实路径不得命中保护路径（系统目录、个人目录、OneDrive 等）
    let canon_file =
        fs::canonicalize(file).map_err(|e| DeleteError::Io(format!("无法解析文件路径：{e}")))?;
    if let Err(e) = super::check_target(&canon_file) {
        return Err(DeleteError::Rejected(e));
    }

    delete_checked_canonical(file, &canon_file, size, mode, dry_run)
}

/// 共享删除尾段：回收站预检 → dry-run 门 → 删除前复检 → 按真实路径删除。
///
/// 调用方负责完成各自的文件身份与保护校验；从这里开始两条删除路径完全一致
/// （单点收敛原则，ADR-0003）。
fn delete_checked_canonical(
    file: &Path,
    canon_file: &Path,
    size: u64,
    mode: DeleteMode,
    dry_run: bool,
) -> Result<u64, DeleteError> {
    // 回收站可行性预检（dry-run 也会执行，让用户提前看到不可回收的文件）
    if mode == DeleteMode::Trash {
        trash_preflight(canon_file, size).map_err(DeleteError::Rejected)?;
    }

    // dry-run 到此为止
    if dry_run {
        return Ok(size);
    }

    // 删除前最后复检：原路径的真实路径必须仍是 canon_file，随后按真实路径删除
    let again = fs::canonicalize(file)
        .map_err(|e| DeleteError::Io(format!("删除前无法解析文件路径：{e}")))?;
    if again != canon_file {
        return Err(DeleteError::Rejected("文件路径在删除前发生变化".into()));
    }
    match mode {
        // 使用已解析的真实路径执行删除，缩小路径替换的窗口
        DeleteMode::Trash => trash::delete(canon_file)
            .map_err(|e| DeleteError::Io(format!("移入回收站失败：{e}")))?,
        DeleteMode::Permanent => fs::remove_file(canon_file)
            .map_err(|e| DeleteError::Io(format!("永久删除失败：{e}")))?,
    }
    Ok(size)
}

/// 删除整个目录树（卸载残留清理用，永久删除语义、无回收站）。
///
/// - 目标不得是链接/联接点；canonicalize 后必须通过保护检查（系统/个人目录、
///   盘符根一律拒绝）——残留目录由名称匹配产生，这条检查是最后防线
/// - `dry_run = true` 时只做检查不删除
/// - 内部使用 `fs::remove_dir_all`：不跟随符号链接（链接本身被移除，目标保留）
pub fn remove_dir_tree_standalone(dir: &Path, dry_run: bool) -> Result<u64, DeleteError> {
    let meta =
        fs::symlink_metadata(dir).map_err(|e| DeleteError::Io(format!("无法读取目录属性：{e}")))?;
    if meta.file_type().is_symlink() {
        return Err(DeleteError::Rejected("目标是链接或联接点，拒绝删除".into()));
    }
    if !meta.is_dir() {
        return Err(DeleteError::Rejected("不是目录".into()));
    }
    let real =
        fs::canonicalize(dir).map_err(|e| DeleteError::Io(format!("无法解析目录路径：{e}")))?;
    super::check_target(&real).map_err(DeleteError::Rejected)?;

    // 统计内容规模（dry-run 报告与真实删除共用；上限防御异常深/大的树）
    let mut total_files: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut stack = vec![real.clone()];
    let mut visited = 0usize;
    while let Some(d) = stack.pop() {
        visited += 1;
        if visited > 100_000 {
            return Err(DeleteError::Rejected(
                "目录规模异常（超过 10 万项），拒绝删除".into(),
            ));
        }
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total_files += 1;
                if let Ok(m) = entry.metadata() {
                    total_bytes += m.len();
                }
            }
        }
    }

    if dry_run {
        return Ok(total_files);
    }
    fs::remove_dir_all(&real).map_err(|e| DeleteError::Io(format!("删除目录失败：{e}")))?;
    let _ = total_bytes;
    Ok(total_files)
}

// ---------------------------------------------------------------------------
// 回收站可行性预检（Windows）
// ---------------------------------------------------------------------------

/// 检查该文件是否真的能被放入回收站。
///
/// Shell 的回收站操作在无法回收时（网络位置、回收站禁用、卷级 NukeOnDelete
/// 策略、超出配额）会**静默永久删除**并返回成功——若不预检，删除报告会把
/// 不可恢复的删除宣称为"已移入回收站"。
///
/// 生产输入是 `fs::canonicalize` 的产物（`\\?\` verbatim 路径，见
/// [`delete_file`] 第 4/5 步），卷根推导与 API 调用都按该形态处理。
///
/// 已知限制：按卷容量 5%（Windows 10/11 默认上限）估算配额，用户自定义的更小
/// 配额（BitBucket\Volume\{GUID}\MaxCapacity）暂不读取（见 ADR-0003）。
#[cfg(windows)]
pub fn trash_preflight(path: &Path, size: u64) -> Result<(), String> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    let root = volume_root(path);
    let wide_root = to_wide(root.as_os_str());
    let drive_type = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
    // 事实收集与决策分离：决策逻辑抽成纯函数（可脱离注册表/真实卷单测）
    let no_recycle = no_recycle_files_enabled();
    let nuke = volume_nuke_on_delete(drive_letter_of_volume_root(&root));
    let (volume_total, volume_total_known) = volume_total_bytes(&root);
    evaluate_trash_policy(
        drive_type,
        no_recycle,
        nuke,
        volume_total_known,
        volume_total,
        size,
    )
}

#[cfg(not(windows))]
pub fn trash_preflight(_path: &Path, _size: u64) -> Result<(), String> {
    Ok(())
}

/// Windows `GetDriveTypeW` 返回值（稳定 ABI 常量，自带定义以便决策函数跨平台
/// 编译与单测；与 windows-sys 的等值性由测试 `preflight_drive_type_constants_match_windows_abi` 钉住）。
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;
const DRIVE_RAMDISK: u32 = 6;

/// 回收站预检的**纯决策函数**：输入全部事实，输出拒绝原因。检查顺序与
/// trash_preflight 原实现一致（盘符类型 → 用户级禁用 → 卷级 NukeOnDelete →
/// 容量配额），原实现仅在"卷级 NukeOnDelete"处新增一条拒绝条件。
#[cfg_attr(not(windows), allow(dead_code))]
fn evaluate_trash_policy(
    drive_type: u32,
    no_recycle_files: bool,
    nuke_on_delete: bool,
    volume_total_known: bool,
    volume_total: u64,
    size: u64,
) -> Result<(), String> {
    if drive_type == DRIVE_REMOTE || drive_type == DRIVE_CDROM {
        return Err("该位置是网络位置或光驱，无法使用回收站；如确认可丢弃请选择永久删除".into());
    }
    if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE && drive_type != DRIVE_RAMDISK {
        return Err("无法确认该位置支持回收站；如确认可丢弃请选择永久删除".into());
    }
    // 回收站被用户/策略禁用时，Shell 会直接永久删除
    if no_recycle_files {
        return Err("系统回收站已被禁用，无法安全回收；如确认可丢弃请选择永久删除".into());
    }
    // 卷级 NukeOnDelete 策略（BitBucket\Volume\{GUID}）同样让 Shell 跳过回收站
    // 直接永久删除——与回收站禁用同级危险，必须预检拒绝
    if nuke_on_delete {
        return Err(
            "该卷设置了 NukeOnDelete（删除不进回收站），无法安全回收；如确认可丢弃请选择永久删除"
                .into(),
        );
    }
    // 配额估算：Windows 10/11 默认回收站上限约为卷容量的 5%
    if volume_total_known && volume_total > 0 && size > volume_total / 20 {
        return Err(
            "文件大小超过该卷回收站容量估算上限（约 5%），无法放入回收站；如确认可丢弃请选择永久删除"
                .into(),
        );
    }
    Ok(())
}

#[cfg(windows)]
mod windows_impl {
    use super::to_wide;
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };

    /// 取路径所属卷根（`C:\`、`\\?\C:\` 或 `\\server\share\`）——`GetDriveTypeW`
    /// 需要根路径，且**尾反斜杠不可省**：`\\?\C:`（无尾反斜杠）会让
    /// GetDriveTypeW/GetDiskFreeSpaceExW 双双失败，预检将静默变成一律拒绝。
    pub(super) fn volume_root(path: &Path) -> PathBuf {
        let mut comps = path.components();
        if let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
            (comps.next(), comps.next())
        {
            let mut root = PathBuf::from(prefix.as_os_str());
            root.push("\\");
            return root;
        }
        path.parent().unwrap_or(path).to_path_buf()
    }

    /// 从卷根取盘符（小写）。UNC 卷根无盘符 → None（预检阶段网络位置已被拒绝，
    /// 实际到不了注册表检查；None 时按"所有条目都适用"保守处理）。
    pub(super) fn drive_letter_of_volume_root(root: &Path) -> Option<char> {
        let s = root.to_string_lossy();
        let colon = s.rfind(':')?;
        let c = s[..colon].chars().next_back()?;
        c.is_ascii_alphabetic().then(|| c.to_ascii_lowercase())
    }

    /// HKCU 的全局回收站禁用策略（`BitBucket\NoRecycleFiles`）。
    pub(super) fn no_recycle_files_enabled() -> bool {
        let subkey = to_wide(OsStr::new(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket",
        ));
        let value = to_wide(OsStr::new("NoRecycleFiles"));
        reg_dword(HKEY_CURRENT_USER, &subkey, &value) == Some(1)
    }

    /// 卷级 NukeOnDelete 策略（`BitBucket\Volume\{GUID}`，HKCU 与 HKLM 都查）：
    /// 该条目存在时 Shell 对该卷的删除**直接永久删除**、不进回收站。
    ///
    /// 归属判断：条目带 `DriveLetter` 且与文件所在盘符不符 → 不适用；
    /// 条目缺失/无法解析 `DriveLetter` 时按适用处理（保守方向：宁可拒绝，
    /// 交还用户选择永久删除，也不把永久删除误报成"已移入回收站"）。
    pub(super) fn volume_nuke_on_delete(drive: Option<char>) -> bool {
        const VOLUME_KEY: &str =
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket\\Volume";
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let subkey = to_wide(OsStr::new(VOLUME_KEY));
            let mut key: HKEY = std::ptr::null_mut();
            // 多数系统没有 Volume 子键或该 hive 下没有：视为无策略
            if unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
                continue;
            }
            let mut found = false;
            // {GUID} 子键名最长 39 字符 + 终止符，64 留足余量
            let mut name_buf = [0u16; 64];
            let mut index: u32 = 0;
            loop {
                let mut name_len = name_buf.len() as u32;
                let rc = unsafe {
                    RegEnumKeyExW(
                        key,
                        index,
                        name_buf.as_mut_ptr(),
                        &mut name_len,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if rc != 0 {
                    break; // ERROR_NO_MORE_ITEMS 或读取失败：枚举结束
                }
                index += 1;
                let entry = to_wide(OsStr::new(&String::from_utf16_lossy(
                    &name_buf[..name_len as usize],
                )));
                if reg_dword(key, &entry, to_wide(OsStr::new("NukeOnDelete")).as_slice()) == Some(1)
                    && applies_to_drive(key, &entry, drive)
                {
                    found = true;
                    break;
                }
            }
            unsafe { RegCloseKey(key) };
            if found {
                return true;
            }
        }
        false
    }

    /// 条目是否适用于目标盘符（见 [`volume_nuke_on_delete`] 的归属规则）。
    fn applies_to_drive(key: HKEY, subkey: &[u16], drive: Option<char>) -> bool {
        let letter = reg_string(key, subkey, &to_wide(OsStr::new("DriveLetter")));
        match (
            letter.as_deref().and_then(|s| s.trim().chars().next()),
            drive,
        ) {
            (Some(c), Some(d)) => c.to_ascii_lowercase() == d,
            // 盘符拼不出来（空值/异常条目）或目标无盘符 → 保守视为适用
            _ => true,
        }
    }

    /// 卷总容量（字节）；查询失败返回 known=false（预检跳过配额判断，与原行为一致）。
    pub(super) fn volume_total_bytes(root: &Path) -> (u64, bool) {
        let wide = to_wide(root.as_os_str());
        let mut free_available: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_available,
                &mut total,
                &mut total_free,
            )
        };
        (total, ok != 0)
    }

    /// 读取 `key\subkey` 下的 DWORD 值；键/值不存在或类型不符返回 None。
    fn reg_dword(key: HKEY, subkey: &[u16], value: &[u16]) -> Option<u32> {
        let mut out: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                key,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut out as *mut u32).cast(),
                &mut len,
            )
        };
        (rc == 0).then_some(out)
    }

    /// 读取 `key\subkey` 下的 REG_SZ 值；键/值不存在返回 None。
    fn reg_string(key: HKEY, subkey: &[u16], value: &[u16]) -> Option<String> {
        let mut buf = [0u16; 64];
        let mut len = (buf.len() * std::mem::size_of::<u16>()) as u32;
        let rc = unsafe {
            RegGetValueW(
                key,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }
        Some(
            String::from_utf16_lossy(&buf)
                .trim_end_matches('\0')
                .to_string(),
        )
    }
}

#[cfg(windows)]
use windows_impl::{
    drive_letter_of_volume_root, no_recycle_files_enabled, volume_nuke_on_delete, volume_root,
    volume_total_bytes,
};

#[cfg(windows)]
fn to_wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_file(dir: &Path, name: &str, bytes: usize) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, vec![0u8; bytes]).unwrap();
        path
    }

    #[test]
    fn dry_run_performs_checks_but_keeps_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "junk.bin", 42);
        let size = delete_file(dir.path(), &file, DeleteMode::Permanent, true).unwrap();
        assert_eq!(size, 42);
        assert!(file.exists(), "dry-run 不得删除文件");
    }

    // -----------------------------------------------------------------------
    // delete_file_standalone（用户点名单文件删除）
    // -----------------------------------------------------------------------

    #[test]
    fn standalone_dry_run_keeps_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "picked.bin", 42);
        let size = delete_file_standalone(&file, DeleteMode::Permanent, true).unwrap();
        assert_eq!(size, 42);
        assert!(file.exists(), "dry-run 不得删除文件");
    }

    #[test]
    fn standalone_permanent_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "picked.bin", 10);
        delete_file_standalone(&file, DeleteMode::Permanent, false).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn standalone_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            delete_file_standalone(dir.path(), DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[test]
    fn standalone_rejects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("__no_such__.bin");
        assert!(matches!(
            delete_file_standalone(&missing, DeleteMode::Permanent, true),
            Err(DeleteError::Io(_))
        ));
    }

    #[test]
    fn standalone_rejects_protected_file() {
        // 真实系统路径保护：System32 下的文件即使被用户点名也必须拒绝；
        // 依赖真实环境，缺失时跳过（显式打印避免假绿）
        let Some(sysroot) = std::env::var_os("SystemRoot") else {
            eprintln!("跳过：SystemRoot 不可用");
            return;
        };
        let hosts = PathBuf::from(sysroot)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts");
        if !hosts.is_file() {
            eprintln!("跳过：hosts 文件不存在");
            return;
        }
        assert!(matches!(
            delete_file_standalone(&hosts, DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    // -----------------------------------------------------------------------
    // remove_dir_tree_standalone（卸载残留目录删除）
    // -----------------------------------------------------------------------

    #[test]
    fn dir_tree_dry_run_counts_but_keeps() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.bin"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("sub").join("b.bin"), [0u8; 20]).unwrap();

        let count = remove_dir_tree_standalone(dir.path(), true).unwrap();
        assert_eq!(count, 2);
        assert!(dir.path().join("a.bin").exists(), "dry-run 不得删除");
    }

    #[test]
    fn dir_tree_removes_nested_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("deep").join("er")).unwrap();
        fs::write(dir.path().join("deep").join("er").join("x.bin"), [0u8; 5]).unwrap();

        remove_dir_tree_standalone(dir.path(), false).unwrap();
        assert!(!dir.path().exists(), "目录树应被整体删除");
    }

    #[test]
    fn dir_tree_rejects_drive_root() {
        assert!(matches!(
            remove_dir_tree_standalone(Path::new(r"C:\"), true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[test]
    fn dir_tree_rejects_protected_dir() {
        let Some(sysroot) = std::env::var_os("SystemRoot") else {
            eprintln!("跳过：SystemRoot 不可用");
            return;
        };
        let system32 = PathBuf::from(sysroot).join("System32");
        if !system32.is_dir() {
            eprintln!("跳过：System32 不存在");
            return;
        }
        assert!(matches!(
            remove_dir_tree_standalone(&system32, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[test]
    fn permanent_delete_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "junk.bin", 10);
        delete_file(dir.path(), &file, DeleteMode::Permanent, false).unwrap();
        assert!(!file.exists());
    }

    #[test]
    fn rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        assert!(matches!(
            delete_file(dir.path(), &sub, DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[test]
    fn rejects_file_outside_root() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let file = temp_file(other.path(), "outside.bin", 10);
        assert!(matches!(
            delete_file(root.path(), &file, DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[test]
    fn rejects_protected_root() {
        // 依赖真实系统路径；缺失时跳过（显式打印，避免"绿色但零断言"的假象）
        let Some(sysroot) = std::env::var_os("SystemRoot") else {
            eprintln!("跳过：SystemRoot 不可用");
            return;
        };
        let root = PathBuf::from(sysroot).join("System32");
        let file = root.join("drivers").join("etc").join("hosts");
        if !file.is_file() {
            eprintln!("跳过：hosts 文件不存在");
            return;
        }
        assert!(matches!(
            delete_file(&root, &file, DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn rejects_symlink_file() {
        use std::os::windows::fs::symlink_file;
        let dir = tempfile::tempdir().unwrap();
        let target = temp_file(dir.path(), "target.bin", 5);
        let link = dir.path().join("link.bin");
        // 创建符号链接通常需要开发者模式/管理员；无权限时跳过
        if symlink_file(&target, &link).is_err() {
            eprintln!("跳过：无权限创建符号链接");
            return;
        }
        assert!(matches!(
            delete_file(dir.path(), &link, DeleteMode::Permanent, true),
            Err(DeleteError::Rejected(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn trash_preflight_allows_small_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "small.bin", 16);
        assert!(trash_preflight(&file, 16).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn preflight_drive_type_constants_match_windows_abi() {
        // 决策函数自带 DRIVE_* 常量以便跨平台单测；必须与 windows-sys 逐项等值
        use windows_sys::Win32::System::WindowsProgramming as W;
        assert_eq!(super::DRIVE_FIXED, W::DRIVE_FIXED);
        assert_eq!(super::DRIVE_REMOVABLE, W::DRIVE_REMOVABLE);
        assert_eq!(super::DRIVE_REMOTE, W::DRIVE_REMOTE);
        assert_eq!(super::DRIVE_CDROM, W::DRIVE_CDROM);
        assert_eq!(super::DRIVE_RAMDISK, W::DRIVE_RAMDISK);
    }

    #[cfg(windows)]
    #[test]
    fn trash_preflight_allows_verbatim_canonicalized_path() {
        // 生产输入形态：delete_file / delete_file_standalone 传给预检的是
        // fs::canonicalize 的产物（\\?\ verbatim 路径）。volume_root 对
        // VerbatimDisk 前缀的 push("\\") 处理必须保持——丢了尾反斜杠，
        // GetDriveTypeW/GetDiskFreeSpaceExW 对 \\?\C: 双双失败，预检会静默
        // 变成一律拒绝（2026-11-06 审查）。
        let dir = tempfile::tempdir().unwrap();
        let file = temp_file(dir.path(), "verbatim.bin", 16);
        // 环境探测：tempdir 可能在不支持回收站的卷上，此时显式跳过而非假绿
        if let Err(e) = trash_preflight(&file, 16) {
            eprintln!("跳过：tempdir 所在卷不支持回收站预检（{e}）");
            return;
        }
        let canon = fs::canonicalize(&file).unwrap();
        assert!(
            canon.to_string_lossy().starts_with(r"\\?\"),
            "前提：canonicalize 应产出 verbatim 路径，实际 {}",
            canon.display()
        );
        assert!(
            trash_preflight(&canon, 16).is_ok(),
            "verbatim 形态的小文件必须通过预检"
        );
    }

    // -----------------------------------------------------------------------
    // 回收站预检纯决策函数（evaluate_trash_policy）
    // -----------------------------------------------------------------------

    #[test]
    fn preflight_policy_rejects_network_and_cdrom() {
        // DRIVE_REMOTE=4 / DRIVE_CDROM=5：明确拒绝
        assert!(evaluate_trash_policy(4, false, false, false, 0, 16).is_err());
        assert!(evaluate_trash_policy(5, false, false, false, 0, 16).is_err());
        // DRIVE_UNKNOWN=0 等未知类型：保守拒绝
        assert!(evaluate_trash_policy(0, false, false, false, 0, 16).is_err());
        assert!(evaluate_trash_policy(1, false, false, false, 0, 16).is_err());
    }

    #[test]
    fn preflight_policy_rejects_disabled_recycle_bin() {
        assert!(evaluate_trash_policy(3, true, false, false, 0, 16).is_err());
    }

    #[test]
    fn preflight_policy_rejects_nuke_on_delete_volume() {
        // safety-1：卷级 NukeOnDelete 让 Shell 直接永久删除，必须预检拒绝
        assert!(evaluate_trash_policy(3, false, true, false, 0, 16).is_err());
    }

    #[test]
    fn preflight_policy_allows_when_policies_clear() {
        // 固定盘/可移动盘/内存盘 + 无策略 + 小文件 → 允许
        assert!(evaluate_trash_policy(3, false, false, false, 0, 16).is_ok());
        assert!(evaluate_trash_policy(2, false, false, false, 0, 16).is_ok());
        assert!(evaluate_trash_policy(6, false, false, false, 0, 16).is_ok());
        // 配额未知（查询失败）时跳过配额判断，与原行为一致
        assert!(evaluate_trash_policy(3, false, false, false, 0, u64::MAX).is_ok());
    }

    #[test]
    fn preflight_policy_rejects_oversized_file() {
        // 超过卷容量 5% 估算上限：拒绝（与原实现同界：size > total/20）
        let total = 1_000_000u64;
        assert!(evaluate_trash_policy(3, false, false, true, total, total / 20 + 1).is_err());
        // 恰在边界内（含 total/20 本身）仍允许
        assert!(evaluate_trash_policy(3, false, false, true, total, total / 20).is_ok());
    }
}
