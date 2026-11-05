# Purimaster 设计系统

版式驱动的桌面工具审美：暖纸灰中性阶 × 单一深松绿强调 × 4px 基准网格。
Tokens 定义于 `src/style.css` 的 `:root`，换肤只改这一处；各视图 scoped 样式通过既有变量名（`--bg / --surface / --border / --text / --text-secondary / --accent / --accent-hover / --danger / --radius`）零改动继承。

## 一、设计立场

- **克制**：单色系 + 单强调色。彩色只出现在四种语义上——强调（松绿）、危险（红）、成功（绿，存量）、警示（琥珀，存量）。
- **版式先于装饰**：层级靠字号对比、字重、留白与 1px 细线建立，不靠卡片堆砌与投影。
- **几何构成**：品牌标记为 10px 墨绿方点（`App.vue` 中 `h1::before`），无图形 logo、无 emoji 图标。
- **数据是主角**：所有数字等宽对齐（全局 `tabular-nums`），大数字与说明文字形成强层级对比。

## 二、色彩 tokens

| Token | 值 | 用途 | 对比度（已核验） |
| --- | --- | --- | --- |
| `--bg` | `#f3f1ec` | 应用底色 | — |
| `--surface` | `#fbfaf7` | 浮层 / 面板 / 列表容器 | — |
| `--border` | `#ddd9d0` | 1px 分隔线、描边 | — |
| `--border-strong` | `#c8c3b7` | hover 描边、滚动条 | — |
| `--text` | `#1f1e1b` | 主文本 | bg 上 14.8:1 |
| `--text-secondary` | `#6b675e` | 说明文本 | bg 5.0:1 / surface 5.4:1 |
| `--text-tertiary` | `#9b968b` | placeholder，仅装饰性 | — |
| `--accent` | `#14655a` 深松绿 | 主操作 / 进行中 / 选中 | 白字 6.9:1，surface 6.6:1 |
| `--accent-hover` | `#10493f` | 强调 hover | — |
| `--accent-press` | `#0c3831` | 强调按下 | — |
| `--accent-soft` | `#e8eeea` | 行 hover / 选中淡底（8% 松绿） | — |
| `--danger` | `#b3362b` | 危险操作、错误文本 | 白字 6.0:1，surface 5.8:1 |
| `--danger-hover` | `#962c23` | 危险 hover | — |
| `--danger-soft` | `#f6e3e0` | 危险淡底 | — |
| `--success` | `#1a7f47` | 成功（存量记录，surface 4.8:1） | — |
| `--warning` | `#b45309` | 警示琥珀（存量记录，surface 4.8:1） | — |

选型说明：深松绿偏墨青，与视图存量警示琥珀 `#b45309`、成功绿 `#1a7f47` 色相分离清晰；赭橙系因撞警示琥珀被否决；默认蓝/紫被立场排除。中性阶为暖纸灰而非冷蓝灰，规避"AI 生成感"。

## 三、字号 / 间距 / 圆角 / 阴影 / 动效

**字号**（12/16/20/28 落 4px 网格；13/14 为存量正文尺寸，保留兼容）：

| Token | 值 | 用途 |
| --- | --- | --- |
| `--font-xs` | 12px | 辅助说明、badge |
| `--font-sm` | 13px | 次级正文 |
| `--font-md` | 14px | 正文基准（body 默认） |
| `--font-lg` | 16px | 弹窗标题 |
| `--font-xl` | 20px | 页面标题 |
| `--font-display` | 28px | 展示级大数字（视图内 34px 英雄数字同级） |

**间距**（4px 网格）：`--space-1/2/3/4/5/6/8/10/12` = 4/8/12/16/20/24/32/40/48px。凡新写间距取此表，不出现 7px、15px 之类离格值。

**圆角**：`--radius-xs` 2px（细进度条）、`--radius-sm` 4px（badge）、`--radius` 8px（默认：按钮/输入框/容器）、`--radius-lg` 12px（弹窗/英雄区）。上限 12px，禁止 16px+ 的气球圆角。

