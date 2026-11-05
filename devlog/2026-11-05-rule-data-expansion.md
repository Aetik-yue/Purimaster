# 开发日志 2026-11-05（四）：规则数据对标扩充（9 → 18 条）

## 今日目标

- 上轮给引擎装了 min_age/通配/description 三项能力后，这轮去"规则文献"里找内容本身：
  哪些位置是公认垃圾、哪些是伪优化、哪些绝对不能碰

## 调研结论（链接见 improvement-plan §7）

1. **BleachBit cleaners 源码**（discord.xml 等）：Electron 应用缓存的标准三件套
   `Cache / Code Cache / GPUCache`，**不碰** Network Persistent State / Preferences 等设置数据；
   应用清单覆盖 VS Code、Discord、Slack、Brave、Vivaldi 等几十个
2. **DirectX 着色器缓存**（`%LOCALAPPDATA%\D3DSCache`）：安全可清，但代价是游戏下次启动
   重新编译着色器会卡顿——所以配 `min_age = "7d"` 只清旧缓存
3. **Delivery Optimization 缓存**：安全但服务占用中 + 需管理员，官方途径是磁盘清理工具
   → **不加规则**（标准用户扫描只会产生"拒绝访问"噪音）
4. **Prefetch 争论定案**：微软官方说明 Prefetch 是加速启动的性能缓存，删除反而变慢
   （windowslatest.com 2026-10），体积上限约 1 MB → **写进 junk.toml 头部的"不做清单"**，
   防止未来贡献者手痒添加

## 规则数据升级（9 → 18 条 / 4 → 6 分类）

- 新增 app-cache 分类：VS Code、Discord、Slack（Chromium 三件套 + Slack 的 Service Worker 缓存）
- 新增 dev-cache 分类：npm（`%LOCALAPPDATA%\npm-cache`）、pip（`pip\cache`）、
  Yarn v1（`Yarn\Cache`）——全部是各包管理器**官方声明可清**的 cache 目录；
  刻意不清 `.nuget\packages` 这类"实体包"目录（破坏离线还原）
- 新增 system-residue：DirectX 着色器缓存（7d）、Windows 系统网络缓存 INetCache（24h）
- 新增 crash-dump：系统蓝屏小转储 `%SystemRoot%\Minidump`（medium / 7d；正常系统目录
  不存在 → 静默跳过不出现在 UI，出过蓝屏才现身）
- 每条规则的 description 按统一口径重写：是什么 / 为什么可清 / 清理的代价

## 真机验证（只读示例，release）

18 条规则全部加载成功。新增规则在本机命中：npm 缓存 **258.86 MB**（1,015 文件）、
pip 缓存 **140.53 MB**、VS Code 缓存 **60.41 MB**、着色器缓存 7.97 MB、INetCache 0.14 MB；
蓝屏小转储 0（本机无蓝屏，符合设计）。合计 10.09 GB / 12,815 文件——此前 4.6 GB 的
开发缓存是扫描盲区。

## 门禁结果

`cargo test -p purimaster-rules` 35 个全绿（规则数据变更不影响引擎代码；
load_dir 严格校验通过即证明 TOML schema 合法）✓

## 待办

- [ ] 用户真机冒烟：新分类在 UI 的分组展示、npm/pip 缓存清理实测
- [ ] 回收站清理（P1-7，需先立 ADR）仍是下一个功能专项
- [ ] 规则信任模型 ADR（R4/P2-22）在规则数量增长后优先级上升——社区规则共享前必须做
