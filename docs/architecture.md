# Purimaster 技术栈与架构

- **版本**: v0.8
- **日期**: 2026-11-06
- **状态**: 已评审（项目启动会）
- **关联决策**: [ADR-0001 技术栈选型](decisions/0001-rust-tauri.md)、[ADR-0002 规则即数据](decisions/0002-rules-as-data.md)、[ADR-0004 软件卸载](decisions/0004-uninstaller.md)

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
- **命令**：`start_scan`（立即返回；根路径归一化——裸盘符 `C:` 归一为 `C:\`，盘符相对路径
  拒绝）/ `cancel_scan`（空间/垃圾扫描/清理共用同一取消注册表；对不存在的 clientId 返回
  错误）/ `dir_children`（按 nodeId 下钻）/ `list_files`（async + spawn_blocking，带条数上限）/
  `scan_junk`（规则驱动垃圾扫描，只读）/ `clean_junk`（按规则清理，回收站优先，支持 dry-run；
  单飞互斥——运行中拒绝新清理而非取消旧清理）/
  `scan_targeted` / `clean_targeted`（定向清理：用户指定单个目录的临时规则集，
  构造即过保护检查，清理时重新构造不信任扫描时点；选项打包为 `TargetedRequest`）/
  `delete_files`（空间分析手动勾选删除：逐文件点名走 `delete_file_standalone`，无批量根
  上下文，但逐文件保护检查/回收站预检/删除前复检一项不少）/
  `list_programs` / `uninstall_programs` / `scan_residue` / `delete_residue`（软件卸载：
  注册表卸载键枚举、逐个执行官方卸载器并等待、名称精确匹配的残留扫描、守卫式注册表键树
  删除——拒绝清单见 [ADR-0004](decisions/0004-uninstaller.md)；`remove_dir_tree_standalone`
  为新增的目录树删除原语）/
  `allow_close`（复位关窗保护）/
  `quit_app`（确认后取消全部任务并退出）/ `open_recycle_bin`（打开系统回收站）/
  `ipc_hello`（通路自检）
- **事件**：`scan://progress|complete|cancelled|failed`（空间扫描，complete 携带
  junctions 联接点计数）；`junk://progress|complete|cancelled`（垃圾扫描，progress 报告
  规则推进与累计计数）；`targeted://complete|cancelled`（定向清理扫描结果，结构同规则扫描）；
  `clean://progress|complete`（清理进度与报告，含 cancelled 标记，规则清理/定向清理/
  手动删除/残留清除共用）；
  `uninstall://progress|complete`（卸载进度与逐软件结果）；
  `app://close-busy`（关窗被阻止，负载含 running 任务数与 cleaning 标记）。
  空间扫描的 `complete` 只携带根摘要 + 第一层子目录（几 KB），更深层由 `dir_children`
  按需拉取——全盘扫描不再产生几十 MB 单条载荷
- **进度节流统一约定**：所有 progress 事件由壳层按 150ms 节流（~6.6 次/秒），
  引擎（crates/rules）逐文件回调、不做节流
- **关窗保护**：有任务运行时首次 `CloseRequested` 被 `prevent_close` 并发
  `app://close-busy`，前端确认后走 `quit_app`；首次阻止后有 1.5 秒武装窗口，
  期间重复关窗被忽略（防双击 X 在确认框弹出前落穿退出）；武装期过后再次关闭视为
  坚持退出直接放行（覆盖 WebView 无响应收不到事件的场景）；任务表清空时复位保护状态
- **规则引擎的 scan_only**：规则可声明 `scan_only = true`——扫描正常统计、
  清理阶段跳过（仍参与归属去重），用于"有条件限制不宜自动清理"的目标
- **规则引擎的筛选启发式**（对标 CleanerML/winapp2/CCleaner，2026-11-05）：
  `min_age`（"24h"/"7d"/"2w"）按修改时间跳过近期文件——正在使用的临时/缓存文件
  几乎总是新写的（CCleaner 对 Windows Temp 默认只清 24 小时前的）；扫描与清理一致
  生效，被跳过文件以 `freshFiles` 计数透传 UI（检测透明化）。`paths` 支持通配符，
  按现存目录展开（`*` 不跨分隔符），覆盖多 Profile 应用缓存。
  `description` 为面向用户的一句话说明，展示在清理详情
- **大小可信度语义**：目录聚合附带 `unknownSubtree`（子树含无权限目录，祖先链传播）与
  junctions 计数，UI 据此提示"实际占用可能更大/与资源管理器对不上的原因"
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

## 变更记录

- v0.8（2026-11-06）：新增软件卸载——`list_programs`/`uninstall_programs`/`scan_residue`/`delete_residue` 命令与 `uninstall://` 事件；platform 新增 `registry.rs`（守卫式注册表删除，ADR-0004 拒绝清单）、`uninstall.rs`（卸载键枚举/官方卸载器执行/残留扫描）、`remove_dir_tree_standalone` 目录树删除原语
- v0.7（2026-11-06）：新增空间分析手动删除——`delete_files` 命令 + `delete_file_standalone`（平台层新增的无根单文件删除原语，与 `delete_file` 共享回收站预检/删除前复检尾段）；报告弹窗抽取为 `ReportModal` 共享组件
- v0.6（2026-11-06）：新增定向清理——`scan_targeted`/`clean_targeted` 命令与 `targeted://complete|cancelled` 事件，`RuleSet::targeted` 临时规则集构造器（构造即过保护检查、清理时重新构造）；确认弹窗逐项显示规则说明、删除方式语义说明与报告措辞精确化
- v0.5（2026-11-06）：前端易用性——新命令 `list_drives`（盘符枚举，空间分析快捷入口）、`list_files` 加 `offset` 分页（文件清单"显示更多"）；确认/报告弹窗支持 Esc 与遮罩关闭（关窗保护弹窗刻意除外）
- v0.4（2026-11-05）：规则引擎对标开源清理软件升级——`min_age` 文件年龄启发式（24h 默认，扫描/清理一致，freshFiles 透明计数）、`paths` 通配符多 Profile 展开、`description` 规则说明透传 UI
- v0.3（2026-11-05）：按模拟用户测试报告落地一轮——`start_scan` 根路径归一化（裸盘符/盘符相对路径）、清理单飞（拒绝式）与空间扫描单飞（取消式）、`cancel_scan` 对不存在任务返回错误、关窗保护加 1.5 秒武装窗口并随任务表清空复位、`open_recycle_bin` 命令、规则引擎 `scan_only` 属性、扫描索引 `unknownSubtree`/junctions、IPC 契约 serde 快照测试
- v0.2（2026-11-05）：IPC 新增 `junk://progress`、`clean://progress`、`app://close-busy` 事件与 `allow_close`、`quit_app` 命令；约定进度事件统一由壳层 150ms 节流；前端页签改 KeepAlive（切页不丢扫描/清理状态）
- v0.1（2026-11-03）：初版（项目启动会评审）
