# 开发日志 2026-11-03：项目启动与需求对齐

## 今日目标

- 对齐 Purimaster 的产品定位、MVP 范围与技术栈
- 建立文档规范与开发过程规范，形成可长期执行的文档体系

## 完成情况

- ✅ 需求对齐完成，结论如下：
  - 平台：仅 Windows 10/11 x64
  - 技术栈：Rust + Tauri 2.x，前端倾向 Vue 3 + TS（**唯一待最终确认项**）
  - 定位：开源项目（MIT 方向）
  - MVP 四大功能：垃圾清理、大文件查找、重复文件查找、磁盘空间可视化
  - 竞品思路：借鉴 Geek 的极简、WizTree 的快、Czkawka 的准；明确不做注册表清理、伪优化、广告
- ✅ 文档体系建立：
  - `docs/requirements.md` 需求文档 v0.2
  - `docs/architecture.md` 技术栈与架构 v0.1
  - `docs/standards/documentation.md` 文档编写规范
  - `docs/standards/process.md` 开发过程规范
  - `docs/decisions/` ADR-0001（技术栈）、ADR-0002（规则即数据）
  - `README.md`、`.gitignore`

## 遇到的问题与解决

- 前端框架未在启动会上拍板（Vue/React 均可胜任）→ 架构上把前端隔离在 `src/`，不阻塞 Rust 侧先行开发，下次会话确认

## 决策记录

- [ADR-0001](../docs/decisions/0001-rust-tauri.md)：Rust + Tauri 2，仅 Windows
- [ADR-0002](../docs/decisions/0002-rules-as-data.md)：清理规则用 TOML 数据文件，与代码分离

## 明日计划

- 确认前端框架选型，关闭 ADR-0001 中的待确认项
- 初始化 Tauri 2 + pnpm 工程骨架（`src/`、`src-tauri/`、`crates/` workspace）
- 首个垂直切片：磁盘空间扫描（jwalk 遍历 → 按目录聚合 → 前端列表展示），打通 IPC 与事件推送
