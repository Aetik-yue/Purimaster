# Purimaster 技术栈与架构

- **版本**: v0.1
- **日期**: 2026-11-03
- **状态**: 已评审（项目启动会）
- **关联决策**: [ADR-0001 技术栈选型](decisions/0001-rust-tauri.md)、[ADR-0002 规则即数据](decisions/0002-rules-as-data.md)

---

## 1. 选型结论

| 层 | 选型 | 说明 |
|----|------|------|
| 桌面框架 | **Tauri 2.x** | 体积小、安全模型好；系统 API 通过 Rust 侧封装 |
| 后端语言 | **Rust**（stable， edition 2024） | 性能与安全兼得 |
| 前端 | **Vue 3 + TypeScript + Vite** | 已在首个垂直切片中落地（2026-11-03） |
| UI 库 | Naive UI（候选） | 候选还包括 Element Plus / shadcn-vue |
| 包管理 | pnpm（前端）、cargo（Rust） | |
| 可视化 | ECharts（Treemap） | 候选自绘 Canvas，视性能定 |

> 前端框架已于 2026-11-03 确认为 Vue 3，如未来需更换（React/Svelte/Solid），仅影响 `src/` 目录，不影响 Rust 侧架构。

## 2. 工程结构

```
Purimaster/
├── src/                        # 前端（Vue 3 + TS）
├── src-tauri/                  # Tauri 壳：IPC 命令、事件，保持"薄"
│   └── src/
│       ├── main.rs
│       └── commands/           # 每个功能一个 command 模块
├── crates/                     # 纯 Rust 工作区，不依赖 Tauri
│   ├── core/                   # 扫描/分析引擎（遍历、聚合、去重）
│   ├── rules/                  # 清理规则引擎（加载 TOML 规则）
│   └── platform/               # Windows 特化：回收站、系统路径、权限
├── docs/                       # 项目文档
├── devlog/                     # 开发日志
└── rules/                      # 清理规则数据文件（TOML）
```

**分层原则**：`src-tauri` 只做 IPC 胶水；一切领域逻辑进 `crates/`，保证引擎可独立测试、未来可移植（CLI / 跨平台）。

## 3. 核心依赖（Rust 侧）

| crate | 用途 |
|-------|------|
| `tauri` 2.x | 桌面框架 |
| `tokio` | 异步运行时（长任务、取消） |
| `rayon` | 多线程并行目录遍历（自研 walker，见下） |
| `trash` | 移入回收站（跨实现一致） |
| `windows` / `windows-sys` | Win32 API（系统路径、卷信息；后期 MFT） |
| `blake3` | 内容哈希（快速、128 位以上安全裕度） |
| `serde` + `toml` + `globset` | 规则文件、IPC 序列化、规则 glob 匹配 |
| `tracing` | 结构化日志 |

> **为什么不用 jwalk**（2026-11-03 实测）：jwalk 的 `DirEntry::metadata()` 每文件一次额外
> 系统调用且串行执行，NVMe 上仅 ~3000 文件/秒；改用 rayon 自研 walker +
> `std::fs::read_dir`（Windows 上 metadata 零额外系统调用）后 ~145,000 文件/秒（48 倍）。

版本以 `Cargo.toml` 实际锁定为准，新增重要依赖需在开发日志中说明理由。

## 4. IPC 设计

- **clientId 关联**（2026-11-03 代码审查定案）：前端在 invoke **之前**生成 clientId，
  命令与所有事件都携带它——事件先于命令返回到达也能正确关联，消除时序竞态
- **命令**：`start_scan`（立即返回）/ `cancel_scan`（空间/垃圾扫描共用同一取消注册表）/
  `dir_children`（按 nodeId 下钻）/ `list_files`（async + spawn_blocking，带条数上限）/
  `scan_junk`（规则驱动垃圾扫描，只读）/ `ipc_hello`（通路自检）
- **事件**：`scan://progress|complete|cancelled|failed`（空间扫描）；
  `junk://complete|cancelled`（垃圾扫描）。空间扫描的 `complete` 只携带根摘要 +
  第一层子目录（几 KB），更深层由 `dir_children` 按需拉取——全盘扫描不再产生几十 MB 单条载荷
- **索引驻留**：`ScanIndex`（扁平 arena：`Vec<NodeRecord>`，u32 下标、只存目录名不存
  绝对路径、构建全程迭代无递归）保存在 Tauri 壳层，仅保留最近一次扫描
- 数据结构在 `crates/core` 定义，`serde::Serialize` 派生（`NodeSummary` 用 camelCase），
  Tauri 层透传

## 5. 关键技术点

1. **并行遍历**：rayon `scope_fifo` 自研 walker（每目录一个任务，`std::fs::read_dir` 枚举 + 缓存 metadata）；权限不足目录跳过并计数，无权限目录仍入树（大小 0）
2. **去重三级流水**：按大小分组 → 组内读首/尾 4KB 哈希 → 仍相同的才全量 blake3
3. **删除安全框架**（详见需求文档 §3.1）：默认回收站、永久删除二次确认、关键路径硬编码排除、全量操作日志
4. **扫描取消**：tokio `CancellationToken` 贯穿遍历回调
5. **可视化**：一次扫描产出目录树索引，前端 Treemap（ECharts）下钻；> 百万节点时前端聚合分页

## 6. 风险与对策

| 风险 | 对策 |
|------|------|
| Tauri 事件高频推送卡 UI | 进度事件 150ms 节流；结果按需下钻，不再整树传输 |
| 扫描大目录树的内存与栈安全 | 扁平 arena 索引（无递归构建、不存绝对路径）；路径映射表扫描结束即释放 |
| MFT 直读（WizTree 式加速）需管理员权限且复杂 | v0.1 不做，v0.2 调研，走 ADR |
| 浏览器缓存路径随版本变化 | 规则即数据（ADR-0002），规则文件可独立更新 |
| 中文/特殊字符路径 | 全链路 UTF-8 + Windows 宽字符 API 封装集中在 `crates/platform` |
