//! purimaster-platform：Windows 平台特化逻辑。
//!
//! 当前只有**系统关键路径保护**：硬编码在本 crate（不随 `rules/*.toml` 数据变化），
//! 即使规则文件被篡改或社区提交了恶意规则，受保护路径也不会成为清理目标
//! （见 docs/decisions/0002-rules-as-data.md）。
//!
//! 保护语义分两级：
//! - **sensitive**：目标等于、位于其内部、或是其祖先，一律拒绝
//!   （系统二进制目录、个人文档目录等，任何情况下都不允许清理）
//! - **ancestor_only**：目标等于它、或是它的祖先拒绝（不允许整体清除），
//!   但其内部的具体子目录可以作为清理目标（如 `%SystemRoot%\Temp`）
//!
//! ## 路径解析（2026-11-04 审查后加固）
//!
//! 校验前先用 `fs::canonicalize` 把目标解析为真实路径，因此以下绕过手法都会被
//! 归一化后拦截：`..` 路径穿越、`\\?\` / `\\?\UNC\` 前缀、8.3 短名、
//! 符号链接/目录联接（junction）。
//!
//! - 目标不存在（`NotFound`）：返回 `Ok`——不存在的路径没有任何可扫描/删除的内容；
//!   **删除动作落地前必须在删除时重新校验**（文件可能在扫描与删除之间被创建）。
//! - 其它解析失败（如权限不足）：保守拒绝。

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

pub mod delete;

pub use delete::{DeleteError, DeleteMode, delete_file};

/// 保护路径配置。
#[derive(Debug, Clone, Default)]
pub struct Protection {
    pub sensitive: Vec<PathBuf>,
    pub ancestor_only: Vec<PathBuf>,
}

/// 读取当前系统的保护路径（环境变量可能在测试/受限环境中缺失，缺失项自动跳过）。
pub fn protection() -> Protection {
    Protection {
        sensitive: sensitive_roots(),
        ancestor_only: ancestor_only_roots(),
    }
}

fn sensitive_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Some(windir) = env_path("SystemRoot").or_else(|| env_path("WINDIR")) {
        for sub in ["System32", "SysWOW64", "WinSxS", "boot"] {
            roots.push(windir.join(sub));
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(p) = env_path(var) {
            roots.push(p);
        }
    }

    // 个人文件目录：绝不作为清理目标（宁可少清不可误删）。
    // 优先使用 Known Folder API（dirs），覆盖 OneDrive 文件夹迁移/重定向的情况；
    // 若 Known Folder 不可用，退回 %USERPROFILE%\固定名 字面路径。
    for folder in [
        dirs::document_dir(),
        dirs::download_dir(),
        dirs::desktop_dir(),
        dirs::picture_dir(),
        dirs::video_dir(),
        dirs::audio_dir(),
    ]
    .into_iter()
    .flatten()
    {
        push_unique(&mut roots, folder);
    }
    if let Some(user) = env_path("USERPROFILE") {
        for sub in [
            "Documents",
            "Desktop",
            "Pictures",
            "Videos",
            "Music",
            "Downloads",
            "Favorites",
        ] {
            push_unique(&mut roots, user.join(sub));
        }
    }

    // OneDrive 同步目录（删除会同步到云端，必须保护）
    for var in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Some(p) = env_path(var) {
            push_unique(&mut roots, p);
        }
    }
    if let Some(user) = env_path("USERPROFILE") {
        push_unique(&mut roots, user.join("OneDrive"));
    }

    roots
}

fn ancestor_only_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    // 整个系统目录/程序数据目录/用户目录/应用数据目录不允许被整体清除，
    // 但其内部的具体缓存目录（Temp、SoftwareDistribution\Download、浏览器缓存等）可以作为目标
    for var in [
        "SystemRoot",
        "WINDIR",
        "ProgramData",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
    ] {
        if let Some(p) = env_path(var) {
            push_unique(&mut roots, p);
        }
    }
    roots
}

