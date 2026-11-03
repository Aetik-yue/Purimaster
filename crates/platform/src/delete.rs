//! 安全删除原语（ADR-0003）。
//!
//! 全项目**唯一**允许执行删除的模块。校验顺序（全部通过才动手）：
//! 1. 目标根本身不得是链接/联接点（防整目录重定向）
//! 2. `symlink_metadata` 确认为普通文件（链接/目录/特殊文件拒绝）
//! 3. 目标根重新过系统关键路径保护检查（防扫描后目标被替换）
//! 4. 文件 `canonicalize` 后的真实路径必须位于目标根真实路径之下（防链接/`..` 替换）
//! 5. 文件真实路径不得命中保护路径
//! 6. 回收站模式：预检该卷是否真的支持回收（网络位置/回收站禁用/超出配额 → 拒绝，
//!    避免 Shell 静默永久删除被误报为"已移入回收站"，2026-11-04 审查修复）
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

    // 6) 回收站可行性预检（dry-run 也会执行，让用户提前看到不可回收的文件）
    if mode == DeleteMode::Trash {
        trash_preflight(&canon_file, size).map_err(DeleteError::Rejected)?;
    }

    // 7) dry-run 到此为止
    if dry_run {
        return Ok(size);
    }

    // 8) 删除前最后复检：原路径的真实路径必须仍是 canon_file，随后按真实路径删除
    let again = fs::canonicalize(file)
        .map_err(|e| DeleteError::Io(format!("删除前无法解析文件路径：{e}")))?;
    if again != canon_file {
        return Err(DeleteError::Rejected("文件路径在删除前发生变化".into()));
    }
    match mode {
        // 使用已解析的真实路径执行删除，缩小路径替换的窗口
        DeleteMode::Trash => trash::delete(&canon_file)
            .map_err(|e| DeleteError::Io(format!("移入回收站失败：{e}")))?,
        DeleteMode::Permanent => fs::remove_file(&canon_file)
            .map_err(|e| DeleteError::Io(format!("永久删除失败：{e}")))?,
    }
    Ok(size)
}

// ---------------------------------------------------------------------------
// 回收站可行性预检（Windows）
// ---------------------------------------------------------------------------

/// 检查该文件是否真的能被放入回收站。
///
/// Shell 的回收站操作在无法回收时（网络位置、回收站禁用、超出配额）会**静默永久删除**
/// 并返回成功——若不预检，删除报告会把不可恢复的删除宣称为"已移入回收站"。
///
/// 已知限制：按卷容量 5%（Windows 10/11 默认上限）估算配额，用户自定义的更小配额
/// 无法在不读取 BitBucket 卷级注册表的情况下得知（见 ADR-0003）。
#[cfg(windows)]
pub fn trash_preflight(path: &Path, size: u64) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };

    fn to_wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// 取路径所属卷根（`C:\` 或 `\\server\share\`）——`GetDriveTypeW` 需要根路径。
    fn volume_root(path: &Path) -> std::path::PathBuf {
        use std::path::Component;
        let mut comps = path.components();
        if let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
            (comps.next(), comps.next())
        {
            let mut root = std::path::PathBuf::from(prefix.as_os_str());
            root.push("\\");
            return root;
        }
        path.parent().unwrap_or(path).to_path_buf()
    }

    let root = volume_root(path);
    let wide_root = to_wide(root.as_os_str());

    let drive_type = unsafe { GetDriveTypeW(wide_root.as_ptr()) };
    if drive_type == DRIVE_REMOTE || drive_type == DRIVE_CDROM {
        return Err("该位置是网络位置或光驱，无法使用回收站；如确认可丢弃请选择永久删除".into());
    }
    if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE && drive_type != DRIVE_RAMDISK {
        return Err("无法确认该位置支持回收站；如确认可丢弃请选择永久删除".into());
    }

    // 回收站被用户/策略禁用时，Shell 会直接永久删除
    let subkey = to_wide(std::ffi::OsStr::new(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket",
    ));
    let value = to_wide(std::ffi::OsStr::new("NoRecycleFiles"));
    let mut disabled: u32 = 0;
    let mut size_bytes = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut disabled as *mut u32).cast(),
            &mut size_bytes,
        )
    };
    if rc == 0 && disabled == 1 {
        return Err("系统回收站已被禁用，无法安全回收；如确认可丢弃请选择永久删除".into());
    }

    // 配额估算：Windows 10/11 默认回收站上限约为卷容量的 5%
    let mut free_available: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide_root.as_ptr(),
            &mut free_available,
            &mut total,
            &mut total_free,
        )
    };
    if ok != 0 && total > 0 && size > total / 20 {
        return Err(
            "文件大小超过该卷回收站容量估算上限（约 5%），无法放入回收站；如确认可丢弃请选择永久删除"
                .into(),
        );
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn trash_preflight(_path: &Path, _size: u64) -> Result<(), String> {
    Ok(())
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
}
