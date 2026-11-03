# 开发日志 2026-11-03（二）：工程初始化与首个垂直切片

## 今日目标

- 初始化 Tauri 2 + Vue 3 工程骨架（`src/`、`src-tauri/`、`crates/` workspace）
- 首个垂直切片：磁盘空间扫描（jwalk 遍历 → 按目录聚合 → 前端树形展示），打通 IPC 与事件推送

## 完成情况

- ✅ 工程骨架：Cargo workspace（`crates/core` + `src-tauri`）、pnpm 前端（Vue 3.5 + TS + Vite 6）
- ✅ `purimaster-core`：`scan_space()` 多线程遍历（jwalk）→ 目录聚合（size/files/dirs）→ 目录树（子节点按大小降序）；支持取消与进度回调；5 个单测
- ✅ Tauri 壳：`start_scan`/`cancel_scan` 命令 + 4 个事件（`scan://progress|complete|cancelled|failed`），进度事件 150ms 节流；任务注册表管理生命周期
- ✅ 前端：路径输入 → 扫描 → 可折叠目录树组件（递归渲染），实时进度
- ✅ 质量门禁全绿：`cargo test` 5/5、`clippy -D warnings` 0 警告、`fmt --check` 通过；前端 `vue-tsc` + `vite build` 通过（JS 产物 72KB / gzip 28.8KB）
- ✅ 前端选型落定：Vue 3，`docs/architecture.md` 待确认项已关闭

## 遇到的问题与解决

1. **Rust 工具链未安装 + 首次安装被中断**
   - 本机原无 Rust；rustup 静默安装在上个会话结束时被中断，留下损坏的 toolchain（`missing manifest in toolchain`）
   - 解决：`rustup toolchain uninstall stable` 后重装，rustc/cargo 1.99.1 stable-msvc 就绪。MSVC Build Tools 2022 本机已有，无需额外安装
2. **tauri-build 在编译期要求 `src-tauri/icons/icon.ico`**（修正预判：原以为只有打包期需要）
   - 报错 `icon.ico not found; required for generating a Windows Resource file`
   - 解决：System.Drawing 生成占位图标（蓝底白 P，3.8KB），`bundle.icon` 已配置。**待办：发布前设计正式品牌图标并用 `pnpm tauri icon` 生成全套尺寸**
3. **取消检查逻辑 bug（单测抓到，修复）**
   - 原实现每 1024 个条目才检查一次取消标志，小目录（< 1024 条目）扫描时预置的取消标志永远读不到，`cancel_stops_scan` 测试失败
   - 这是真实语义漏洞而非测试环境问题；已改为逐条目检查（一次 `Relaxed` 原子读相对文件系统调用可忽略）
4. clippy 新 lint `manual_is_multiple_of` 一处 + fmt 格式一处，均已修复

## 决策记录

- 无新 ADR。延续 [ADR-0001](../docs/decisions/0001-rust-tauri.md)：前端选型 Vue 3 在本文档确认，属该 ADR 待确认项的关闭。

## 遗留风险与后续观察项

- `build_node` 为递归实现，极端深目录树（长路径支持开启）有栈溢出风险 → 观察项，必要时改显式栈迭代
- jwalk 读取失败的条目只计总数（`skipped: u64`），未记录路径 → 等做垃圾清理 UI 时再决定是否需要明细
- 符号链接不入树不计大小（防环路）→ 与 Czkawka 行为对齐，后续如需"链接文件查找"另行设计

## 明日计划

- `pnpm tauri dev` 冒烟测试：真实盘符扫描（C 盘百级 GB 数据）验证性能与 UI 体验
- 首次 git 提交（规范见 docs/standards/process.md：`feat(app): 首个垂直切片——目录空间扫描`）
- 下一功能：垃圾文件扫描规则引擎（ADR-0002 的 TOML schema 设计 + `crates/rules`）
