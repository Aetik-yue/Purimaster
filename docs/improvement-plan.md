# Purimaster 改进方案

- **版本**: v0.1
- **日期**: 2026-11-05
- **状态**: 草稿
- **依据**: 全量源码审读（crates/ 三模块、src-tauri 壳层、src/ 前端、rules/junk.toml）、docs/ 全部文档、devlog/ 全部日志（2026-11-03 至 11-04）

---

## 1. 现状评估

工程质量文化优秀：分层清晰（壳层薄胶水 / 领域逻辑进 crates）、规则即数据、删除安全框架有真实对抗审查记录、39 个单测、devlog 如实记录问题。这些是项目的核心竞争力，应当保持。

对照需求文档 MVP（v0.1）四大功能的完成度：

| 功能 | 状态 | 说明 |
|------|------|------|
| 垃圾扫描清理 | **约 80%** | 规则引擎 + 安全删除框架已落地；缺回收站清理、缺清理进度反馈、规则覆盖薄 |
| 大文件查找 | **0%** | 需求 §3.2 完全未做（但数据基础 ScanIndex 已具备） |
| 重复文件查找 | **0%** | 需求 §3.3 完全未做（blake3 依赖未引入） |
| 空间可视化 | **约 50%** | 树形下钻已可用；需求 §3.4 要求的 Treemap / 条形图未做 |

另有两个"规范与现实脱节"的系统性问题：**无 CI**（process.md §1 要求"合入 main 一律走 PR，CI 绿"，但仓库没有 `.github/`）、**无前端 lint**（process.md §5 门禁要求 `pnpm lint`，但 package.json 没有该 script，也没有 ESLint/Prettier 配置）。

README「项目状态：规划阶段——尚未开始编码」已严重过时。

---

## 2. 缺陷与改进点清单

严重度：🔴 高（影响信任/可用性）｜🟡 中（MVP 收尾必须）｜⚪ 低（打磨项）

### 2.1 产品功能缺口

| # | 严重度 | 问题 | 证据 |
|---|--------|------|------|
| F1 | 🔴 | **回收站清理缺失**。需求 §3.1 扫描范围明确包含"回收站"，devlog 2026-11-04（七）待办也列了"回收站清空入口"。当前规则引擎只能表达"目录内文件"，表达不了"回收站"这种 Shell 特殊目标 | `docs/requirements.md` §3.1、`devlog/2026-11-04-delete-safety.md` 待办 |
| F2 | 🟡 | **大文件查找未做**。`ScanIndex` 里数据齐全，补一个 `top_files` IPC 即可，投入小收益大 | `docs/requirements.md` §3.2 |
| F3 | 🟡 | **Treemap 可视化未做**。当前只有树形列表；架构文档计划用 ECharts | `docs/requirements.md` §3.4、`docs/architecture.md` §1 |
| F4 | 🟡 | **规则覆盖薄**（仅 8 条）：浏览器缓存只覆盖 Chrome/Edge 的 Default Profile（规则文件注释自认）；Firefox 缺失；无 Delivery Optimization、DirectX Shader Cache、Defender 扫描残留、Windows.old 检测 | `rules/junk.toml` |
| F5 | ⚪ | 重复文件查找未做（v0.2 规划内，此处仅登记） | `docs/requirements.md` §3.3 |

### 2.2 交互与体验

| # | 严重度 | 问题 | 证据 |
|---|--------|------|------|
| U1 | 🔴 | **清理全程无进度反馈**。`clean_junk` 只在结束时发一次 `clean://complete`；数万文件的清理期间 UI 停在"正在清理…"，用户无法判断是否卡死。垃圾扫描同理（无 `junk://progress`） | `src-tauri/src/lib.rs:312-326`、`crates/rules/src/lib.rs:558` |
| U2 | 🔴 | **空间分析切页丢结果**。App.vue 用 `v-if` 切页签，SpaceView 卸载即销毁 `result`；用户花 20 秒扫完 C 盘，切去垃圾清理页再切回来必须重扫。JunkView 已用"会话缓存 + 后端缓存"解决同类问题，SpaceView 没有对称处理 | `src/App.vue:57-58`、`src/views/SpaceView.vue:14` |
| U3 | 🟡 | **关窗直接取消所有任务并退出**，清理中的批次被静默中断、逐文件报告丢失。应弹确认（"清理进行中，确定退出？"） | `src-tauri/src/lib.rs:409-417` |
| U4 | 🟡 | **风险等级未落地"确认强度"**。规则注释与需求都写了"UI 据此决定确认强度"，但确认弹窗对所有规则一视同仁；high 规则应要求更强确认 | `crates/rules/src/lib.rs:100`、`src/views/JunkView.vue:382` |
| U5 | 🟡 | **安装链路从未验证**。devlog 没有任何 `tauri build` NSIS 产物的验证记录：`bundle.resources` 在安装形态下规则能否加载、安装包体积 < 10 MB 需求、空闲内存 < 150 MB 需求，均未验证 | `src-tauri/tauri.conf.json:30-33`、`docs/requirements.md` §4 |
| U6 | ⚪ | IPC 自检灯（"IPC：正常 ✓"）是开发调试元素，不应常驻产品 UI；窗口无版本号/关于入口 | `src/App.vue:51-53` |
| U7 | ⚪ | 清理粒度只能整条规则勾选，详情里看到的大文件无法单独排除（主流清理软件支持文件级勾选） | `src/views/JunkView.vue` |

