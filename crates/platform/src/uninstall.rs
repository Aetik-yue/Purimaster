//! 已安装程序枚举、官方卸载器执行与残留扫描（软件卸载，ADR-0004）。
//!
//! 安全边界（ADR-0004）：
//! - 只执行程序自己声明的 `UninstallString`，**绝不**使用 QuietUninstallString
//! - 注册表残留候选只做名称精确匹配（`Software\<名称>`），删除走
//!   [`crate::registry::delete_registry_tree`] 的守卫清单
//! - 文件残留候选受保护检查过滤，删除走 [`crate::delete::remove_dir_tree_standalone`]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 一个已安装程序（来自注册表卸载键）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledProgram {
    pub display_name: String,
    pub display_version: String,
    pub publisher: String,
    /// 安装目录（卸载键可能未提供，为空串）
    pub install_location: String,
    pub uninstall_string: String,
    /// EstimatedSize（KB），0 = 未提供
    pub estimated_size_kb: u64,
    /// 卸载键里的 InstallDate 原样字符串（常为 YYYYMMDD）
    pub install_date: String,
    /// 卸载注册表键标识（"HKLM\Software\..."），残留清理时使用
    pub registry_key: String,
}

/// 一个文件残留目录（含内容总大小）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidueDir {
    pub path: String,
    pub size: u64,
}

