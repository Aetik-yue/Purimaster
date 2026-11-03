# 开发日志 2026-11-04（六）：垃圾清理 UI、scan_junk 与保护层安全加固

## 今日目标

- 提交规则引擎（已完成：`b56ef71`）
- `scan_junk` 命令 + 垃圾清理 UI（只读）
- 对新增代码做对抗性审查并修复确认问题

## 完成情况

- ✅ 后端 `scan_junk`：clientId 关联、可取消、单飞（新扫描自动取消旧垃圾扫描）
- ✅ 规则启动加载：打包资源目录优先；开发模式（仅 debug 构建）从 exe 向上最多 4 层回退
- ✅ `bundle.resources` 配置：发布包携带 `rules/junk.toml`
- ✅ 前端：顶部页签「空间分析 / 垃圾清理」；JunkView 自动扫描、按分类分组、风险徽章、目标路径、问题提示、top-50 文件预览；SpaceView 从 App 抽离
- ✅ 类型契约：JunkScanResult（含 categories）与前端 types.ts 对齐
- ✅ 真机验证：12,294 文件 / 9.73 GB；无权限目录以 ⚠ 显式展示（不再静默消失）
- ✅ 门禁：39 个测试（core 10 + platform 12 + rules 17）、clippy、fmt、前端构建全绿

## 对抗审查结果（22 条发现 → 确认 7 条，1 条驳回）

| 严重度 | 问题 | 修复 |
|--------|------|------|
| **high** | 保护校验只做字符串比较，`..` 穿越 / `\\?\` 前缀 / 8.3 短名 / 联接点可绕过——验证者实测 `%USERPROFILE%\..\..\Windows\System32` 可扫进 System32 | `fs::canonicalize` 解析真实路径后比较（去 `\\?\` 前缀）；目标不存在视为无风险（删除前必须重新校验）；解析失败保守拒绝；新增真实 System32 三种绕过手法的回归测试 |
| medium | 切页不取消扫描 + 重挂自动再扫，可堆积并发扫描 | JunkView/SpaceView 卸载即 `cancel_scan`；后端垃圾扫描单飞 |
| low | 异步 `listen` 在卸载后 resolve 会泄漏监听器并启动孤儿扫描 | `disposed` 守卫 + 迟到句柄立即注销 |
| low | 路径去重大小写敏感，GPO 改写 TEMP 大小写会重复计数 | 去重键统一小写 + 分隔符归一 |
| low | `fs::metadata` 所有错误被当作"应用未安装"静默跳过 | 仅 `NotFound` 静默；其它错误进入 problems 并计入 skipped |
| low | 不可读目录让整条规则从 UI 消失 | `collect_files` 读取失败写入 problems（真机立即复现：WinSAT、VMS 临时目录、`C:\WINDOWS\Temp` 均可见） |
| low | `junk://failed` 在文档/前端存在但从不发送 | 契约收敛为 `junk://complete\|cancelled`，文档与代码同步 |

驳回 1 条：clientId 冲突覆盖论（UUID 生成路径下不可达）。

## 决策记录

- 保护校验语义升级：**路径真实性优先**（canonicalize），不存在的目标放行但删除时必须重校验——写入 platform 文档注释与本节
- 垃圾扫描采用"单飞"策略（后到者取消前者），避免 UI 异常导致的并发堆积

## 待办

- [ ] **删除安全框架**（下一步核心）：回收站优先、dry-run、二次确认、文件占用检测、删除时重新校验目标
- [ ] 用户 UI 冒烟测试（垃圾清理页签）
- [ ] 空闲时复测 C 盘扫描基准（此前 1683s 为内存压力所致，待空闲验证）

## 明日计划

- 删除安全框架设计（ADR：删除策略与回收站/永久删除边界）
- 实现 `clean_junk`（先 dry-run，再真实删除到回收站）
- 清理 UI 勾选与确认流程