### 2.3 性能

| # | 严重度 | 问题 | 证据 |
|---|--------|------|------|
| P1 | 🟡 | **`protection()` 每次调用都重建保护列表**。`check_target` → `protection()` 每次执行 6 次 Known Folder API + 十几个环境变量读取；而 `delete_file` 每个文件调用 `check_target` 两次（步骤 3、5）。清 1 万个文件 ≈ 2 万次保护列表构建。用 `OnceLock` 缓存即可（系统路径进程内不变） | `crates/platform/src/lib.rs:39`、`crates/platform/src/delete.rs:90,102` |
| P2 | ⚪ | **垃圾扫描串行**。`collect_files` 单线程逐目录栈遍历；空间扫描已是 rayon 并行，垃圾扫描没有。Windows 更新 Download 目录可达数 GB / 数万文件时会慢 | `crates/rules/src/lib.rs:436` |
| P3 | ⚪ | **扫描期内存优化空间**：`path_ids: HashMap<PathBuf, u32>` 存全部目录绝对路径，遍历线程发完整 `PathBuf`、消费线程再做 `ancestors()` 查表；mpsc channel 无界，遍历快于消费时内存堆积。可改为遍历时直传 `parent_id` + 有界 channel 背压。注：1683s 基准是内存压力环境所致（空闲基线 20s），此项收益在低内存机器上更明显 | `crates/core/src/lib.rs:180,211` |
| P4 | ⚪ | junction 不入树不计大小是正确的安全选择，但空间分析对 `C:\Users\All Users` 等联接点静默跳过无任何提示，大目录数字对不上时用户会困惑 | `crates/core/src/lib.rs:383` |

### 2.4 可靠性与安全

| # | 严重度 | 问题 | 证据 |
|---|--------|------|------|
| R1 | 🔴 | **打包后无任何日志**。全部诊断走 `eprintln!`（stderr），NSIS 安装版看不到；用户报障时无据可查。需要 `tracing` + 本地滚动文件 appender（零遥测约束下，日志只留本地） | `src-tauri/src/lib.rs:47` |
| R2 | 🟡 | **清理无单飞**。垃圾扫描有 `junk_active` 单飞防堆积，`clean_junk` 没有；双实例或 IPC 直调可并发两批清理。同时建议引入 `tauri-plugin-single-instance` | `src-tauri/src/lib.rs:186-213`（扫描有）vs `:269`（清理无） |
| R3 | 🟡 | **CSP 为 null**。Tauri 安全模型下建议配置基本 CSP 作为纵深防御（应用虽无远程内容，成本几乎为零） | `src-tauri/tauri.conf.json:23` |
| R4 | ⚪ | **规则信任模型缺口**。系统路径保护已闭环（硬编码在 platform），但被篡改的规则仍可清理任意非保护目录（如 `%LOCALAPPDATA%` 下任意应用目录）。单机自用风险低；上线"社区规则"前必须有签名/哈希校验机制，应提前立 ADR | `docs/decisions/0002-rules-as-data.md` |
| R5 | ⚪ | `expand_env` 用 `std::env::var`，非 UTF-8 环境变量值会被当作未定义；`var_os` 更稳（极端罕见，顺手修） | `crates/rules/src/lib.rs:401` |

### 2.5 工程与流程

