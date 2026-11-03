# 开发日志 2026-11-04（七）：删除安全框架与专项审查修复

## 今日目标

- 删除安全框架（ADR-0003）：回收站优先、dry-run、二次确认、逐文件上报
- 清理 UI：勾选、确认弹窗、试运行、报告
- 专项对抗审查并修复确认问题

## 完成情况

- ✅ `crates/platform/delete.rs`：唯一允许删除的模块
  - 五步校验 + 回收站可行性预检 + 删除真实路径 + 删除前一致性复检
  - **回收站预检**：非本地固定/可移动盘、`NoRecycleFiles=1`、文件超过卷容量 5% → 拒绝并提示改用永久删除（修复"Shell 静默永久删除被报告为已回收"）
  - 清理目标根是链接/联接点 → 拒绝
- ✅ 保护层升级：Known Folder API（`dirs`）解析个人目录（覆盖 OneDrive 迁移/重定向）；`%LOCALAPPDATA%`/`%APPDATA%` 加入 ancestor_only；OneDrive 目录加入 sensitive
- ✅ `crates/rules.clean()`：删除时重新枚举；归属去重与 scan 完全一致（子集选择不会多删）；未知规则前置报告；取消不误报；读取失败/非目录目标计入 skipped
- ✅ 壳层：`clean_junk`（回收站默认/永久删除/dry-run）；`take_clean_report` 支持切页后取回报告
- ✅ UI：规则勾选与全选、确认弹窗（**每次打开重置为安全默认**）、永久删除红色警告、试运行、报告弹窗（失败明细/跳过列表/取消标记/回收站语义文案）
- ✅ 测试 55 个（core 10 / platform 19 / rules 26），clippy / fmt / 前端构建全绿

## 专项审查（16 条发现 → 确认 10 条 + 批评者 3 条，全部处置或记录）

| 严重度 | 问题 | 处置 |
|--------|------|------|
| **high** | `trash` crate 在网络盘/超配额/回收站禁用时静默永久删除并返回成功，被报告为"已移入回收站（可恢复）" | 删除前预检（盘类型/禁用/5% 配额），不可回收则拒绝并提示改用永久删除 |
| medium | 校验对象与删除对象不一致（canon 校验、原始路径删除），junction 替换可重定向 | 改为按真实路径删除 + 删除前复检 + 目标根拒绝链接 |
| medium | 个人目录保护用固定名字面路径，未覆盖 Known Folder 重定向/OneDrive | `dirs` Known Folder API + OneDrive 环境变量 |
| **high（批评者）** | 保护名单缺 `%LOCALAPPDATA%`/`%APPDATA%`/OneDrive，恶意规则可整体清理 | 前者加入 ancestor_only，OneDrive 加入 sensitive |
| medium | 切页会取消已确认的清理并丢失逐文件报告 | 切页不再取消清理；报告持久化，`take_clean_report` 取回 |
| medium | 确认弹窗保留上次的"永久删除"/"试运行"选择 | 每次打开重置为回收站 + 非试运行 |
| low | 取消后把未处理规则误报"规则不存在" | 未知规则在开始前一次性判定 |
| low | 子集选择时实际删除量可能超过确认数字 | clean 归属算法与 scan 一致 |
| low | 回收站模式 freedBytes 被宣称为已释放 | 文案改为"清空回收站后释放" |
| low | 回收站路径零测试、部分测试因环境跳过但显示绿色 | 新增 preflight 测试；跳过时显式打印原因 |
| low | 取消时当前目标已发现的问题丢失 | 取消前先回写 problems/skipped |

## 遇到的问题与解决

1. `windows-sys` 的 `DRIVE_*` 常量在 `Win32::System::WindowsProgramming` 而非 FileSystem；需对应 feature
2. `GetDriveTypeW` 接受根路径，传文件全路径返回 `DRIVE_NO_ROOT_DIR`；新增 `volume_root()`（Prefix+RootDir）
3. 只读文件在现代 Rust `remove_file`（POSIX 语义）下仍可删除，原"只读失败"测试假设不成立；改用 `share_mode(0)` 独占句柄制造真实的共享冲突
4. 早前全盘基准 1683s 的结论已在上篇 devlog 记录为环境干扰（内存压力），未复测

## 决策记录

- ADR-0003 增补审查后的语义修正与**明确记录的残留风险**：TOCTOU 极小时间窗（同用户权限，无提权）、回收站配额按 5% 估算、文件占用检测依赖删除时报错

## 待办

- [ ] 用户冒烟测试：先 dry-run 看报告，再对缩略图缓存做一次真实回收站清理
- [ ] 空闲时复测 C 盘扫描基准
- [ ] 后续：句柄级删除（消除 TOCTOU）、回收站清空入口、规则引擎扩展（多 Profile 浏览器缓存）
