# 开发日志 2026-11-06（四）：软件卸载——官方卸载器 + 守卫式注册表残留清除

## 今日目标

- 新增"删除特定软件"功能：列表多选 → 官方卸载 → 残留（文件 + 注册表）逐项确认清除

## 设计（ADR-0004，先立边界再动手）

- **官方卸载器优先**：只执行程序自己的 `UninstallString`（ShellExecuteEx 启动 + 等待，
  每个最长 10 分钟），**绝不使用 QuietUninstallString**——用户始终面对第三方卸载器自己的
  确认 UI/UAC
- **注册表删除守卫**（`registry.rs::delete_registry_tree`，硬编码拒绝清单）：
  仅 `HKLM/HKCU` + `Software\` 下至少一级；`Classes` 子树拒绝；`Microsoft` 子树拒绝
  （唯一例外：`…\CurrentVersion\Uninstall\<键>` 程序自己的卸载键）。守卫不受调用方影响
- **残留候选 = 名称精确匹配**：安装目录、`%APPDATA%/%LOCALAPPDATA%/%ProgramData%\<名称>`、
  `HKLM/HKCU\Software\<名称>` + 卸载键自身；全部过保护检查（`check_target`）后才进候选，
  **逐项勾选后才删**——与"注册表清理/优化"（需求 §7 Non-Goal）的界限写进 ADR 与需求 §3.5
- **新删除原语 `remove_dir_tree_standalone`**：目录树永久删除（无回收站）——拒绝链接/
  盘符根/受保护路径，`remove_dir_all`（不跟随符号链接），带 10 万项规模上限
- **UWP/Store 应用不支持**（枚举/卸载机制完全不同），记为已知限制

## 实现

- platform 新增 `registry.rs`（RegEnumKeyW 枚举/REG_SZ/REG_DWORD 读取/守卫式
  RegDeleteTreeW）与 `uninstall.rs`（三根卸载键枚举 + 跳过 SystemComponent、
  卸载命令拆分（引号/.exe 两种形态）、ShellExecuteExW 等待（退出码 0/3010/1602
  分别解读）、残留扫描）
- 壳层 4 命令：`list_programs` / `uninstall_programs`（`uninstall://progress|complete`
  事件，单飞复用 clean_active）/ `scan_residue`（async spawn_blocking）/
  `delete_residue`（复用 `CleanReport` 与 `clean://complete`，报告由共享 ReportModal 展示）
- 前端第三个页签「软件卸载」（KeepAlive 三分支）：筛选、多选、确认弹窗（讲清"逐个启动
  官方卸载器/可能请求管理员权限/完成后自动扫残留"）、逐软件进度、残留面板（文件带大小、
  注册表键带前缀，默认全勾、逐项可取消）、清除后刷新列表

## 遇到的问题与解决

1. windows-sys 0.59 的 `RegEnumKeyExW` 有 8 参数（含 FILETIME）——改用更简单的
   `RegEnumKeyW`；`RegCreateKeyExW` 因 SECURITY_ATTRIBUTES 被 `Win32_Security`
   feature 门控而 E0432——测试改用 `RegCreateKeyW`
2. 断言过严：真机存在无 `UninstallString` 的条目（注册表如实反映）——放宽为
   "过半条目带卸载命令"
3. clippy 三连：collapsible_if ×2（let-chains 化）、sort_by → sort_by_key + Reverse、
   `size.is_multiple_of(2)`
4. 测试夹具 `create_dir` 只建一层 → `create_dir_all`

## 门禁结果

`cargo fmt --check` ✓ ｜ `cargo clippy -D warnings` ✓ ｜ `cargo test --workspace`
**86 通过**（platform 34：新增注册表守卫 2 + 真实注册表回环 1 + 目录树删除 4 +
卸载列表/残留扫描 sanity 2）✓ ｜ `pnpm lint` ✓ ｜ `pnpm build` ✓

真机 sanity：枚举出本机全部程序（个别无卸载命令的条目按设计跳过删除入口）；
残留扫描命中自建 fixture 目录并统计大小。

## 待办

- [ ] 用户真机冒烟：挑一个不重要的软件走完整卸载 → 残留清除流程（重点看官方卸载器
  等待是否正常、UWP 软件不出现在列表的预期行为）
- [ ] 卸载器派生副本提前退出的刷新体验（完成后手动刷新列表的引导是否足够）
- [ ] P1-8 大文件查找 / P1-9 Treemap / P1-7 回收站清理（需 ADR）仍是功能待办
