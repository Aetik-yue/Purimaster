# 开发日志 2026-11-06（三）：空间分析手动删除（用户点名删文件）

## 今日目标

- 扫描之后让用户自己勾选特定文件删除（空间分析页的树里直接勾、底部选择栏发起）

## 设计决策

- **新删除原语 `delete_file_standalone`（platform 层）**：与规则清理的 `delete_file`
  的差别只在"没有批量清理的目标根上下文"——跳过根相关校验（根非链接/根保护检查/
  文件在根之下），**逐文件检查一项不少**：普通文件确认、真实路径保护检查
  （System32/个人目录/OneDrive 下的文件照拒）、回收站预检、删除前复检。
  两条路径共享尾段 `delete_checked_canonical`（预检→dry-run 门→复检→按真实路径删除），
  保持"删除单点收敛在 platform"的 ADR-0003 原则；ADR 正文按不可变规则不改动，
  原语差异记录在本模块文档与 architecture.md
- **命令 `delete_files`**：逐文件点名 → 复用 `CleanReport` 结构与
  `clean://progress|complete` 事件 → **SpaceView 自己的报告弹窗展示**。
  **刻意不写 `last_clean`**：那是 JunkView 的兜底通道，写入手动删除的报告会在
  首次进入垃圾页时被误弹出
- 单飞复用 `clean_active`（同一时间只有一批删除）；文件数上限 1 万
- 前端：勾选状态集中存放在 SpaceView（provide/inject），树节点只勾选不删除——
  删除入口保持单点；`ReportModal` 从 JunkView 抽出为共享组件（两视图复用，
  支持 `extraNote` 场景附注）
- 手动删除的报告附注："目录大小为扫描时的快照，删除不会自动更新数字；重新扫描
  可刷新"——树里的大小是 ScanIndex 快照，诚实标注而不是假装实时

## 交互流程

展开目录 → 文件行出现勾选框 → 勾选若干 → 底部悬浮选择栏（已选 N 个 · X GB）→
「删除所选」→ 确认弹窗（文件清单 + 回收站/永久删除/预演 + 永久删除键入「删除」+
受保护路径自动拒绝的说明）→ 删除中状态（可取消）→ 共享报告弹窗（失败明细/打开回收站）。

## 遇到的问题与解决

1. `delete_file_standalone` 忘了在 platform/lib.rs 的 `pub use` 里再导出 → E0425
2. 模板里嵌套 `Ref<Map>` 不自动解包 → `selection.selected.value.has(...)` 显式取值
3. 报告弹窗重构时误删了 `confirmTargetedClean` 的函数声明行（Edit 锚点含声明行）——
   立即发现补回；教训：大块替换的锚点不要包含紧邻的下一行

## 门禁结果

`cargo fmt --check` ✓ ｜ `cargo clippy -D warnings` ✓ ｜ `cargo test --workspace`
**76 通过**（platform 24，新增 standalone 5 个：dry-run/真删/拒绝目录/拒绝缺失/
拒绝 System32 hosts）✓ ｜ `pnpm lint` ✓ ｜ `pnpm build` ✓

## 待办

- [ ] 用户真机冒烟：勾选删除全流程（预演 → 回收站真删）、受保护文件拒绝提示、
  跨目录多选
- [ ] P1-8 大文件查找 / P1-9 Treemap / P1-7 回收站清理（需 ADR）
