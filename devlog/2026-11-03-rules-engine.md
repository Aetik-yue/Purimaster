# 开发日志 2026-11-03（五）：清理规则引擎（crates/rules + crates/platform）

## 今日目标

- 首次 git 提交（已完成：`68f2664`，36 文件 / 7878 行）
- 实现垃圾清理规则引擎：TOML 规则加载、校验、环境变量展开、glob 匹配、只读扫描
- 系统关键路径保护硬编码层（ADR-0002）

## 完成情况

- ✅ `crates/platform`：保护路径两级语义
  - **sensitive**（等于/位于内部/是其祖先 → 一律拒绝）：System32、SysWOW64、WinSxS、boot、Program Files、(x86)、个人目录（Documents/Desktop/Pictures/Videos/Music/Downloads/Favorites）
  - **ancestor_only**（等于它或是它的祖先 → 拒绝整体清除，内部子目录允许）：SystemRoot、ProgramData、USERPROFILE
  - 盘符根/UNC 根拒绝；相对路径拒绝；规范化后大小写不敏感、分隔符容错、按路径段前缀比较（`C:\WindowsOld` 不会误判）
  - 8 个单测
- ✅ `crates/rules`：
  - TOML schema（`deny_unknown_fields` 严格模式）：分类、规则 id/标题/风险等级/目标/包含/排除
  - 加载时严格校验：重复 id、未定义分类、空路径、glob 语法、分类定义冲突
  - `%VAR%` 环境变量展开（未定义变量保持原样并标记为不可用目标）；同一规则内展开后重复的目标自动去重
  - glob：相对目标目录、`/` 分隔、大小写不敏感、`literal_separator`
  - 扫描：按规则分组统计、每规则 top-50 最大文件预览、跨规则按路径去重、同步取消检查、缺失目标静默跳过
  - 17 个单测
- ✅ `rules/junk.toml`：8 条初始规则 / 4 个分类（Temp、浏览器缓存、系统残留、崩溃转储）
- ✅ 真机只读验证：**12,516 个文件 / 9.69 GB**（Edge 缓存 ~1GB、Windows 更新缓存 1.95GB、Temp 6.7GB，含一个 2.18GB 沙箱 swap.vhdx）；保护逻辑正常拒绝/放行目标
- ✅ 门禁：35 个测试（core 10 + platform 8 + rules 17）、clippy、fmt 全绿

## 遇到的问题与解决

1. **ancestor_only 判断方向写反**：`is_strict_prefix(&a, &t)` 应为 `is_strict_prefix(&t, &a)`，否则 `%SystemRoot%\Temp` 会被误拒——被 platform 单测当场抓住
2. **取消检查缺入口检查点**：目标全部不存在时扫不到任何文件，预置的取消标志永远读不到；在 scan 入口与每条规则处补检查
3. **目标展开重复**：`%TEMP%` 与 `%LOCALAPPDATA%\Temp` 在常规系统上是同一路径，导致目标列表重复（文件计数已有去重，列表没有）；改为编译期按展开后路径去重，补测试
4. clippy：`manual err`、两处 `collapsible_if`，均已按建议修正

## 决策记录

- 保护路径分 sensitive / ancestor_only 两级（实现层细节，已写入 `crates/platform` 文档注释）；**未开新 ADR**，是 ADR-0002 的落地
- 已知限制（记录待办）：
  - 浏览器缓存只覆盖 Default Profile（多 Profile 遍历后续版本）
  - 回收站清理需要 Shell API，不在本引擎（后续单独规则类型）
  - `%TEMP%` 中的 2.18GB 沙箱 swap.vhdx 这类"在用的大文件"会被列为候选——删除阶段的"文件占用检测"必须先于删除实现

## 明日计划

- Tauri 命令 `scan_junk`（clientId 关联 + 事件）与垃圾清理 UI（按分类分组、风险标识、预览）
- **删除安全框架**：dry-run、回收站优先、二次确认、文件占用检测
- 真机复测完整清扫流程