| # | 严重度 | 问题 | 证据 |
|---|--------|------|------|
| E1 | 🔴 | **无 CI**。门禁（fmt/clippy/test/前端 build）全靠手工执行，与 process.md "CI 绿才能合入"脱节 | 缺 `.github/` |
| E2 | 🟡 | **前端 lint 缺失**。process.md 门禁写明 `pnpm lint`，但 package.json 无此 script、无 ESLint/Prettier 配置 | `package.json` |
| E3 | 🟡 | **错误文案中文硬编码在核心 crate**。`ScanError`/`RuleError`/`DeleteError` 的 Display 是中文字符串；需求预留 i18n，若不趁代码量小时把错误改为结构化（code + 参数、前端翻译），后续是大面积返工 | `crates/core/src/lib.rs:119`、`crates/rules/src/lib.rs:36` |
| E4 | ⚪ | **前端零测试**。后端 39 个单测，前端 0 个；`format.ts` 等纯函数和确认弹窗逻辑可先被 vitest 覆盖 | `package.json` |
| E5 | ⚪ | **TS 类型手工镜像 Rust 结构**，无生成机制，存在漂移风险（可在 IPC 契约变更时引入 tauri-specta） | `src/types.ts` |
| E6 | ⚪ | 前端重复逻辑未提取（`genId`/`onEvent` 在 SpaceView、JunkView 各写一份）；版本号三处（package.json / tauri.conf.json / Cargo.toml）手工同步 | `src/views/*.vue` |

### 2.6 做得对、不要动的

- 删除单文件的八步校验顺序与"唯一删除模块"设计（`crates/platform/delete.rs`）
- 规则加载严格校验 + 保护路径硬编码双保险（ADR-0002/0003 闭环）
- 扫描索引扁平 arena + 按需下钻的 IPC 载荷控制
- devlog 如实记录失败与驳回的文化

---

## 3. 实施路线图

### P0 — 立即执行（约 1～2 天，目标是"当前版本可用且可信"）

| 项 | 内容 | 验收 |
|----|------|------|
| 1. 清理/垃圾扫描进度事件 | `clean://progress` 与 `junk://progress`（150ms 节流，携带已删文件数/字节数/当前规则），UI 显示进度与"已完成 x / 共 y" | 清理 5000+ 文件目录时进度持续跳动；取消后进度停止 |
| 2. `protection()` 缓存 | `OnceLock<Protection>` 进程级缓存 | 现有测试全绿；万文件清理耗时可测量下降 |
| 3. SpaceView 状态保持 | `<KeepAlive>` 包裹两个视图（或把 `result` 上移到 App 级 store）；同步调整"切页取消扫描"策略为"仅卸载时取消" | 扫完 C 盘 → 切页签 → 切回，结果树仍在，无需重扫 |
| 4. 关窗保护 | 有任务运行时拦截 `CloseRequested`，弹确认框 | 清理进行中点关闭 → 弹确认 → 取消后任务继续 |
| 5. CI + 前端 lint | GitHub Actions（windows-latest：cargo fmt --check / clippy -D warnings / test / pnpm lint+build）；补 ESLint + Prettier 与 `pnpm lint` script | PR 上门禁自动执行，与 process.md §5 完全一致 |
| 6. README 状态更新 | 状态改为"开发中"，补已完成功能与运行方式 | 与实际一致 |

### P1 — v0.1 MVP 收尾（1～2 周）

| 项 | 内容 | 验收 |
|----|------|------|
| 7. 回收站清理 | 专项设计：`SHQueryRecycleBin` 统计 + `SHEmptyRecycleBin`/`IFileOperation` 清空（按盘符可选）；UI 上作为独立卡片而非普通规则；清空前显示各盘回收站体积 | 回收站有条目时显示统计；清空后资源管理器确认；只清选中盘符 |
| 8. 大文件 Top-N | `top_files(clientId, minSize, limit)` 命令，遍历 `ScanIndex` 聚合文件级 Top-100（扫描时顺带记录文件明细，内存换功能，上限截断） | 空间页签新增"大文件"列表，可定位所在目录 |
| 9. 空间可视化 | 引入 ECharts Treemap（懒加载，仅在空间页签使用）；数据源复用 `dir_children` 下钻 | 从盘符下钻到文件；大节点点击下钻不卡顿 |
| 10. 规则扩充 | Chrome/Edge 全 Profile（动态发现 User Data 下 Profile 目录）、Firefox 缓存、Delivery Optimization、DirectX Shader Cache、Defender 残留、Windows.old 存在性检测 | 真机扫描新增类别有数据/有明确跳过原因 |
| 11. 风险确认强度 | medium 规则勾选时确认弹窗黄色提示；high 规则（未来）要求输入"删除"确认 | 需求 §3.1"危险操作必须显式确认"逐级落地 |
| 12. 单实例 | `tauri-plugin-single-instance`，二次启动唤起已有窗口 | 双击两次 exe 只有一个进程 |
| 13. 本地日志 | `tracing` + `tracing-appender` 按天滚动，写入 `%LOCALAPPDATA%\Purimaster\logs`；panic hook 落盘；设置页留"打开日志目录" | 安装版可复现问题定位；无网络上报 |
| 14. 错误码化（i18n 前置） | 三个 crate 的错误改为 `code + 参数`（serde 可序列化），Display 保留中文兜底；前端建立错误文案表 | IPC 错误载荷含稳定 code；中文界面行为不变 |
| 15. 安装链路验证 | `tauri build` 走通 NSIS；真机安装/卸载各一次：规则加载、体积 < 10 MB、空闲内存 < 150 MB、卸载无残留 | 结果记入 devlog，未达标项开 fix |