fn push_unique(roots: &mut Vec<PathBuf>, path: PathBuf) {
    let key = normalize(&path);
    if !roots.iter().any(|p| normalize(p) == key) {
        roots.push(path);
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 用当前系统的保护路径校验清理目标是否允许。
pub fn check_target(target: &Path) -> Result<(), String> {
    check_target_against(target, &protection())
}

/// 用指定保护配置校验清理目标是否允许（便于测试）。
pub fn check_target_against(target: &Path, protection: &Protection) -> Result<(), String> {
    if !target.is_absolute() {
        return Err(format!("清理目标必须是绝对路径：{}", target.display()));
    }
    if is_drive_root(target) {
        return Err("不允许以盘符根目录作为清理目标".to_string());
    }

    // 解析为真实路径：`..` / 8.3 短名 / 符号链接 / 联接点 / `\\?\` 前缀全部归一化
    let t = match fs::canonicalize(target) {
        Ok(real) => normalize(&real),
        // 目标不存在：没有任何可扫描/删除的内容，不构成风险。
        // 注意：删除动作必须在使用时重新校验。
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        // 其它错误（权限不足等）：保守拒绝
        Err(e) => return Err(format!("无法解析清理目标（保守拒绝）：{e}")),
    };

    if is_drive_root(Path::new(&t)) {
        return Err("不允许以盘符根目录作为清理目标".to_string());
    }

    for s in &protection.sensitive {
        let s = resolve_key(s);
        if related(&t, &s) {
            return Err(format!("目标命中受保护路径：{s}"));
        }
    }
    for a in &protection.ancestor_only {
        let a = resolve_key(a);
        // 目标等于它、或目标是它的祖先 → 拒绝整体清除；其内部子目录允许
        if t == a || is_strict_prefix(&t, &a) {
            return Err(format!("不允许整体清除受保护目录：{a}"));
        }
    }
    Ok(())
}

/// 保护路径的比较键：尽量解析真实路径（保护目录通常存在）。
fn resolve_key(path: &Path) -> String {
    match fs::canonicalize(path) {
        Ok(real) => normalize(&real),
        Err(_) => normalize(path),
    }
}

/// 是否为盘符根（`C:\`）或 UNC 根（`\\server\share`）。
fn is_drive_root(path: &Path) -> bool {
    let mut components = path.components();
    let has_prefix = matches!(components.next(), Some(Component::Prefix(_)));
    let has_root = matches!(components.next(), Some(Component::RootDir));
    has_prefix && has_root && components.next().is_none()
}

/// 规范化：去掉 `\\?\` / `\\?\UNC\` 前缀、小写、正斜杠转反斜杠、去掉末尾分隔符。
fn normalize(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let stripped = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.into_owned()
    };
    let mut s = stripped.replace('/', "\\").to_ascii_lowercase();
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// `child` 是否是 `parent` 的严格子路径（按路径段比较，避免 `c:\win` 匹配 `c:\windows`）。
fn is_strict_prefix(parent: &str, child: &str) -> bool {
    child.len() > parent.len()
        && child.starts_with(parent)
        && child.as_bytes().get(parent.len()) == Some(&b'\\')
}

fn related(a: &str, b: &str) -> bool {
    a == b || is_strict_prefix(a, b) || is_strict_prefix(b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// 构造真实存在的测试目录：
    /// `<tmp>/sensitive/`、`<tmp>/sensitive/inner/`、`<tmp>/rootdir/sub/`、`<tmp>/other/`
    fn setup() -> (TempDir, Protection) {
        let dir = tempfile::tempdir().unwrap();
        let sensitive = dir.path().join("sensitive");
        fs::create_dir_all(sensitive.join("inner")).unwrap();
        let ancestor = dir.path().join("rootdir");
        fs::create_dir_all(ancestor.join("sub")).unwrap();
        fs::create_dir_all(dir.path().join("other")).unwrap();
        let protection = Protection {
            sensitive: vec![sensitive],
            ancestor_only: vec![ancestor],
        };
        (dir, protection)
    }

    #[test]
    fn rejects_inside_sensitive() {
        let (dir, p) = setup();
        let target = dir.path().join("sensitive").join("inner");
        assert!(check_target_against(&target, &p).is_err());
        assert!(check_target_against(&dir.path().join("sensitive"), &p).is_err());
    }

    #[test]
    fn rejects_ancestor_of_sensitive() {
        let (dir, p) = setup();
        // 临时目录根是 sensitive 的祖先
        assert!(check_target_against(dir.path(), &p).is_err());
    }

    #[test]
    fn dotdot_traversal_is_resolved_and_rejected() {
        // 回归测试（2026-11-04 审查）：`other/../sensitive` 必须被解析后拒绝
        let (dir, p) = setup();
        let sneaky = dir.path().join("other").join("..").join("sensitive");
        assert!(check_target_against(&sneaky, &p).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_is_stripped_and_rejected() {
        let (dir, p) = setup();
        let verbatim = PathBuf::from(format!(r"\\?\{}", dir.path().join("sensitive").display()));
        assert!(check_target_against(&verbatim, &p).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn case_insensitive_comparison() {
        let (dir, p) = setup();
        let upper = PathBuf::from(
            dir.path()
                .join("sensitive")
                .to_string_lossy()
                .to_uppercase(),
        );
        assert!(check_target_against(&upper, &p).is_err());
    }

    #[test]
    fn allows_descendant_of_ancestor_only() {
        let (dir, p) = setup();
        assert!(check_target_against(&dir.path().join("rootdir").join("sub"), &p).is_ok());
    }

    #[test]
    fn rejects_purging_ancestor_only_root() {
        let (dir, p) = setup();
        assert!(check_target_against(&dir.path().join("rootdir"), &p).is_err());
    }

    #[test]
    fn nonexistent_target_is_allowed() {
        // 不存在的目标没有可扫描/删除的内容；删除前会重新校验
        let (dir, p) = setup();
        assert!(check_target_against(&dir.path().join("not-installed"), &p).is_ok());
    }

    #[test]
    fn rejects_drive_root() {
        assert!(check_target_against(Path::new(r"C:\"), &Protection::default()).is_err());
        assert!(check_target_against(Path::new(r"D:\"), &Protection::default()).is_err());
    }

    #[test]
    fn rejects_relative() {
        assert!(check_target_against(Path::new(r"temp"), &Protection::default()).is_err());
    }

    #[test]
    fn prefix_check_is_segment_based() {
        let dir = tempfile::tempdir().unwrap();
        let win = dir.path().join("win");
        let win_old = dir.path().join("winOld");
        fs::create_dir_all(&win).unwrap();
        fs::create_dir_all(&win_old).unwrap();
        let p = Protection {
            sensitive: vec![win],
            ancestor_only: vec![],
        };
        // 不能因为字符串前缀而误判 winOld
        assert!(check_target_against(&win_old, &p).is_ok());
    }

    /// 回归测试（2026-11-04 审查发现）：真实系统路径的 `..` / `\\?\` 绕过必须被拦截。
    /// 依赖真实 Windows 环境变量；缺失时跳过。
    #[cfg(windows)]
    #[test]
    fn real_system32_bypass_attempts_rejected() {
        let Some(sysroot) = std::env::var_os("SystemRoot") else {
            return;
        };
        let sysroot = PathBuf::from(sysroot);
        let system32 = sysroot.join("System32");
        if !system32.is_dir() {
            return;
        }
        let p = protection();

        // 直接命中
        assert!(check_target_against(&system32, &p).is_err());
        // `..` 穿越（审查时实测可绕过旧实现）：<SystemRoot>\..\<SystemRoot>\System32
        let sneaky = sysroot
            .join("..")
            .join(sysroot.file_name().unwrap_or_default())
            .join("System32");
        assert!(check_target_against(&sneaky, &p).is_err());
        // `\\?\` 前缀
        let verbatim = PathBuf::from(format!(r"\\?\{}", system32.display()));
        assert!(check_target_against(&verbatim, &p).is_err());
    }
}