/// 一个程序的残留扫描结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidueReport {
    pub program_name: String,
    pub files: Vec<ResidueDir>,
    pub registry: Vec<String>,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::registry::{enum_subkeys, get_dword, get_string, key_exists};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT, WIN32_ERROR};
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    /// 卸载列表的三个根：(键标识前缀, HKEY, 子键路径)
    const UNINSTALL_ROOTS: [(&str, HKEY, &str); 3] = [
        (
            "HKLM",
            HKEY_LOCAL_MACHINE,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            "HKLM",
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            "HKCU",
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ];

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// 枚举全部已安装程序（跳过 SystemComponent=1 的系统组件条目）。
    pub fn list_installed_programs() -> Vec<InstalledProgram> {
        let mut out: Vec<InstalledProgram> = Vec::new();
        for (root_label, hroot, sub) in UNINSTALL_ROOTS {
            for name in enum_subkeys(hroot, sub) {
                let Some(prog) = read_entry(root_label, hroot, sub, &name) else {
                    continue;
                };
                out.push(prog);
            }
        }
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.registry_key.cmp(&b.registry_key))
        });
        out
    }

    fn read_entry(
        root_label: &str,
        hroot: HKEY,
        sub: &str,
        name: &str,
    ) -> Option<InstalledProgram> {
        let key_path = format!(r"{root_label}\{sub}\{name}");
        // get_string/get_dword 按 (root, 子键路径, 值名) 工作——子键要拼上条目名
        let full = format!(r"{sub}\{name}");
        let display_name = get_string(hroot, &full, "DisplayName")?;
        if display_name.trim().is_empty() {
            return None;
        }
        if get_dword(hroot, &full, "SystemComponent").unwrap_or(0) == 1 {
            return None; // Windows 自身隐藏的系统组件条目
        }
        Some(InstalledProgram {
            display_name: display_name.trim().to_string(),
            display_version: get_string(hroot, &full, "DisplayVersion").unwrap_or_default(),
            publisher: get_string(hroot, &full, "Publisher").unwrap_or_default(),
            install_location: get_string(hroot, &full, "InstallLocation").unwrap_or_default(),
            uninstall_string: get_string(hroot, &full, "UninstallString").unwrap_or_default(),
            estimated_size_kb: get_dword(hroot, &full, "EstimatedSize").unwrap_or(0) as u64,
            install_date: get_string(hroot, &full, "InstallDate").unwrap_or_default(),
            registry_key: key_path,
        })
    }

    /// 拆分卸载命令：带引号的 exe + 参数，或按 ".exe" 切分。
    fn split_command(s: &str) -> (String, String) {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix('"')
            && let Some(i) = rest.find('"')
        {
            return (rest[..i].to_string(), rest[i + 1..].trim().to_string());
        }
        if let Some(idx) = t.to_ascii_lowercase().find(".exe") {
            let i = idx + 4;
            return (t[..i].to_string(), t[i..].trim().to_string());
        }
        (t.to_string(), String::new())
    }

    /// 执行官方卸载器并等待（最长 10 分钟）。
    pub fn run_uninstaller(uninstall_string: &str) -> Result<String, String> {
        let (exe, args) = split_command(uninstall_string);
        if exe.is_empty() {
            return Err(format!("卸载命令为空：{uninstall_string}"));
        }
        let exe_w = to_wide(&exe);
        let args_w = to_wide(&args);
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpFile = exe_w.as_ptr();
        info.lpParameters = if args.is_empty() {
            std::ptr::null()
        } else {
            args_w.as_ptr()
        };
        info.nShow = SW_SHOWNORMAL;

        let ok = unsafe { ShellExecuteExW(&mut info) };
        if ok == 0 {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(format!(
                "无法启动卸载器（错误码 {code}）：{exe}。可能需要管理员权限或文件已不存在"
            ));
        }
        if info.hProcess.is_null() {
            return Ok("卸载器已启动（无法等待其结束），完成后请刷新列表".into());
        }
        // 最长等 10 分钟：部分卸载器派生副本后立即退出，等待会超时——如实报告
        let wait: WIN32_ERROR = unsafe { WaitForSingleObject(info.hProcess, 600_000) };
        if wait == WAIT_TIMEOUT {
            unsafe { CloseHandle(info.hProcess) };
            return Ok("等待超时（10 分钟）：卸载器可能仍在运行，请完成后刷新列表".into());
        }
        let mut code: u32 = 0;
        unsafe {
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        Ok(match code {
            0 => "卸载器正常退出".into(),
            3010 => "卸载成功，需要重启完成".into(),
            1602 => "卸载被用户取消".into(),
            c => format!("卸载器退出码 {c}（可能未完成或被用户取消）"),
        })
    }

    /// 残留候选的名称 token：显示名、发布者、安装目录末段。
    fn name_tokens(prog: &InstalledProgram) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |s: &str| {
            let t = s.trim();
            if t.len() >= 3 && !out.iter().any(|e: &String| e.eq_ignore_ascii_case(t)) {
                out.push(t.to_string());
            }
        };
        push(&prog.display_name);
        push(&prog.publisher);
        if !prog.install_location.is_empty()
            && let Some(last) = Path::new(prog.install_location.trim_end_matches('\\')).file_name()
        {
            push(&last.to_string_lossy());
        }
        out
    }

    fn dir_exists(p: &Path) -> bool {
        p.is_dir()
    }

    fn dir_size(dir: &Path) -> u64 {
        // 简易递归求和（残留目录规模有限；栈深度用迭代栈避免）
        let mut total = 0u64;
        let mut stack = vec![dir.to_path_buf()];
        let mut visited = 0usize;
        while let Some(d) = stack.pop() {
            visited += 1;
            if visited > 100_000 {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(ft) = entry.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(entry.path());
                } else if ft.is_file()
                    && let Ok(m) = entry.metadata()
                {
                    total += m.len();
                }
            }
        }
        total
    }

    /// 扫描一个已卸载程序的残留（文件目录 + 注册表键，名称精确匹配）。
    pub fn scan_residue(prog: &InstalledProgram) -> ResidueReport {
        let mut files: Vec<ResidueDir> = Vec::new();
        let mut registry: Vec<String> = Vec::new();

        // 已知的确定残留：程序自己的卸载键（官方卸载器没删干净时）
        if key_exists(&prog.registry_key) {
            registry.push(prog.registry_key.clone());
        }

        for tok in name_tokens(prog) {
            // 文件残留：安装目录 + 常见数据目录同名匹配
            let mut candidates: Vec<PathBuf> = Vec::new();
            if !prog.install_location.is_empty() {
                candidates.push(PathBuf::from(prog.install_location.trim()));
            }
            for base_var in ["APPDATA", "LOCALAPPDATA", "ProgramData"] {
                if let Ok(base) = std::env::var(base_var)
                    && !base.is_empty()
                {
                    candidates.push(Path::new(&base).join(&tok));
                }
            }
            for c in candidates {
                if !dir_exists(&c) || files.iter().any(|f| f.path == c.to_string_lossy()) {
                    continue;
                }
                // 保护检查：受保护路径（系统/个人目录）绝不进候选
                if crate::check_target(&c).is_err() {
                    continue;
                }
                files.push(ResidueDir {
                    path: c.to_string_lossy().into_owned(),
                    size: dir_size(&c),
                });
            }

            // 注册表残留：Software\<名称>（精确匹配，守卫删除前还会再校验）
            for root_label in ["HKLM", "HKCU"] {
                let key = format!(r"{root_label}\Software\{tok}");
                if key_exists(&key) && !registry.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
                    registry.push(key);
                }
            }
        }

        files.sort_by_key(|f| std::cmp::Reverse(f.size));
        ResidueReport {
            program_name: prog.display_name.clone(),
            files,
            registry,
        }
    }
}

