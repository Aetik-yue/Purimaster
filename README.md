# Purimaster 净化大师

> 轻量、干净、值得信任的 Windows 磁盘清理工具

![Status](https://img.shields.io/badge/status-%E8%A7%84%E5%88%92%E9%98%B6%E6%AE%B5-orange)

## 这是什么

Purimaster 是一款开源的 Windows 磁盘清理工具，基于 Rust + Tauri 构建。
目标是做出一个**没有广告、没有捆绑、没有遥测、删得放心**的清理软件。

核心能力（规划中）：

- 🧹 **垃圾文件扫描清理** — Temp、浏览器缓存、回收站、系统残留等
- 📦 **大文件查找** — 快速定位占用空间的大文件
- 👯 **重复文件查找** — 基于内容哈希的精确去重
- 🗺️ **磁盘空间可视化** — 目录空间占用分析，一眼看出空间去哪了

## 项目状态

🚧 **规划阶段** — 正在进行需求对齐与架构设计，尚未开始编码。

## 文档导航

| 文档 | 说明 |
|------|------|
| [需求文档](docs/requirements.md) | 产品定位、功能需求、竞品分析 |
| [技术栈与架构](docs/architecture.md) | 技术选型、工程结构、关键技术点 |
| [文档编写规范](docs/standards/documentation.md) | 文档怎么写、放哪里、怎么命名 |
| [开发过程规范](docs/standards/process.md) | 分支、提交、测试、质量门禁 |
| [架构决策记录](docs/decisions/) | 重要技术决策及其背景（ADR） |
| [开发日志](devlog/) | 按日期记录的开发流水 |

## 开源协议

MIT（待最终确认，见 [ADR-0001](docs/decisions/0001-rust-tauri.md)）