### P2 — v0.2 打磨（2～4 周）

| 项 | 内容 |
|----|------|
| 16. 重复文件查找 | 按需求 §3.3 三级流水（大小预筛 → 首尾 4KB → blake3 全量），走 ADR 复用删除框架 |
| 17. 扫描内存优化 | 遍历直传 `parent_id` 消除 `path_ids` 大表；有界 channel 背压；复测空闲与低内存两种环境基准 |
| 18. 垃圾扫描并行化 | per-target rayon 并行（保持组内统计确定性） |
| 19. 文件级勾选 | 规则详情内支持勾选/排除具体文件，确认数字与实际删除严格一致 |
| 20. 前端测试基建 | vitest + @vue/test-utils；先覆盖 `format.ts`、勾选/确认状态机 |
| 21. IPC 契约生成 | 评估 tauri-specta 自动生成 TS 类型，消除手工镜像 |
| 22. 规则信任 ADR | 规则文件签名或哈希清单机制设计（社区规则前置条件） |
| 23. 回收站配额精确预检 | 读 `BitBucket\Volume` 注册表卷级 MaxCapacity，替代 5% 估算 |
| 24. UI 组件化 | 提取确认弹窗/报告弹窗/任务卡片公共组件（或引入 Naive UI，走 ADR）；提取 `useTask` composable；产品化 IPC 自检（移入设置/诊断页） |

### 明确不做（维持 Non-Goals）

注册表"清理/优化"、内存/开机"加速"、广告推广位、跨平台——与需求文档 §7 一致，任何改进不得触碰。

---

## 5. 模拟用户测试结果与处置（2026-11-05 增补）

P0 实施后跑了一轮六视角模拟用户测试（动态工作流：小白/安全/进阶/契约审计四路代码走查 +
真机引擎运行 + 逐条独立复核 + 汇总审读），41 条发现，其中 3 条经独立复核确认为真问题
（完整报告见当日工作流产物卡片）。处置如下：

**已修复（本轮）**：

