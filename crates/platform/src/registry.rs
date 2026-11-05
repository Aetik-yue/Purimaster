//! 注册表读取与守卫式键树删除（软件卸载功能，ADR-0004）。
//!
//! 删除入口 [`delete_registry_tree`] 带硬编码拒绝清单：只放行 `Software\` 之下、
//! 非 Classes 子树、非 `Microsoft` 子树（程序自己的卸载键除外）的键。这条守卫
//! 不受任何数据/参数影响——即使调用方传错，系统关键区域也删不掉。

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegDeleteTreeW,
    RegEnumKeyW, RegOpenKeyExW, RegQueryValueExW,
};

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn open_key(root: HKEY, sub: &str) -> Result<HKEY, u32> {
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(root, to_wide(sub).as_ptr(), 0, KEY_READ, &mut h) };
    if rc == ERROR_SUCCESS { Ok(h) } else { Err(rc) }
}

fn close_key(h: HKEY) {
    unsafe { RegCloseKey(h) };
}

/// 枚举直接子键名。
#[cfg(windows)]
pub(crate) fn enum_subkeys(root: HKEY, sub: &str) -> Vec<String> {
    let Ok(h) = open_key(root, sub) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index: u32 = 0;
    loop {
        let mut buf = [0u16; 260];
        let rc = unsafe { RegEnumKeyW(h, index, buf.as_mut_ptr(), buf.len() as u32) };
        if rc == ERROR_NO_MORE_ITEMS {
            break;
        }
        if rc == ERROR_SUCCESS {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            out.push(String::from_utf16_lossy(&buf[..len]));
            index += 1;
        } else {
            break;
        }
    }
    close_key(h);
    out
}

/// 读 REG_SZ 值。
#[cfg(windows)]
pub(crate) fn get_string(root: HKEY, sub: &str, name: &str) -> Option<String> {
    let h = open_key(root, sub).ok()?;
    // 先查类型与大小，再读数据
    let mut ty: u32 = 0;
    let mut size: u32 = 0;
    let rc = unsafe {
        RegQueryValueExW(
            h,
            to_wide(name).as_ptr(),
            std::ptr::null(),
            &mut ty,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS || ty != 1 /* REG_SZ */ || size == 0 || !size.is_multiple_of(2) {
        close_key(h);
        return None;
    }
    let mut buf = vec![0u16; (size / 2) as usize];
    let rc = unsafe {
        RegQueryValueExW(
            h,
            to_wide(name).as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    close_key(h);
    if rc != ERROR_SUCCESS {
        return None;
    }
    // 去掉结尾 NUL
    while buf.last() == Some(&0) {
        buf.pop();
    }
    Some(String::from_utf16_lossy(&buf))
}

/// 读 REG_DWORD 值。
#[cfg(windows)]
pub(crate) fn get_dword(root: HKEY, sub: &str, name: &str) -> Option<u32> {
    let h = open_key(root, sub).ok()?;
    let mut ty: u32 = 0;
    let mut size: u32 = 4;
    let mut out: u32 = 0;
    let rc = unsafe {
        RegQueryValueExW(
            h,
            to_wide(name).as_ptr(),
            std::ptr::null(),
            &mut ty,
            (&mut out as *mut u32).cast(),
            &mut size,
        )
    };
    close_key(h);
    if rc == ERROR_SUCCESS && ty == 4 /* REG_DWORD */ && size == 4 {
        Some(out)
    } else {
        None
    }
}

/// 解析 "HKLM\Software\..." / "HKCU\Software\..." 形式的键标识。
fn parse_key_id(key_id: &str) -> Result<(HKEY, String), String> {
    let (root, rest) = key_id
        .split_once('\\')
        .ok_or_else(|| format!("非法的注册表键标识：{key_id}"))?;
    let root = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        other => {
            return Err(format!(
                "拒绝删除：不支持的注册表根「{other}」（仅允许 HKLM/HKCU）"
            ));
        }
    };
    Ok((root, rest.to_string()))
}

/// 守卫式删除整个注册表键树（ADR-0004 拒绝清单，硬编码不受调用方影响）：
/// - 仅接受 `HKLM\Software\…` / `HKCU\Software\…`
/// - `Software` 本身（无下级）拒绝
/// - `Software\Classes\` 子树（文件关联）拒绝
/// - `Software\Microsoft\` 子树拒绝，**例外**：程序自己的卸载键
///   `…\Microsoft\Windows\CurrentVersion\Uninstall\<键>`
pub fn delete_registry_tree(key_id: &str) -> Result<(), String> {
    let (root, sub) = parse_key_id(key_id)?;
    let lower = sub.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("software\\") else {
        return Err(format!("拒绝删除：{key_id} 不在 Software 子树下"));
    };
    if rest.is_empty() {
        return Err(format!("拒绝删除：{key_id} 不能是 Software 本身"));
    }
    if rest == "classes" || rest.starts_with("classes\\") {
        return Err(format!("拒绝删除：{key_id} 位于 Classes 子树（文件关联）"));
    }
    if rest == "microsoft" || rest.starts_with("microsoft\\") {
        const UNINSTALL: &str = r"microsoft\windows\currentversion\uninstall\";
        let is_uninstall_key = rest.starts_with(UNINSTALL) && rest.len() > UNINSTALL.len();
        if !is_uninstall_key {
            return Err(format!(
                "拒绝删除：{key_id} 位于受保护的 Microsoft 子树（仅允许程序自己的卸载键）"
            ));
        }
    }
    // 用原始大小写路径执行删除（parse 用的是小写副本）
    let (_, sub_raw) = parse_key_id_raw(key_id)?;
    let rc = unsafe { RegDeleteTreeW(root, to_wide(&sub_raw).as_ptr()) };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("删除注册表键失败（错误码 {rc}）：{key_id}"))
    }
}

fn parse_key_id_raw(key_id: &str) -> Result<(HKEY, String), String> {
    let (root, rest) = key_id
        .split_once('\\')
        .ok_or_else(|| format!("非法的注册表键标识：{key_id}"))?;
    let root = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        other => return Err(format!("不支持的注册表根：{other}")),
    };
    Ok((root, rest.to_string()))
}