#[cfg(windows)]
pub use imp::{list_installed_programs, run_uninstaller, scan_residue};

#[cfg(not(windows))]
pub fn list_installed_programs() -> Vec<InstalledProgram> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn run_uninstaller(_uninstall_string: &str) -> Result<String, String> {
    Err("仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn scan_residue(_prog: &InstalledProgram) -> ResidueReport {
    ResidueReport {
        program_name: _prog.display_name.clone(),
        files: Vec::new(),
        registry: Vec::new(),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn lists_some_programs_with_required_fields() {
        let progs = list_installed_programs();
        if progs.is_empty() {
            eprintln!("跳过：本机未枚举到任何程序");
            return;
        }
        assert!(
            progs.len() > 5,
            "正常 Windows 应有多个程序，实际 {}",
            progs.len()
        );
        // 个别条目可能没有 UninstallString（注册表如实反映），但绝大多数应有
        let with_uninstaller = progs
            .iter()
            .filter(|p| !p.uninstall_string.trim().is_empty())
            .count();
        assert!(
            with_uninstaller * 2 > progs.len(),
            "应有过半条目带卸载命令：{with_uninstaller}/{}",
            progs.len()
        );
        for p in progs.iter().take(80) {
            assert!(!p.display_name.trim().is_empty());
            assert!(p.registry_key.starts_with("HKLM\\") || p.registry_key.starts_with("HKCU\\"));
        }
    }

    #[test]
    fn residue_scan_finds_designated_dir() {
        // 自建"已卸载软件"的数据目录：install_location 指向它，扫描必须命中
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("data").join("x.bin"), [0u8; 64]).unwrap();

        let prog = InstalledProgram {
            display_name: "__purimaster_residue_fixture__".into(),
            display_version: String::new(),
            publisher: String::new(),
            install_location: dir.path().join("data").to_string_lossy().into_owned(),
            uninstall_string: String::new(),
            estimated_size_kb: 0,
            install_date: String::new(),
            registry_key: r"HKCU\Software\__purimaster_residue_fixture__".into(),
        };
        let report = scan_residue(&prog);
        assert!(
            report
                .files
                .iter()
                .any(|f| f.path == dir.path().join("data").to_string_lossy()),
            "安装目录应进入候选：{:?}",
            report.files
        );
        assert!(
            report.files.iter().all(|f| f.size >= 64),
            "目录大小应被统计"
        );
    }
}