| 发现 | 处置 |
|------|------|
| F-23（verified）裸盘符 `C:` 按盘符相对路径语义静默扫错目录 | `start_scan` 归一化：`C:` → `C:\`；`C:foo` 形式直接拒绝并提示写全路径；前端同步归一化 |
| F-05 永久删除缺需求文档要求的二次确认 | 确认弹窗在永久删除（非预演）时要求键入「删除」解锁按钮 |
| F-10 风险等级未落地确认强度 | 确认弹窗列表显示风险徽章；含中/高风险时显示黄色警示行（与 F-05 同一机制，P1-11 部分提前落地） |
| F-11 规则注释承诺"只扫描不删除"但引擎无此机制 | 规则引擎新增 `scan_only` 属性（扫描正常统计、清理跳过、仍参与归属去重）；Windows 更新缓存规则启用 |
| F-15 确认退出会丢已删除部分的逐文件报告 | 关窗弹窗在含清理任务时明示"已删除的文件将没有报告" |
| F-16/F-37 `close_denied` 跨任务残留 | 任务表清空时复位保护状态；前端"继续等待"改为先复位成功再关弹窗，失败保持弹窗 |
| F-35 性急双击 X 在确认框弹出前落穿退出 | 关窗保护加 1.5 秒武装窗口，期间重复关窗忽略 |
| F-22/F-39 清理无单飞 | `clean_junk` 单飞（拒绝式：删除不可中断，不取消运行中的清理） |
| F-31 空间扫描后端无单飞 | `start_scan` 单飞（取消式，与垃圾扫描同策略）；前端 startScan 防重入 |
| F-40 终态事件丢失时 UI 永久卡"进行中"（部分） | `cancel_scan` 对不存在的任务返回错误，前端取消失败可见（超时兜底留待后续） |
| F-01 IPC 自检芯片常驻产品 UI | 仅在异常/检测中显示，正常时隐藏；文案改用户语言 |
| F-02 首屏空白的默认页签 | 默认页签改为垃圾清理（打开即扫） |
| F-03/F-33 取消无提示、进度无时长 | 两视图取消后显示"扫描已取消"；空间扫描进度显示已用时长 |
| F-04/F-13/F-20 "试运行"措辞、入口不可见、全选绕过分级 | 改"先预演（只检查，不删除任何文件）"；清单提示文案补充全选与预演说明 |
| F-06 清理状态无释放量 | 进度行追加"已/将释放 X"（dryRun 语义区分） |
| F-07 主视觉大数字无标签 | 加"已勾选可清理"标签 |
| F-08 报告无"打开回收站"入口 | 回收站模式的报告新增"打开回收站"按钮（`open_recycle_bin` 命令） |
| F-14 dry-run 下预检拒绝被误报为删除失败 | 报告文案按 dryRun 区分"未通过安全检查（未删除）"与"删除失败" |
| F-21 失败明细截断无提示 | `failedCount > failed.length` 时显示"另有 N 条未展示" |
| F-26 截断内容 UI 永不可达（子目录侧） | DirTree 截断提示加"显示更多"分段放开按钮 |
| F-27 unreadable 不向祖先传播 | 聚合新增 `unknownSubtree`（祖先链传播 + 单测），父行显示"含未知"角标 |
| F-28 联接点静默吞掉 | 扫描显式计数 junctions，汇总行提示"另有 N 个链接/联接点未计入" |
| F-29 文件清单与快照可能矛盾 | 文件清单处注明"实时读取，可能与扫描快照不同步" |
| F-32/F-36 注释与实际机制错位 | 修正 `dir_children` 与 `take_clean_report` 注释，写明双机制分工 |
| F-41 契约无防漂移手段（部分） | 新增 progress/report 的 serde 字段名快照测试（完整方案仍归 P2-21 tauri-specta） |
| （真机补测）WER 归档子目录成片"无法读取"刷屏 | 扫描 problems 明细截断至 10 条 + 汇总行 |

**真机运行补测**（工作流真机视角超时被裁剪，由主会话补跑只读示例）：规则扫描实测
14,358 文件 / 10.1 GB（Temp 7.2 GB、更新缓存 1.9 GB、Edge 缓存 982 MB）；全盘 C: 负载态
894 秒 / 149.6 万文件 / 342 GB / 跳过 132（空闲基线 20 秒，受限于同机并发测试）。实测还
发现 Temp 中 2.2 GB 的活跃 swap.vhdx——被占用文件清理时将按设计计入失败明细，属预期管理
问题，随 P1 轮在规则描述中说明。

**登记待办（未实施）**：

- F-24（verified）大文件查找、F-25（verified）Treemap —— 即 P1-8 / P1-9，独立专项
- F-12 与 F-05 合并处置；P1-11 的 high 规则键入确认词已在本次对 permanent 落地，
  规则级 high 风险确认随规则扩充补齐
- F-17/F-18 规则信任模型 —— 即 R4/P2-22，先立 ADR
- F-19 清理时间窗差额说明、F-30 逻辑大小 vs 磁盘占用、F-40 前端超时兜底 —— 低优先打磨
- F-41 的完整契约防漂移 —— P2-21

---

## 6. 对标开源清理软件的规则引擎升级（2026-11-05 增补）

调研 [BleachBit CleanerML](https://docs.bleachbit.org/cml/cleanerml)、
[winapp2.ini](https://github.com/MoscaDotTo/Winapp2)（2400+ 条社区规则的格式规范）、
[CCleaner 官方 24 小时启发式](https://support.ccleaner.com/s/article/change-advanced-settings-in-ccleaner-for-windows)
与 [Czkawka](https://github.com/qarmin/czkawka) 后，提炼三项设计落地：

1. **`min_age` 文件年龄过滤**（学 BleachBit 的 `age="30d"` 属性 + CCleaner
   "Windows Temp 只清理 24 小时前的文件"默认开启）：`<数字><h|d|w>`，扫描与清理一致生效；
   修改时间晚于阈值或时间戳在未来的文件跳过。**正在使用的临时/缓存文件几乎总是新写的**，
   这是各家清理软件一致的安全底线。Temp/浏览器缓存规则全部配 `min_age = "24h"`。
   被跳过的近期文件单独计数（`freshFiles`），UI 显示"另有 N 个近期文件未计入（可能正在
   使用）"——检测透明化（学 Czkawka：让用户知道算法为什么没选中某个文件）。
2. **`paths` 通配符展开**（学 winapp2 FileKey 的多 Profile 枚举与 CleanerML 的 search=glob）：
   含 `*`/`?` 的路径按当前文件系统展开为现存目录，`*` 不跨分隔符；未安装应用的 Profile
   目录自然消失，与"目标不存在即静默跳过"语义一致。展开目标逐个过保护检查。
   Chrome/Edge 缓存从写死 Default Profile 升级为覆盖全部 Profile，新增 Firefox cache2 规则。
3. **`description` 规则说明**（学 winapp2 每条规则的注释文化 + CleanerML
   "模式尽量具体"的安全指引）：面向用户的一句话说明（这是什么、为什么可清理、
   有什么副作用），展示在清理详情里，替代"专业名词 + 用户自己猜"。

规则数量 8 → 9（新增 Firefox）；真机验证：24h 启发式让用户 Temp 从 10,738 降到 7,917
个文件（近期文件安全跳过），Edge 缓存从 982 MB 降到 474 MB，Chrome 通配展开扫到了
此前为 0 的多 Profile 缓存。测试 66 个全绿（rules 新增 4 个：min_age 解析/行为、
通配展开、description 透传）。

---

## 7. 规则数据对标扩充（2026-11-05 增补，9 → 18 条）

读 BleachBit cleaners 源码（discord.xml/vscode.xml 等）与系统缓存路径文献后扩充规则数据：

- **app-cache 分类**（学 BleachBit 的 Electron 应用条目）：VS Code / Discord / Slack，
  只清 Chromium 三件套（Cache/Code Cache/GPUCache），不碰设置数据
- **dev-cache 分类**：npm / pip / Yarn v1——只清各包管理器**官方声明可清**的 cache 目录；
  刻意排除 `.nuget\packages` 等实体包目录（破坏离线还原）
- **system-residue 增补**：DirectX 着色器缓存（`D3DSCache`，`min_age = "7d"`——清了安全
  但代价是着色器重编译卡顿，来源：wisecleaner/squirreldisk）、Windows 系统网络缓存
  （INetCache）
- **crash-dump 增补**：蓝屏小转储 `%SystemRoot%\Minidump`（medium/7d，正常系统不存在→
  静默跳过）
- **不做清单**（写入 junk.toml 头部，含依据）：Prefetch（[微软官方说明删除反而变慢](
  https://www.windowslatest.com/2026/10/26/microsoft-explains-why-deleting-prefetch-can-slow-down-windows-11-as-its-designed-to-speed-up-app-launches)）、
  浏览器登录态、Delivery Optimization 缓存（服务占用+需管理员，归磁盘清理工具，
  来源：[Microsoft Learn](https://learn.microsoft.com/en-us/windows/deployment/do/waas-delivery-optimization-reference)）、
  实体包目录、LiveKernelReports（标准用户无权限，扫描纯噪音）

真机验证：18 条全部加载；新增规则命中 npm 缓存 258.86 MB、pip 缓存 140.53 MB、
VS Code 缓存 60.41 MB、着色器缓存 7.97 MB（此前均为扫描盲区）；合计 10.09 GB。

---

## 4. 风险提示

1. **P0 第 3 项（KeepAlive）** 会改变"切页取消"语义：SpaceView 不再卸载，原"切走即取消扫描"逻辑需重新审视（保留后台扫描 + 状态保持，更符合直觉，但要确认扫描中切页签不再堆积任务——后端单飞已有，空间扫描建议同策略）。
2. **回收站清理（P1-7）是新的删除面**：必须复用 `crates/platform` 删除框架并新增 ADR（回收站清空不可恢复、无回收站兜底，确认强度应为全部操作中最高）。
3. **错误码化（P1-14）触及三个 crate 的公共 API**：宜在 P1 早期一次做完，避免和规则扩充交叉返工。
