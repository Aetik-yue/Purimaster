# 开发日志 2026-11-05：改进方案 P0 批次落地

## 今日目标

- 执行 [改进方案](../docs/improvement-plan.md) 的 P0 批次（6 项）

## 完成情况

- ✅ **P0-1 清理/垃圾扫描进度事件**：`crates/rules` 的 `scan`/`clean` 增加 `progress` 回调参数
  （新增 `JunkScanProgress`/`CleanProgress` 结构，规则边界 + 逐文件回调）；壳层 150ms 节流后发
  `junk://progress` / `clean://progress`；JunkView 显示"正在扫描「浏览器缓存」（3/12）· 已发现
  N 个文件"与清理逐条推进。**大清理期间 UI 不再是黑盒**
- ✅ **P0-2 `protection()` 缓存**：`OnceLock` 进程级缓存。修复"每文件两次 `check_target`、每次
  重建保护列表（6 次 Known Folder API + 十几个环境变量读取）"——万文件清理约 2 万次重复构建
- ✅ **P0-3 SpaceView 状态保持**：App.vue 页签容器改 `<KeepAlive>`。扫完 C 盘切页签再切回，
  结果树保留；语义变化：空间扫描切页不再取消（后台继续，监听仍存活），两个视图的
  `onUnmounted` 仅在应用退出时触发
- ✅ **P0-4 关窗保护**：`CloseRequested` 时若有任务运行 → `prevent_close` + 发
  `app://close-busy`（带任务数）→ 前端确认框（继续等待 / 仍要退出）；**同一窗口第二次关闭
  请求直接放行**（覆盖 WebView 无响应收不到事件的场景）；新命令 `allow_close`（复位保护）、
  `quit_app`（取消全部任务后 `app.exit(0)`）
- ✅ **P0-5 CI + 前端 lint**：`.github/workflows/ci.yml`（windows-latest：fmt/clippy -D
  warnings/test；ubuntu-latest：pnpm lint/build）；ESLint 10 + eslint-plugin-vue 10 +
  typescript-eslint 8 + Prettier，`pnpm lint` / `lint:fix` / `format` script——
  process.md §5 门禁首次全部可自动执行
- ✅ **P0-6 README 更新**：状态"规划阶段"→"开发中"，补本地运行与质量门禁说明、改进方案导航

## 遇到的问题与解决

1. ESLint 首跑报 `no-undef`（console/crypto/setTimeout）：flat config 默认无浏览器全局，
   引入 `globals` 包声明 `globals.browser` 后全绿
2. eslint-plugin-vue v10 的 flat 配置同时暴露 `recommended` 与 `flat/recommended` 两套键名，
   用后者（v9/v10 兼容写法）
3. 新增进度回调参数使 `rules` 的 12 处测试调用点与示例 `junk_scan.rs` 需同步更新
   （统一补 `&mut |_| {}`），属预期的 API 破坏性变更（0.x 阶段允许）

## 决策记录

- 进度节流职责划分：**引擎逐文件回调、壳层节流**——crates 保持纯逻辑可独立测试（新增
  `scan_progress_*`/`clean_progress_*` 三个单测），节流属于 IPC 策略留在壳层；约定已写入
  architecture.md §4
- 关窗保护采用"二次请求放行"而非超时放行：无定时器、逻辑简单，且天然覆盖 WebView 崩溃
  后弹框无人应答的死锁风险
- KeepAlive 引入后未改后端：空间扫描后台继续符合直觉，垃圾扫描本就切页不取消；
  后端空间扫描无单飞，但 UI 上扫描入口唯一（按钮态互斥），无堆积路径——记录为已知取舍

## 门禁结果

`cargo fmt --check` ✓ ｜ `cargo clippy --workspace --all-targets -D warnings` ✓ ｜
`cargo test --workspace` **58 通过**（core 10 / platform 19 / rules 29，含 3 个新进度测试）✓ ｜
`pnpm lint` ✓ ｜ `pnpm build`（vue-tsc + vite）✓

## 待办（移交后续）

- [ ] 用户冒烟测试：清理进度条、关窗确认框、切页保持（三项均为交互行为，需真机验证）
- [ ] P1 批次：回收站清理（需新 ADR）、大文件 Top-N、Treemap、规则扩充、风险确认分级、
  单实例、本地日志、错误码化、安装链路验证（详见 improvement-plan.md §3）
- [ ] P1-15 之前补一次 `tauri build` 的 NSIS 产物真机验证
