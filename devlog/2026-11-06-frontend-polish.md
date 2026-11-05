# 开发日志 2026-11-06：前端易用性打磨

## 今日目标

- 继续前端改进：补齐模拟测试遗留的交互缺口（截断不可达、空态首屏、弹窗键盘操作）

## 完成情况

- ✅ **空间分析空态首屏**（原 F-02 遗留的另一半）：无结果时不再一片空白——显示引导文案
  + 本机盘符快捷按钮（新命令 `list_drives`，platform 层 GetLogicalDriveStringsW +
  GetDriveTypeW；UI 只展示固定盘/可移动盘，点按即扫描）
- ✅ **弹窗键盘与遮罩操作**：确认弹窗与报告弹窗支持 Esc 关闭、点击遮罩关闭
  （确认弹窗 Esc = 取消，不删任何东西；报告遮罩 = "好的"）；**关窗保护弹窗刻意不响应
  Esc/遮罩**——安全确认不允许被绕过。永久删除确认词输入框自动聚焦
- ✅ **垃圾扫描细进度条**：按规则推进（`ruleIndex/ruleCount`），150ms 节流事件驱动，
  补上"有总量刻度"的视觉反馈（文字进度之外）
- ✅ **目录树文件清单分页**（补 F-26 的文件侧）：`list_files` 加 `offset` 参数
  （core 签名变更 + 分页单测），截断提示处新增"显示更多文件"按钮，逐页追加；
  子目录侧上一轮已有"显示更多"
- ✅ UI 小样：进度条 transition、盘符按钮 hover/active 态、加载中按钮禁用态

## 遇到的问题与解决

1. `driveButtons` 初版写成普通数组（setup 时 drives 还是空）——改为 `computed`，
   异步加载后自动更新
2. `encode_wide` 只存在于 `OsStr`——`GetLogicalDriveStringsW` 返回的 `&str` 片段需先转
   `OsStr` 再编码宽字符
3. core `list_files` 签名变更波及 3 处测试调用点，统一补 `offset: 0`

## 门禁结果

`cargo fmt --check` ✓ ｜ `cargo clippy -D warnings` ✓ ｜ `cargo test --workspace`
**67 通过**（core 13 / platform 19 / rules 35，新增 list_files 分页测试）✓ ｜
`pnpm lint` ✓ ｜ `pnpm build` ✓

## 待办

- [ ] 用户真机冒烟：盘符按钮、Esc 关弹窗、文件"显示更多"逐页加载
- [ ] P1-8 大文件查找 / P1-9 Treemap 仍是下一个功能专项
- [ ] P1-7 回收站清理（先立 ADR）；P1-15 NSIS 安装链路验证
