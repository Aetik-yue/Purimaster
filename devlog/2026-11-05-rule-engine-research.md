# 开发日志 2026-11-05（三）：对标开源清理软件的规则引擎升级

## 今日目标

- 调研优秀开源清理软件（BleachBit / winapp2.ini / CCleaner / Czkawka）的筛选算法与 UI 设计
- 把学到的思路落地到 Purimaster

## 调研结论（来源见 improvement-plan §6 链接）

1. **BleachBit CleanerML**：`<delete age="30d">` 文件年龄属性；"模式尽量具体"的安全指引；
   "能截断就不删除"
2. **winapp2.ini**（2400+ 社区规则）：Detect（检测到应用才激活）/ FileKey（动作）/
   ExcludeKey（安全排除）三段式声明；每条规则带注释文化
3. **CCleaner**：官方默认开启"Windows Temp 只清理 24 小时前的文件"——正在使用的
   临时文件几乎总是新写的，这是行业共识的安全启发式
4. **Czkawka**：检测透明（用户能知道为什么某文件没被选中）；UI 演进到 Krokiet 强调
   性能与直观

## 落地实现

- **规则引擎新增 `min_age`**（`24h`/`7d`/`2w`）：扫描与清理一致跳过近期文件与
  未来时间戳文件；被跳过文件单独计数 `freshFiles` 透传到 UI
- **规则引擎新增 `description`**：面向用户的一句话说明，详情区展示
- **`paths` 支持通配符**：按现存目录展开（`*` 不跨分隔符），展开目标逐个过保护检查；
  单规则展开上限 256（防病态 glob）
- **junk.toml 全面升级**：Temp/浏览器缓存配 `min_age = "24h"`；Chrome/Edge 改为
  `User Data\*\Cache` 多 Profile 通配；新增 Firefox cache2 规则（8→9 条）；
  每条规则写 description（这个目录是什么、为什么可清、有什么副作用）
- **UI 透明化**：规则行显示"另有 N 个近期文件未计入（可能正在使用）"；
  详情首行显示 description
- 开发约束：新依赖 `glob`（本地缓存有）可离线获取；`filetime` 缓存缺失后放弃，
  测试夹具改用 SetFileTime（dev-dependency windows-sys 0.59 本地已缓存）

## 遇到的问题与解决

1. **代理断连**：filetime/glob 拉取失败（proxy 127.0.0.1 拒绝）——查本地 cargo 缓存，
   保留 glob（有缓存）、改写夹具避开 filetime（无缓存）
2. **测试夹具 SetFileTime 静默失败**（mtime 只差 1.8ms）：三步定位——先加"夹具自检"
   断言抓到静默失败；确认根因是只读句柄无 FILE_WRITE_ATTRIBUTES 访问权；
   再修正 `share_mode(FILE_WRITE_ATTRIBUTES)` 的误用（那是访问权不是共享模式），
   最终用 `access_mode(GENERIC_WRITE | FILE_WRITE_ATTRIBUTES)` + custom_flags；
   SetFileTime 返回值显式检查 panic 带 GetLastError
3. **clippy 两连**：`ok_or_else(|| invalid())` 冗余闭包；collect_files 参数 9/7 超限——
   三个旁路计数收拢为 `CollectStats` 结构体
4. PowerShell 的 `2>$null` 重定向会污染退出码与中文输出——统一用 cmd 或
   `powershell -Command` 内部捕获

## 真机验证（只读示例，release）

- 用户临时目录：10,738 → **7,917 文件 / 6.87 GB**（24h 内 ~2,800 个文件安全跳过，
  含正在使用的 swap.vhdx）
- Edge 缓存：982 → **474 MB**（近期缓存不计入）；Chrome 通配展开扫到 7,915+
  文件的 Profile 缓存（此前 Default-only 为 0）
- 合计 9,318 MB / 9,731 文件；WER 报错仍受 PROBLEM_PREVIEW 截断控制

## 门禁结果

`cargo fmt --check` ✓ ｜ `cargo clippy -D warnings` ✓ ｜ `cargo test --workspace`
**66 通过**（core 12 / platform 19 / rules 35）✓ ｜ `pnpm lint` ✓ ｜ `pnpm build` ✓

## 待办

- [ ] 用户真机冒烟：确认弹窗、近期文件提示行、description 详情展示
- [ ] P1-8 大文件查找 / P1-9 Treemap（下一个功能专项）
- [ ] 空闲时复测全盘基准；NSIS 安装链路验证