**阴影**（单色近黑，禁止彩色投影）：`--shadow-1`（静置）、`--shadow-2`（悬浮条）、`--shadow-3`（弹窗）、`--scrim` 遮罩 `rgba(31,30,27,0.44)`。阴影只用于真浮层（弹窗、底部选择栏），列表容器只用 1px 边线。

**动效**：`--dur-fast` 0.15s（hover/按下）、`--dur` 0.2s（显隐/位移）、`--ease cubic-bezier(0.2,0,0,1)`。所有过渡落在这个区间，不做弹跳与长动画。

## 四、版式规则

1. **强层级对比**：同一屏允许的最大字号差即层级差——英雄数字（28–34px / 700）vs 说明文字（12px / 400）差近 3 倍；中间级别（20/16px）只用于标题，不作正文放大。
2. **充足留白**：区块间距 ≥ 12px；英雄区、弹窗内边距 20–24px。宁可留白，不加框。
3. **数字一律 `tabular-nums`**：已在 `body` 全局声明，任何文件数/字节数/百分比纵向对齐，视图无需逐处再写。
4. **分隔靠留白与 1px 细线**：`--border` 细线（实线分区、虚线预览区）优先于新卡片；新增面板必须先回答"为什么不是一个分隔线"。
5. **hover / focus 统一**：hover = `--accent-soft` 淡底或 `--border-strong` 描边；键盘焦点 = 2px `--accent` outline（`:focus-visible` 全局）；输入框焦点 = 描边变松绿 + 3px `--accent-soft` 软环。同类控件不允许各自发明 hover 色。
6. **过渡 0.15–0.2s**：只过渡 `color / background-color / border-color / box-shadow`，不做位移弹跳。
7. **红色仅用于危险**：删除、退出、失败、错误提示。进度、选中、提示一律松绿或中性，禁止用红色制造紧迫感。
8. **进度条**：4px 细条（`--radius-xs`），填充为松绿，不做粗彩条。

## 五、明确禁止（去 AI 味清单）

- 渐变横幅、渐变按钮、渐变背景（任何 `linear-gradient` 装饰性使用）。
- emoji 作图标（模板中的 `⚠`/`✓` 为存量文本符号，仅用于警示/成功语义，不新增）。
- 彩色大阴影（`box-shadow` 带色相）、多层发光。
- 过大圆角（>12px）与胶囊堆叠。
- 紫色系、默认蓝（`#3b82f6` 类）、蓝紫渐变审美。

## 六、兼容性注记（存量硬编码，已统一）

上一版注记所列的存量硬编码在当前代码中已全部替换为 tokens（2026-11 复核：`git grep` 全量核验 src/ 无 `rgba(47,111,237` / `#1a9e55` / `#b91c1c` / `#fef3c7` 代码残留，仅 DirTree.vue:270 注释提及旧蓝；`#1a7f47` / `#b45309` 仅存于 `:root` token 定义本身）。原表条目移入替换记录，避免按旧表追查幽灵硬编码：

| 原存量（已不存在） | 统一为 |
| --- | --- |
| 旧蓝行 hover/选中底 `rgba(47,111,237,…)`（原 JunkView / UninstallView / DirTree 行 hover） | `var(--accent-soft)` |
| `.empty` 成功绿 `#1a9e55`（原 JunkView） | `var(--success)` |
| badge 色 `#1a7f47` / `#b45309` / `#fef3c7`（原 JunkView / DirTree / ReportModal） | `var(--success)` / `var(--warning)` 及对应语义 |
| `.warn` / `.fail` / `.fail-err` 红字 `#b91c1c` | `var(--danger)` |
| 冷灰遮罩与投影 `rgba(15,23,42,…)`（`.overlay` / `.modal`） | `var(--scrim)` / `var(--shadow-3)` |
| 模态/英雄区离格 10px 圆角 | `var(--radius-lg)`（12px）等 radius tokens |

后续若再发现存量硬编码，按上表「统一为 tokens」原则登记于此。

## 七、换肤方法

改 `src/style.css` 的 `:root` 即整体换肤：九个契约变量名不可删改；新增 token 只能加不能改语义。浅色为唯一主题（桌面清理工具无深色需求，不做半成品深色模式）。
