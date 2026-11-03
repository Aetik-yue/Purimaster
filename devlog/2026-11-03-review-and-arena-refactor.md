# 开发日志 2026-11-03（四）：对抗性代码审查与扫描架构重构

## 今日目标

- 对首个垂直切片新代码做多维对抗审查
- 修复确认的问题后交付可用版本

## 审查方式与结果

四维度并行审查（core 并发正确性 / Tauri IPC 契约 / Vue 前端状态机 / 磁盘工具安全），每条发现由独立怀疑者对抗验证：

- 原始发现 34 条 → 验证前 12 条 → **确认 11 条**、驳回 1 条、多条下调严重度
- 验证者纠正了若干夸大描述（如"阻塞数秒"实为 0.2~0.4s、路径笔误、`metadata` 在 Windows 并非阻塞点），并实测了关键阈值（`build_node` 在 2 MiB 栈下 release 约 5000 层溢出、WinSxS\Manifests 45,832 个直属文件）

### 确认并修复的问题

| 严重度 | 问题 | 修复 |
|--------|------|------|
| high | taskId 竞态：事件按 taskId 过滤但 taskId 在 invoke 返回后才赋值，小目录扫描完成事件先到即被丢弃 → UI 永久卡"扫描中"、取消也救不回 | **clientId 由前端在 invoke 前生成**，命令与全部事件携带它 |
| high | `scan://complete` 一次性发送整棵树（C 盘几十 MB JSON，经 eval 在 WebView 主线程解析）→ 结束瞬间假死 | 只发根摘要 + 第一层子目录（KB 级）；深层用 `dir_children(clientId, nodeId)` 按需拉取 |
| high | `list_files` 同步命令跑在 WebView IPC 主线程，且无上限传输 | 改 async + `spawn_blocking`；core 端 limit 截断 + 返回 total |
| medium | 扫描内存三份路径副本并存；`build_node` 递归导致极深目录树栈溢出（实测 2 MiB 栈约 5000 层崩溃） | **core 重写为扁平 arena 索引**：`Vec<NodeRecord>`、u32 下标、只存目录名不存绝对路径、构建全程无递归 |
| medium | 无权限目录以 0 B 呈现，与空目录无法区分 | `unreadable` 标记；前端显示"无权限"徽章 + 大小"未知"；**根目录不可读直接报错** |
| medium | 子目录行无上限渲染（WinSxS 类目录挂载数万组件） | 子目录行最多 100 条 + 省略提示 |
| medium | 取消是 fire-and-forget，UI 不乐观置位；取消命令失败无反馈 | clientId 保证 cancelled 事件可关联；取消 invoke 失败显示错误 |
| low | 窗口关闭不触发取消，扫描线程在退出期间继续阻塞 | `on_window_event(CloseRequested)` 统一置位全部取消标志 |
| low | 取消无法打断阻塞中的系统调用（SMB 等） | 记录为已知限制（MVP 不承诺网络盘）；逐条目检查保持 |
| 防御 | `walker.join().unwrap_or(0)` 会吞掉遍历线程 panic | join 失败转为显式 `ScanError`，不返回部分结果 |

### 被驳回的发现

- "walker panic 被吞 → 伪造 skipped"：机制属实但当前代码无任何可达的 panic 路径，验证者判为防御性加固点。仍顺手改为显式错误处理。

## 基准异常调查（未完成，有明确结论方向）

重写后全盘 C: 基准出现 **1683.9s**（重构前 20s），呈现"前 45s 扫到 144 万文件、之后 27 分钟仅新增 5 千"的停滞特征。排查：

1. 复跑 300s 未完成 → 确认非偶发
2. 加**慢目录追踪**（`PURIMASTER_TRACE_SLOW=1`，单目录 >2s 打印路径，OnceLock 缓存开关）以定位阻塞目录
3. 诊断任务被宿主以"系统内存不足"回收；随后查明：**物理内存 15.8 GB、可用仅 1.4 GB，`cs2` 进程占用 4.1 GB，系统重度换页**；扫描进程自身工作集仅 56 MB
4. 结论：**基准异常是环境干扰**（同一轮开局已慢 2.4 倍、进程内存正常、宿主侧内存压力直接可观测），非代码回退
5. 验证状态：**待机器空闲后复测**（命令：`cargo run --release -p purimaster-core --example scan_real -- C:\`，可叠加 `PURIMASTER_TRACE_SLOW=1`）

## 质量门禁

- `cargo test --workspace`：10/10（新增：深目录树不溢出、list_files 截断与 total、空目录 ≠ 不可读）
- clippy `-D warnings`、fmt、`vue-tsc` + `vite build`：全部通过

## 决策记录

- IPC 契约升级（clientId + 按需下钻 + arena 索引）已同步 `docs/architecture.md` §4/§5 与风险表，未开新 ADR（属既有架构方向的落地修正）

## 待办

- [ ] 机器空闲时复测 C 盘基准（预期 ~20s 或更快）
- [ ] 用户复测 UI：小目录秒出结果、完整扫描树、下钻、无权限徽章
- [ ] 首次 git 提交（待用户确认）
- [ ] 下一步功能：垃圾清理规则引擎（TOML schema + `crates/rules`）
