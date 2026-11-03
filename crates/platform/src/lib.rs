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

use std::path::{Component, Path, PathBuf};

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
    if let Some(user) = env_path("USERPROFILE") {
        // 个人文件目录：绝不作为清理目标（下载目录同样保护，宁可少清不可误删）
        for sub in [
            "Documents",
            "Desktop",
            "Pictures",
            "Videos",
            "Music",
            "Downloads",
            "Favorites",
        ] {
            roots.push(user.join(sub));
        }
    }
    roots
}

fn ancestor_only_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    // 整个系统目录/程序数据目录/用户目录不允许被整体清除，
    // 但其内部的具体缓存目录（Temp、SoftwareDistribution\Download 等）可以作为目标
    for var in ["SystemRoot", "WINDIR", "ProgramData", "USERPROFILE"] {
        if let Some(p) = env_path(var) {
            roots.push(p);
        }
    }
    roots
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

    let t = normalize(target);
    for s in &protection.sensitive {
        let s = normalize(s);
        if related(&t, &s) {
            return Err(format!("目标命中受保护路径：{}", s));
        }
    }
    for a in &protection.ancestor_only {
        let a = normalize(a);
        // 目标等于它、或目标是它的祖先 → 拒绝整体清除；其内部子目录允许
        if t == a || is_strict_prefix(&t, &a) {
            return Err(format!("不允许整体清除受保护目录：{}", a));
        }
    }
    Ok(())
}

/// 是否为盘符根（`C:\`）或 UNC 根（`\\server\share`）。
fn is_drive_root(path: &Path) -> bool {
    let mut components = path.components();
    let has_prefix = matches!(components.next(), Some(Component::Prefix(_)));
    let has_root = matches!(components.next(), Some(Component::RootDir));
    has_prefix && has_root && components.next().is_none()
}

/// 规范化：小写、正斜杠转反斜杠、去掉末尾分隔符（盘符根除外）。
fn normalize(path: &Path) -> String {
    let mut s = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
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

    fn prot() -> Protection {
        Protection {
            sensitive: vec![PathBuf::from(r"C:\Windows\System32")],
            ancestor_only: vec![
                PathBuf::from(r"C:\Windows"),
                PathBuf::from(r"C:\ProgramData"),
            ],
        }
    }

    #[test]
    fn rejects_inside_sensitive() {
        assert!(check_target_against(Path::new(r"C:\Windows\System32\config"), &prot()).is_err());
        assert!(check_target_against(Path::new(r"C:\Windows\System32"), &prot()).is_err());
    }

    #[test]
    fn rejects_ancestor_of_sensitive() {
        // C:\Windows 本身是 ancestor_only；但即使不在列表里，作为 System32 的祖先也应拒绝
        assert!(check_target_against(Path::new(r"C:\"), &prot()).is_err());
    }

    #[test]
    fn rejects_drive_root() {
        assert!(check_target_against(Path::new(r"C:\"), &prot()).is_err());
        assert!(check_target_against(Path::new(r"D:\"), &prot()).is_err());
    }

    #[test]
    fn rejects_relative() {
        assert!(check_target_against(Path::new(r"temp"), &prot()).is_err());
    }

    #[test]
    fn allows_safe_subtree_under_ancestor_only() {
        assert!(check_target_against(Path::new(r"C:\Windows\Temp"), &prot()).is_ok());
        assert!(
            check_target_against(Path::new(r"C:\ProgramData\Microsoft\Windows\WER"), &prot())
                .is_ok()
        );
    }

    #[test]
    fn rejects_purging_ancestor_only_root() {
        assert!(check_target_against(Path::new(r"C:\ProgramData"), &prot()).is_err());
        assert!(check_target_against(Path::new(r"C:\Windows"), &prot()).is_err());
    }

    #[test]
    fn case_insensitive_and_separator_tolerant() {
        let p = Protection {
            sensitive: vec![PathBuf::from(r"C:\Windows\System32")],
            ancestor_only: vec![],
        };
        assert!(check_target_against(Path::new(r"c:/windows/SYSTEM32/drivers"), &p).is_err());
    }

    #[test]
    fn prefix_check_is_segment_based() {
        let p = Protection {
            sensitive: vec![PathBuf::from(r"C:\Windows")],
            ancestor_only: vec![],
        };
        // 不能因为字符串前缀而误判 C:\WindowsOld
        assert!(check_target_against(Path::new(r"C:\WindowsOld\Temp"), &p).is_ok());
    }
}
