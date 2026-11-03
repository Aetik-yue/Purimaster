# ADR-0001: 采用 Rust + Tauri 2，仅支持 Windows

- **状态**: 已接受
- **日期**: 2026-11-03

## 背景

Purimaster 定位为开源的 Windows 磁盘清理工具，需要在项目启动时确定桌面技术栈。约束条件：安装包体积要小（清理软件自身臃肿是行业笑柄）、性能要好（全盘扫描）、领域逻辑重度依赖 Windows 系统能力。

## 决策

- 后端：**Rust**（stable，edition 2024）
- 桌面框架：**Tauri 2.x**
- 平台范围：**仅 Windows 10/11 x64**，不考虑 macOS/Linux
- 前端：Vue 3 + TypeScript + Vite（待用户最终确认，替换只影响 `src/`）

## 备选方案

| 方案 | 不选的原因 |
|------|-----------|
| C# / .NET 8 + WPF/WinUI | Windows 原生体验最好，但团队更希望用 Rust，且 core 引擎未来移植性差 |
| Electron + 前端 | 包体 100 MB+、内存占用高，与"轻量"定位冲突 |
| C++ / Qt | 性能最强但开发效率与安全性代价大，不适合当前规模 |

## 后果

- 优点：安装包可控制在 10 MB 内；`crates/core` 与 Tauri 解耦后可独立测试、未来可做 CLI 或移植；Rust 调用 Win32 API（windows crate）成熟
- 代价：Windows 特有 API 需要经 FFI 封装，集中在 `crates/platform`；前端仍是 Web 渲染，极端大数据量视图需做虚拟化
- 跨平台承诺关闭，但 core/rules crate 不得引入 `windows` 依赖，平台相关代码全部隔离在 `crates/platform`