/// 键是否存在（读打开即视为存在）。
pub fn key_exists(key_id: &str) -> bool {
    match parse_key_id(key_id) {
        Ok((root, sub)) => match open_key(root, &sub) {
            Ok(h) => {
                close_key(h);
                true
            }
            Err(_) => false,
        },
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_rejects_dangerous_keys() {
        assert!(delete_registry_tree("HKLM\\Software").is_err());
        assert!(delete_registry_tree("HKCU\\Software").is_err());
        assert!(delete_registry_tree("HKLM\\Software\\Classes\\.txt").is_err());
        assert!(delete_registry_tree("HKLM\\Software\\Classes").is_err());
        assert!(delete_registry_tree("HKLM\\Software\\Microsoft\\Windows").is_err());
        assert!(delete_registry_tree("HKCU\\Software\\Microsoft").is_err());
        assert!(delete_registry_tree("HKLM").is_err());
        assert!(delete_registry_tree("HKEY_CLASSES_ROOT\\.txt").is_err());
        assert!(delete_registry_tree("Software\\Foo").is_err());
        assert!(delete_registry_tree("").is_err());
    }

    #[test]
    fn guard_allows_shape_of_legitimate_keys() {
        // 不实际执行删除——只验证守卫解析对合法形状放行到删除一步
        //（错误码非"拒绝"文案即可）
        let err = delete_registry_tree("HKLM\\Software\\__purimaster_guard_test__").unwrap_err();
        assert!(err.contains("错误码"), "应通过守卫进入删除阶段：{err}");
        let uninstall_key =
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\__purimaster_guard_test__";
        let err2 = delete_registry_tree(uninstall_key).unwrap_err();
        assert!(err2.contains("错误码"), "卸载键形状应被放行：{err2}");
    }

    #[cfg(windows)]
    #[test]
    fn real_registry_roundtrip() {
        // 真实注册表回环：创建测试键 → 存在 → 守卫删除 → 不存在
        use windows_sys::Win32::System::Registry::RegCreateKeyW;

        const TEST_KEY: &str = r"HKCU\Software\__purimaster_residue_test__";
        let (root, sub) = parse_key_id(TEST_KEY).unwrap();
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegCreateKeyW(root, to_wide(&sub).as_ptr(), &mut h) };
        if rc != ERROR_SUCCESS {
            eprintln!("跳过：无法创建测试键（错误码 {rc}）");
            return;
        }
        close_key(h);
        assert!(key_exists(TEST_KEY));
        delete_registry_tree(TEST_KEY).unwrap();
        assert!(!key_exists(TEST_KEY), "删除后键应不存在");
    }

    #[test]
    fn key_exists_false_for_garbage() {
        assert!(!key_exists("__非法__\\键"));
    }
}
