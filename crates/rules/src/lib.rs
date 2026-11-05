//! purimaster-rules：清理规则引擎。
//!
//! 规则即数据（见 docs/decisions/0002-rules-as-data.md）：
//! - 规则定义在 `rules/*.toml`，加载时**严格校验**（schema、重复 id、未知分类、glob 语法）
//! - 目标路径支持 `%ENV%` 环境变量展开（Windows 约定）
//! - include/exclude 为相对目标目录的 glob（`/` 分隔、大小写不敏感）
//! - 目标必须通过 [`purimaster_platform::check_target`] 的系统关键路径保护检查
//!
//! 本 crate 只负责**扫描**垃圾文件（只读）。任何删除行为不在这里实现。

use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glob::MatchOptions;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

/// 删除方式（转发自 platform，便于壳层与测试直接使用）
pub use purimaster_platform::DeleteMode;

/// 每条规则最多预置多少条最大文件预览（避免 IPC 载荷过大）。
const PREVIEW_LIMIT: usize = 50;
/// 每条规则最多保留多少条问题明细（同类权限问题可能成片出现）。
const PROBLEM_PREVIEW: usize = 10;
/// 单个通配路径最多展开多少个目标（防病态 glob）。
const MAX_GLOB_TARGETS: usize = 256;

// ---------------------------------------------------------------------------
// 错误与取消
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RuleError {
    Io(std::io::Error),
    Toml(toml::de::Error),
    Validation(String),
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleError::Io(e) => write!(f, "读取规则文件失败：{e}"),
            RuleError::Toml(e) => write!(f, "规则文件语法错误：{e}"),
            RuleError::Validation(m) => write!(f, "规则校验失败：{m}"),
        }
    }
}

impl std::error::Error for RuleError {}

impl From<std::io::Error> for RuleError {
    fn from(e: std::io::Error) -> Self {
        RuleError::Io(e)
    }
}

impl From<toml::de::Error> for RuleError {
    fn from(e: toml::de::Error) -> Self {
        RuleError::Toml(e)
    }
}

/// 扫描被取消。
#[derive(Debug, Clone, Copy)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "扫描已取消")
    }
}

impl std::error::Error for Cancelled {}

// ---------------------------------------------------------------------------
// 规则定义（TOML schema）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    categories: BTreeMap<String, String>,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDef {
    id: String,
    category: String,
    title: String,
    #[serde(default)]
    risk: Risk,
    paths: Vec<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    /// 仅扫描不清理：规则目标有条件限制（如更新服务运行中）时，
    /// 规则作者可用此属性声明"扫描展示，但清理阶段跳过"。
    #[serde(default)]
    scan_only: bool,
    /// 面向用户的一句话说明：这是什么、为什么可清理（UI 详情展示）。
    #[serde(default)]
    description: String,
    /// 文件年龄下限（如 "24h"、"7d"）：修改时间晚于该阈值的文件跳过不计。
    /// 行业标准安全启发式——正在使用的临时文件几乎总是新写的
    /// （CCleaner 对 Windows Temp 默认只清理 24 小时前的文件）。
    #[serde(default)]
    min_age: Option<String>,
}

/// 清理风险等级：UI 据此决定确认强度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    #[default]
    Low,
    Medium,
    High,
}

// ---------------------------------------------------------------------------
// 进度上报（壳层负责节流，引擎逐文件回调）
// ---------------------------------------------------------------------------

/// 垃圾扫描进度快照（按规则推进，逐文件累计）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkScanProgress {
    /// 当前规则下标（0 起）
    pub rule_index: u32,
    pub rule_count: u32,
    /// 当前规则标题
    pub current_title: String,
    /// 累计已发现的文件数
    pub files: u64,
    /// 累计已发现的字节数
    pub bytes: u64,
}

/// 清理进度快照（删除时重新枚举，无法预知总量，只报已完成部分）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanProgress {
    /// 已删除（或 dry-run 下"将删除"）的文件数
    pub deleted_files: u64,
    pub freed_bytes: u64,
    pub failed_count: u64,
    /// 当前处理的规则标题
    pub current_title: String,
}

// ---------------------------------------------------------------------------
// 编译后的规则
// ---------------------------------------------------------------------------

/// 单个目标目录（环境变量已展开、已通过保护检查）。
#[derive(Debug, Clone)]
pub struct CompiledTarget {
    pub path: PathBuf,
    /// 非空表示该目标不可用（未展开的环境变量 / 命中保护路径等）。
    pub problem: Option<String>,
}

/// 编译后的规则。
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub category: String,
    pub title: String,
    pub risk: Risk,
    /// 仅扫描：scan 阶段正常统计，clean 阶段跳过删除。
    pub scan_only: bool,
    /// 面向用户的一句话说明（UI 详情展示）。
    pub description: String,
    /// 文件年龄下限：修改时间晚于该阈值的文件在扫描与清理中一致跳过。
    pub min_age: Option<Duration>,
    pub targets: Vec<CompiledTarget>,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

/// 规则集合（可来自多个 TOML 文件）。
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    categories: BTreeMap<String, String>,
    rules: Vec<Rule>,
}

impl RuleSet {
    /// 从单个 TOML 文本构建。
    pub fn from_toml_str(source: &str) -> Result<Self, RuleError> {
        Self::from_toml_strs(&[source])
    }

    /// 从多个 TOML 文本合并构建（使用当前系统的保护路径）。
    pub fn from_toml_strs(sources: &[&str]) -> Result<Self, RuleError> {
        Self::from_toml_strs_with_protection(sources, &purimaster_platform::protection())
    }

    /// 从多个 TOML 文本合并构建，指定保护配置（便于测试/沙箱）。
    pub fn from_toml_strs_with_protection(
        sources: &[&str],
        protection: &purimaster_platform::Protection,
    ) -> Result<Self, RuleError> {
        let mut categories: BTreeMap<String, String> = BTreeMap::new();
        let mut defs: Vec<RuleDef> = Vec::new();

        for source in sources {
            let file: RuleFile = toml::from_str(source)?;
            for (id, title) in file.categories {
                match categories.get(&id) {
                    Some(existing) if existing != &title => {
                        return Err(RuleError::Validation(format!(
                            "分类 `{id}` 定义冲突：`{existing}` vs `{title}`"
                        )));
                    }
                    _ => {
                        categories.insert(id, title);
                    }
                }
            }
            defs.extend(file.rules);
        }

        let mut seen = HashSet::new();
        let mut rules = Vec::with_capacity(defs.len());
        for def in defs {
            let id = def.id.trim().to_string();
            if id.is_empty() {
                return Err(RuleError::Validation("规则 id 不能为空".into()));
            }
            if def.title.trim().is_empty() {
                return Err(RuleError::Validation(format!(
                    "规则 `{id}` 的 title 不能为空"
                )));
            }
            if def.paths.is_empty() {
                return Err(RuleError::Validation(format!(
                    "规则 `{id}` 至少要有一个 path"
                )));
            }
            if !categories.contains_key(&def.category) {
                return Err(RuleError::Validation(format!(
                    "规则 `{id}` 引用了未定义的分类 `{}`",
                    def.category
                )));
            }
            if !seen.insert(id.clone()) {
                return Err(RuleError::Validation(format!("规则 id 重复：`{id}`")));
            }

            let include = compile_globs(&def.include)
                .map_err(|e| RuleError::Validation(format!("规则 `{id}` 的 include 非法：{e}")))?;
            let exclude = compile_globs(&def.exclude)
                .map_err(|e| RuleError::Validation(format!("规则 `{id}` 的 exclude 非法：{e}")))?;
            let min_age = match &def.min_age {
                Some(raw) => Some(parse_min_age(raw).map_err(|e| {
                    RuleError::Validation(format!("规则 `{id}` 的 min_age 非法：{e}"))
                })?),
                None => None,
            };

            let targets = {
                // 同一规则内多个路径可能展开为同一目标（如 %TEMP% == %LOCALAPPDATA%\Temp，
                // 或通配符展开出的多个 Profile 目录），按大小写不敏感键去重（Windows 语义）
                let mut seen_paths: HashSet<String> = HashSet::new();
                def.paths
                    .iter()
                    .flat_map(|p| compile_targets(p, protection))
                    .filter(|t| seen_paths.insert(dedup_key(&t.path)))
                    .collect()
            };

            rules.push(Rule {
                id,
                category: def.category,
                title: def.title,
                risk: def.risk,
                scan_only: def.scan_only,
                description: def.description,
                min_age,
                targets,
                include,
                exclude,
            });
        }

        Ok(RuleSet { categories, rules })
    }

    /// 加载目录下全部 `*.toml`（按文件名排序，保证合并顺序确定）。
    pub fn load_dir(dir: &Path) -> Result<Self, RuleError> {
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                files.push(path);
            }
        }
        files.sort();
        let sources: Vec<String> = files
            .iter()
            .map(|p| fs::read_to_string(p).map_err(RuleError::Io))
            .collect::<Result<_, _>>()?;
        let refs: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
        Self::from_toml_strs(&refs)
    }

    pub fn categories(&self) -> &BTreeMap<String, String> {
        &self.categories
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// 构造"定向清理"临时规则集：用户在 UI 里指定的单个目录（可带类型过滤与年龄下限）。
    ///
    /// 与规则文件驱动的清理走**完全相同**的安全管线——目标在构造时过系统关键路径
    /// 保护检查（盘符根、受保护目录当场被拒），删除阶段仍逐文件八步校验（ADR-0003）。
    /// 规则集只存在于内存中的这一次扫描/清理，不写入任何规则文件。
    ///
    /// - `path`：要清理的目录（不存在的目录会通过构造，但扫描时静默为空——
    ///   调用方应先自行校验 `is_dir` 给出友好错误）
    /// - `include`：可选的文件名 glob 过滤（如 `["*.log"]`），空表示全部文件
    /// - `min_age`：可选年龄下限；定向清理是用户显式指定，通常传 `None`
    pub fn targeted(
        path: &Path,
        include: &[String],
        min_age: Option<Duration>,
        protection: &purimaster_platform::Protection,
    ) -> Result<Self, RuleError> {
        let include = compile_globs(include)
            .map_err(|e| RuleError::Validation(format!("文件类型过滤非法：{e}")))?;
        let targets = vec![compile_dir_target(path, protection)];
        Ok(RuleSet {
            categories: BTreeMap::from([("targeted".to_string(), "定向清理".to_string())]),
            rules: vec![Rule {
                id: "targeted".to_string(),
                category: "targeted".to_string(),
                title: "定向清理".to_string(),
                risk: Risk::Medium,
                scan_only: false,
                description: format!("你指定的目录：{}", path.display()),
                min_age,
                targets,
                include,
                exclude: None,
            }],
        })
    }

    /// 扫描所有规则目标，产出按规则分组的垃圾统计。
    ///
    /// - 目标不存在（应用未安装等）时静默跳过
    /// - 同一文件被多条规则命中时只计入**第一条**规则（按路径去重）
    /// - 每条规则最多返回 [`PREVIEW_LIMIT`] 条最大文件预览
    /// - `progress`：每条规则开始与每发现一个文件时回调（调用方负责节流）
    pub fn scan(
        &self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(JunkScanProgress),
    ) -> Result<JunkScanResult, Cancelled> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let mut groups = Vec::with_capacity(self.rules.len());
        let mut total_files: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut skipped: u64 = 0;
        // 跨规则去重：大小写不敏感键（Windows 语义；联接点别名属已知限制）
        let mut seen: HashSet<String> = HashSet::new();
        let rule_count = self.rules.len() as u32;

        for (rule_index, rule) in self.rules.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled);
            }
            let mut files: u64 = 0;
            let mut bytes: u64 = 0;
            let mut used_targets: Vec<String> = Vec::new();
            let mut problems: Vec<String> = Vec::new();
            let mut stats = CollectStats::default();
            // 最小堆保存最大的 PREVIEW_LIMIT 条：Reverse 让堆顶为最小
            let mut top: BinaryHeap<std::cmp::Reverse<(u64, String)>> = BinaryHeap::new();

            progress(JunkScanProgress {
                rule_index: rule_index as u32,
                rule_count,
                current_title: rule.title.clone(),
                files: total_files,
                bytes: total_bytes,
            });

            for target in &rule.targets {
                if let Some(problem) = &target.problem {
                    problems.push(format!("{}：{problem}", target.path.display()));
                    continue;
                }
                match fs::metadata(&target.path) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => {
                        problems.push(format!("{}：目标不是目录", target.path.display()));
                        continue;
                    }
                    // 不存在（如应用未安装）是正常情况，静默跳过
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    // 其它错误（权限、网络共享不可达等）必须让用户看见
                    Err(e) => {
                        problems.push(format!("{}：无法访问目标（{e}）", target.path.display()));
                        skipped += 1;
                        continue;
                    }
                }
                used_targets.push(target.path.display().to_string());

                collect_files(
                    &target.path,
                    rule.include.as_ref(),
                    rule.exclude.as_ref(),
                    rule.min_age,
                    cancel,
                    &mut stats,
                    &mut |path, size| {
                        if !seen.insert(dedup_key(&path)) {
                            return; // 已被其它规则计入
                        }
                        files += 1;
                        bytes += size;
                        top.push(std::cmp::Reverse((size, path.display().to_string())));
                        if top.len() > PREVIEW_LIMIT {
                            top.pop();
                        }
                        progress(JunkScanProgress {
                            rule_index: rule_index as u32,
                            rule_count,
                            current_title: rule.title.clone(),
                            files: total_files + files,
                            bytes: total_bytes + bytes,
                        });
                    },
                )?;
            }

            total_files += files;
            total_bytes += bytes;
            skipped += stats.skipped;
            problems.append(&mut stats.problems);
            let fresh = stats.fresh;

            // 问题明细截断：同类权限问题可能成片出现（如 WER 归档子目录全部
            // 拒绝访问），全量展示会淹没 UI——保留前若干条并给出汇总行
            if problems.len() > PROBLEM_PREVIEW {
                let hidden = problems.len() - PROBLEM_PREVIEW;
                problems.truncate(PROBLEM_PREVIEW);
                problems.push(format!("…另有 {hidden} 条同类问题未展示（多为无权限读取）"));
            }

            let mut top_files: Vec<JunkFile> = top
                .into_iter()
                .map(|std::cmp::Reverse((size, path))| JunkFile { path, size })
                .collect();
            top_files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));

            groups.push(JunkGroup {
                rule_id: rule.id.clone(),
                category: rule.category.clone(),
                title: rule.title.clone(),
                risk: rule.risk,
                scan_only: rule.scan_only,
                description: rule.description.clone(),
                fresh_files: fresh,
                files,
                bytes,
                targets: used_targets,
                top_files,
                problems,
            });
        }

        Ok(JunkScanResult {
            categories: self.categories.clone(),
            groups,
            total_files,
            total_bytes,
            skipped,
        })
    }
}

/// 编译目标：展开环境变量 + 保护检查。
///
/// 含通配符（`*`/`?`）的路径按当前文件系统展开为**现存目录**（多 Profile 浏览器
/// 缓存等场景，对齐 winapp2/CleanerML 的做法）；未安装的应用目录自然消失，
/// 与普通目标"不存在即静默跳过"的语义一致。展开出的每个目标仍逐个过保护检查。
fn compile_targets(raw: &str, protection: &purimaster_platform::Protection) -> Vec<CompiledTarget> {
    let expanded = expand_env(raw);
    if !expanded.contains('*') && !expanded.contains('?') {
        return vec![compile_dir_target(Path::new(&expanded), protection)];
    }
    let pattern = expanded.replace('\\', "/");
    let options = MatchOptions {
        // Windows 路径大小写不敏感；`*` 不得跨越分隔符（防 `User Data\*` 吞掉
        // 更深层结构），`.` 开头的目录也参与匹配
        case_sensitive: false,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    match glob::glob_with(&pattern, options) {
        Ok(paths) => paths
            .filter_map(|p| p.ok())
            .filter(|p| p.is_dir())
            .take(MAX_GLOB_TARGETS)
            .map(|p| compile_dir_target(&p, protection))
            .collect(),
        Err(e) => vec![CompiledTarget {
            path: PathBuf::from(&expanded),
            problem: Some(format!("通配路径无法解析（{e}）")),
        }],
    }
}

fn compile_dir_target(path: &Path, protection: &purimaster_platform::Protection) -> CompiledTarget {
    let problem = if !path.is_absolute() {
        Some("路径含未展开的环境变量或不是绝对路径".to_string())
    } else {
        purimaster_platform::check_target_against(path, protection).err()
    };
    CompiledTarget {
        path: path.to_path_buf(),
        problem,
    }
}

/// 解析 `min_age` 字段：`<数字><单位>`，单位 h（小时）/ d（天）/ w（周）。
fn parse_min_age(raw: &str) -> Result<Duration, String> {
    let raw = raw.trim();
    let invalid = || format!("`{raw}`（应为如 \"24h\"、\"7d\"、\"2w\" 的时长）");
    if raw.len() < 2 {
        return Err(invalid());
    }
    let (digits, unit) = raw.split_at(raw.len() - 1);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let n: u64 = digits.parse().map_err(|_| invalid())?;
    if n == 0 {
        return Err(invalid());
    }
    let seconds = match unit {
        "h" => n.checked_mul(3600),
        "d" => n.checked_mul(86400),
        "w" => n.checked_mul(7 * 86400),
        _ => return Err(invalid()),
    };
    seconds.map(Duration::from_secs).ok_or_else(invalid)
}

/// 编译 glob 列表；空列表返回 None（表示不参与过滤）。
fn compile_globs(patterns: &[String]) -> Result<Option<GlobSet>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .case_insensitive(true)
            .build()
            .map_err(|e| format!("`{pattern}`：{e}"))?;
        builder.add(glob);
    }
    builder.build().map(Some).map_err(|e| e.to_string())
}

/// 展开 `%VAR%` 形式的环境变量；未定义的变量保持原样（交由后续校验报出）。
pub fn expand_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            match std::env::var(name) {
                Ok(value) if !value.is_empty() => out.push_str(&value),
                _ => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        } else {
            // 落单的 %，原样保留
            out.push('%');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// 路径去重键：大小写不敏感 + 分隔符归一（Windows 语义）。
///
/// 不做 `canonicalize`（每文件一次代价过高）；联接点/8.3 别名造成的重复计数
/// 是已记录的限制，由删除阶段的目标级真实路径校验兜底。
fn dedup_key(path: &Path) -> String {
    let mut s = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// collect_files 的旁路统计（跳过/近期/问题明细），收拢为一避免参数爆炸。
#[derive(Default)]
struct CollectStats {
    skipped: u64,
    fresh: u64,
    problems: Vec<String>,
}

/// 递归收集目录下的文件（迭代式，无递归栈风险）。
///
/// `min_age`：文件年龄下限（BleachBit `age` 属性 / CCleaner "只清理 24 小时前的
/// Temp 文件" 同款启发式）。修改时间晚于截止点、或时间戳在未来的文件跳过并计入
/// `stats.fresh`——正在使用的临时/缓存文件几乎总是新写的。
fn collect_files(
    root: &Path,
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
    min_age: Option<Duration>,
    cancel: &AtomicBool,
    stats: &mut CollectStats,
    on_file: &mut dyn FnMut(PathBuf, u64),
) -> Result<(), Cancelled> {
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                // 目录存在但读不了（如 %SystemRoot%\Temp 对标准用户）：必须让用户看见
                stats
                    .problems
                    .push(format!("{}：无法读取目录（{e}）", dir.display()));
                stats.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled);
            }
            let Ok(entry) = entry else {
                stats.skipped += 1;
                continue;
            };
            let Ok(ft) = entry.file_type() else {
                stats.skipped += 1;
                continue;
            };
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue; // 符号链接/其它类型不参与
            }
            let rel = match path.strip_prefix(root) {
                Ok(rel) => rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                Err(_) => {
                    stats.skipped += 1;
                    continue;
                }
            };
            if exclude.is_some_and(|ex| ex.is_match(&rel)) {
                continue;
            }
            if include.is_some_and(|inc| !inc.is_match(&rel)) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                stats.skipped += 1;
                continue;
            };
            if let Some(min_age) = min_age {
                let is_fresh = match meta.modified() {
                    // 修改时间晚于截止点 → 近期文件；elapsed 出错（未来时间戳）同判
                    Ok(m) => m.elapsed().map(|age| age < min_age).unwrap_or(true),
                    Err(_) => true,
                };
                if is_fresh {
                    stats.fresh += 1;
                    continue;
                }
            }
            on_file(path, meta.len());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 清理（删除）——ADR-0003：删除时重新枚举、逐文件重校验、逐文件结果上报
// ---------------------------------------------------------------------------

/// 清理选项。
#[derive(Debug, Clone, Copy)]
pub struct CleanOptions {
    pub mode: purimaster_platform::DeleteMode,
    /// 只做全部安全检查，不实际删除
    pub dry_run: bool,
}

/// 单个失败文件（`failed` 列表最多保留 [`FAILED_PREVIEW`] 条，计数见 `failed_count`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanFileFailure {
    pub path: String,
    pub error: String,
}

/// 清理报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub dry_run: bool,
    /// `"trash"`（回收站）或 `"permanent"`（永久删除）
    pub mode: &'static str,
    /// 已删除（或 dry-run 下"将删除"）的文件数
    pub deleted_files: u64,
    /// 移入回收站/删除的文件总字节数。
    /// 注意：回收站模式下需清空回收站后才真正释放磁盘空间。
    pub freed_bytes: u64,
    pub failed_count: u64,
    pub failed: Vec<CleanFileFailure>,
    /// 因读取失败/目标异常而跳过的条目数
    pub skipped: u64,
    /// 被跳过/拒绝的规则或目标说明
    pub skipped_rules: Vec<String>,
    /// 是否因取消而提前结束（已完成的部分仍计入报告）
    pub cancelled: bool,
}

/// `failed` 列表最多保留多少条明细
const FAILED_PREVIEW: usize = 100;

impl RuleSet {
    /// 按规则 id 清理垃圾文件。
    ///
    /// - 删除前**重新枚举**规则目标并**重新校验**（ADR-0003，不信任扫描结果）
    /// - 归属计算与 `scan` 一致：按规则顺序全局去重，保证实际删除量不超过确认弹窗的统计
    /// - 单个文件失败不中断，计入 `failed`
    /// - 取消会提前结束，已完成的部分保留在报告中（`cancelled = true`）
    /// - `progress`：每条选中规则开始与每个文件处理完后回调（调用方负责节流）
    pub fn clean(
        &self,
        rule_ids: &[String],
        cancel: &AtomicBool,
        options: CleanOptions,
        progress: &mut dyn FnMut(CleanProgress),
    ) -> CleanReport {
        let mut report = CleanReport {
            dry_run: options.dry_run,
            mode: options.mode.as_str(),
            deleted_files: 0,
            freed_bytes: 0,
            failed_count: 0,
            failed: Vec::new(),
            skipped: 0,
            skipped_rules: Vec::new(),
            cancelled: false,
        };
        if rule_ids.is_empty() {
            return report;
        }

        let known: std::collections::HashSet<&str> =
            self.rules.iter().map(|r| r.id.as_str()).collect();
        // 未知规则 id 在开始前一次性报告（与取消无关，避免取消后误报）
        for id in rule_ids {
            if !known.contains(id.as_str()) {
                report.skipped_rules.push(format!("{id}：规则不存在"));
            }
        }

        let selected: std::collections::HashSet<&str> =
            rule_ids.iter().map(String::as_str).collect();
        // 归属去重与 scan 保持一致：所有规则按顺序参与，未选中的规则只参与"认领"不删除
        let mut seen: HashSet<String> = HashSet::new();

        'rules: for rule in &self.rules {
            let is_selected = selected.contains(rule.id.as_str());
            if is_selected {
                progress(CleanProgress {
                    deleted_files: report.deleted_files,
                    freed_bytes: report.freed_bytes,
                    failed_count: report.failed_count,
                    current_title: rule.title.clone(),
                });
                if rule.scan_only {
                    report.skipped_rules.push(format!(
                        "{}：规则配置为仅扫描（scan_only），本轮未清理",
                        rule.id
                    ));
                }
            }
            let title = rule.title.clone();
            let may_delete = is_selected && !rule.scan_only;
            for target in &rule.targets {
                if cancel.load(Ordering::Relaxed) {
                    report.cancelled = true;
                    break 'rules;
                }
                if let Some(problem) = &target.problem {
                    if is_selected {
                        report.skipped_rules.push(format!("{}：{problem}", rule.id));
                    }
                    continue;
                }
                // 目标身份校验：必须仍是普通目录，且不能是链接/联接点
                match fs::symlink_metadata(&target.path) {
                    Ok(m) if m.file_type().is_symlink() => {
                        if is_selected {
                            report
                                .skipped_rules
                                .push(format!("{}：目标本身是链接或联接点，拒绝清理", rule.id));
                            report.skipped += 1;
                        }
                        continue;
                    }
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => {
                        if is_selected {
                            report
                                .skipped_rules
                                .push(format!("{}：目标不是目录", rule.id));
                            report.skipped += 1;
                        }
                        continue;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        if is_selected {
                            report
                                .skipped_rules
                                .push(format!("{}：无法访问目标（{e}）", rule.id));
                            report.skipped += 1;
                        }
                        continue;
                    }
                }
                // 删除时重新校验目标（防止扫描后目标被替换/篡改）
                if is_selected && let Err(e) = purimaster_platform::check_target(&target.path) {
                    report.skipped_rules.push(format!("{}：{e}", rule.id));
                    report.skipped += 1;
                    continue;
                }

                let mut stats = CollectStats::default();
                let walk = collect_files(
                    &target.path,
                    rule.include.as_ref(),
                    rule.exclude.as_ref(),
                    rule.min_age,
                    cancel,
                    &mut stats,
                    &mut |path, _size| {
                        if !seen.insert(dedup_key(&path)) {
                            return;
                        }
                        // 未选中的规则与 scan_only 规则只参与归属计算，不执行删除
                        if !may_delete {
                            return;
                        }
                        match purimaster_platform::delete_file(
                            &target.path,
                            &path,
                            options.mode,
                            options.dry_run,
                        ) {
                            Ok(size) => {
                                report.deleted_files += 1;
                                report.freed_bytes += size;
                            }
                            Err(e) => {
                                report.failed_count += 1;
                                if report.failed.len() < FAILED_PREVIEW {
                                    report.failed.push(CleanFileFailure {
                                        path: path.display().to_string(),
                                        error: e.to_string(),
                                    });
                                }
                            }
                        }
                        progress(CleanProgress {
                            deleted_files: report.deleted_files,
                            freed_bytes: report.freed_bytes,
                            failed_count: report.failed_count,
                            current_title: title.clone(),
                        });
                    },
                );
                // 无论是否取消都回写跳过/问题信息，避免报告丢失现场
                if is_selected {
                    report.skipped += stats.skipped;
                    for p in stats.problems {
                        report.skipped_rules.push(format!("{}：{p}", rule.id));
                    }
                }
                if walk.is_err() {
                    report.cancelled = true;
                    break 'rules;
                }
            }
        }

        report
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkGroup {
    pub rule_id: String,
    pub category: String,
    pub title: String,
    pub risk: Risk,
    /// 仅扫描：清理阶段会跳过该规则
    pub scan_only: bool,
    /// 面向用户的一句话说明（可能为空）
    pub description: String,
    /// 因年龄策略（min_age）跳过的近期文件数——检测透明化：
    /// 用户应知道"这些文件没算进来"以及原因
    pub fresh_files: u64,
    pub files: u64,
    pub bytes: u64,
    /// 实际存在并已扫描的目标目录
    pub targets: Vec<String>,
    /// 最大的若干文件预览（≤ [`PREVIEW_LIMIT`] 条，按大小降序）
    pub top_files: Vec<JunkFile>,
    /// 被拒绝/异常的目标说明（保护路径、未展开变量、非目录等）
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkScanResult {
    /// 分类 id → 显示名（供 UI 分组）
    pub categories: BTreeMap<String, String>,
    pub groups: Vec<JunkGroup>,
    pub total_files: u64,
    pub total_bytes: u64,
    /// 因权限等原因读取失败的条目数
    pub skipped: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use purimaster_platform::Protection;
    use std::fs;

    fn tomldir(path: &Path) -> String {
        // TOML 字面量字符串（单引号），避免 Windows 反斜杠转义问题
        format!("'{}'", path.display())
    }

    fn rule_toml(paths: &str, include: Option<&str>, exclude: Option<&str>) -> String {
        let include_line = include
            .map(|i| format!("include = {i}"))
            .unwrap_or_default();
        let exclude_line = exclude
            .map(|e| format!("exclude = {e}"))
            .unwrap_or_default();
        format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "test-rule"
category = "temp"
title = "测试规则"
paths = [{paths}]
{include_line}
{exclude_line}
"#
        )
    }

    fn scan_with(set: &RuleSet) -> JunkScanResult {
        set.scan(&AtomicBool::new(false), &mut |_| {}).unwrap()
    }

    #[test]
    fn scan_counts_and_previews_top_files() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("a.bin"), [0u8; 100]).unwrap();
        fs::create_dir(junk.join("sub")).unwrap();
        fs::write(junk.join("sub").join("b.bin"), [0u8; 300]).unwrap();
        fs::write(junk.join("sub").join("c.bin"), [0u8; 50]).unwrap();

        let toml = rule_toml(&tomldir(&junk), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);

        assert_eq!(result.total_files, 3);
        assert_eq!(result.total_bytes, 450);
        let group = &result.groups[0];
        assert_eq!(group.files, 3);
        assert_eq!(group.bytes, 450);
        assert_eq!(group.targets.len(), 1);
        assert!(group.problems.is_empty());
        assert_eq!(group.top_files[0].size, 300);
        assert_eq!(group.top_files.len(), 3);
        assert_eq!(group.risk, Risk::Low);
    }

    #[test]
    fn include_and_exclude_globs() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("keep.log"), [0u8; 10]).unwrap();
        fs::write(junk.join("skip.log"), [0u8; 20]).unwrap();
        fs::write(junk.join("other.txt"), [0u8; 30]).unwrap();

        let toml = rule_toml(
            &tomldir(&junk),
            Some(r#"["**/*.log"]"#),
            Some(r#"["**/skip*"]"#),
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);

        assert_eq!(result.total_files, 1);
        assert_eq!(result.total_bytes, 10);
        assert!(result.groups[0].top_files[0].path.ends_with("keep.log"));
    }

    #[test]
    fn duplicate_rule_ids_rejected() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "dup"
category = "temp"
title = "A"
paths = ["C:\\temp-a"]

[[rule]]
id = "dup"
category = "temp"
title = "B"
paths = ["C:\\temp-b"]
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap_err();
        assert!(err.to_string().contains("重复"), "实际错误：{err}");
    }

    #[test]
    fn unknown_category_rejected() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "x"
category = "nope"
title = "X"
paths = ["C:\\x"]
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap_err();
        assert!(err.to_string().contains("未定义的分类"), "实际错误：{err}");
    }

    #[test]
    fn empty_paths_rejected() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "x"
category = "temp"
title = "X"
paths = []
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap_err();
        assert!(
            err.to_string().contains("至少要有一个 path"),
            "实际错误：{err}"
        );
    }

    #[test]
    fn bad_glob_rejected() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "x"
category = "temp"
title = "X"
paths = ["C:\\x"]
include = ["["]
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap_err();
        assert!(err.to_string().contains("include 非法"), "实际错误：{err}");
    }

    #[test]
    fn env_expansion() {
        let expanded = expand_env("%SystemRoot%\\Temp");
        assert!(!expanded.contains('%'), "应完全展开：{expanded}");
        assert!(expanded.ends_with("\\Temp"));

        let missing = expand_env("%PURIMASTER_NO_SUCH_VAR%\\x");
        assert_eq!(missing, "%PURIMASTER_NO_SUCH_VAR%\\x");
    }

    #[test]
    fn protected_target_is_rejected_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let sensitive = dir.path().join("system32");
        let safe = dir.path().join("temp");
        fs::create_dir(&sensitive).unwrap();
        fs::create_dir(&safe).unwrap();
        fs::write(sensitive.join("critical.dll"), [0u8; 100]).unwrap();
        fs::write(safe.join("junk.bin"), [0u8; 10]).unwrap();

        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "multi"
category = "temp"
title = "多目标"
paths = [{sensitive}, {safe}]
"#,
            sensitive = tomldir(&sensitive),
            safe = tomldir(&safe)
        );
        let protection = Protection {
            sensitive: vec![sensitive.clone()],
            ancestor_only: vec![],
        };
        let set = RuleSet::from_toml_strs_with_protection(&[&toml], &protection).unwrap();
        let result = scan_with(&set);

        let group = &result.groups[0];
        // 受保护目标被拒绝并说明原因；安全目标正常扫描
        assert_eq!(group.files, 1);
        assert_eq!(result.total_bytes, 10);
        assert_eq!(group.problems.len(), 1);
        assert!(group.problems[0].contains("受保护路径"));
        assert_eq!(group.targets.len(), 1);
    }

    #[test]
    fn dedup_across_rules_counts_once() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.bin"), [0u8; 20]).unwrap();

        let paths = tomldir(&junk);
        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r1"
category = "temp"
title = "R1"
paths = [{paths}]

[[rule]]
id = "r2"
category = "temp"
title = "R2"
paths = [{paths}]
"#
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);

        assert_eq!(result.total_files, 2);
        assert_eq!(result.groups[0].files, 2);
        assert_eq!(result.groups[1].files, 0);
    }

    #[test]
    fn duplicate_paths_are_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        let path = tomldir(&junk);
        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r1"
category = "temp"
title = "R1"
paths = [{path}, {path}]
"#
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        assert_eq!(set.rules()[0].targets.len(), 1);
    }

    #[test]
    fn cancelled_scan_returns_error() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "x"
category = "temp"
title = "X"
paths = ["C:\\x"]
"#;
        let set = RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(set.scan(&cancel, &mut |_| {}).is_err());
    }

    #[test]
    fn missing_target_is_silently_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-installed");
        let toml = rule_toml(&tomldir(&missing), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);
        assert_eq!(result.total_files, 0);
        assert!(result.groups[0].problems.is_empty());
        assert!(result.groups[0].targets.is_empty());
    }

    #[test]
    fn merges_multiple_sources() {
        let a = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r1"
category = "temp"
title = "R1"
paths = ["C:\\r1"]
"#;
        let b = r#"
[categories]
browser = "浏览器缓存"

[[rule]]
id = "r2"
category = "browser"
title = "R2"
paths = ["C:\\r2"]
"#;
        let set = RuleSet::from_toml_strs_with_protection(&[a, b], &Protection::default()).unwrap();
        assert_eq!(set.rules().len(), 2);
        assert_eq!(set.categories().len(), 2);
    }

    #[test]
    fn category_conflict_rejected() {
        let a = r#"
[categories]
temp = "临时文件"
"#;
        let b = r#"
[categories]
temp = "另一个名字"
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[a, b], &Protection::default()).unwrap_err();
        assert!(err.to_string().contains("定义冲突"), "实际错误：{err}");
    }

    #[test]
    fn load_dir_reads_all_toml_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.toml"),
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r1"
category = "temp"
title = "R1"
paths = ["C:\\r1"]
"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("b.toml"),
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r2"
category = "temp"
title = "R2"
paths = ["C:\\r2"]
"#,
        )
        .unwrap();
        // 非 toml 文件应被忽略
        fs::write(dir.path().join("notes.txt"), "not a rule").unwrap();

        let set = RuleSet::load_dir(dir.path()).unwrap();
        assert_eq!(set.rules().len(), 2);
    }

    #[test]
    fn preview_limit_caps_large_directories() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        for i in 1..=60usize {
            fs::write(junk.join(format!("f{i:02}.bin")), vec![0u8; i]).unwrap();
        }
        let toml = rule_toml(&tomldir(&junk), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);

        assert_eq!(result.total_files, 60);
        let group = &result.groups[0];
        assert_eq!(group.top_files.len(), PREVIEW_LIMIT);
        // 预览为最大的 50 个，首条应为 60 字节
        assert_eq!(group.top_files[0].size, 60);
        assert_eq!(group.top_files.last().unwrap().size, 11);
    }

    #[test]
    fn deny_unknown_fields() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "x"
category = "temp"
title = "X"
paths = ["C:\\x"]
include_paths = ["typo"]
"#;
        let err =
            RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap_err();
        assert!(matches!(err, RuleError::Toml(_)), "实际错误：{err}");
    }

    // -----------------------------------------------------------------------
    // 清理（删除）测试 —— ADR-0003
    // -----------------------------------------------------------------------

    fn clean_opts(mode: purimaster_platform::DeleteMode, dry_run: bool) -> CleanOptions {
        CleanOptions { mode, dry_run }
    }

    fn junk_set(dir: &Path) -> (RuleSet, std::path::PathBuf) {
        let junk = dir.join("junk");
        fs::create_dir(&junk).unwrap();
        let toml = rule_toml(&tomldir(&junk), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        (set, junk)
    }

    #[test]
    fn clean_dry_run_keeps_files() {
        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.bin"), [0u8; 20]).unwrap();

        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, true),
            &mut |_| {},
        );
        assert!(report.dry_run);
        assert_eq!(report.deleted_files, 2);
        assert_eq!(report.freed_bytes, 30);
        assert!(!report.cancelled);
        assert!(junk.join("a.bin").exists(), "dry-run 不得删除文件");
        assert!(junk.join("b.bin").exists());
    }

    #[test]
    fn clean_permanent_removes_files() {
        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.bin"), [0u8; 20]).unwrap();

        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 2);
        assert_eq!(report.freed_bytes, 30);
        assert!(!junk.join("a.bin").exists());
        assert!(!junk.join("b.bin").exists());
    }

    #[test]
    fn clean_unknown_rule_id_is_reported() {
        let set = RuleSet::default();
        let report = set.clean(
            &["nope".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Trash, true),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 0);
        assert!(
            report
                .skipped_rules
                .iter()
                .any(|s| s.contains("规则不存在"))
        );
    }

    #[test]
    fn clean_cancelled_before_start_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();

        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(true),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert!(report.cancelled);
        assert_eq!(report.deleted_files, 0);
        assert!(junk.join("a.bin").exists());
    }

    #[cfg(windows)]
    #[test]
    fn clean_failure_does_not_abort_batch() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        // 以 share_mode(0) 独占打开：模拟被占用文件，删除时会产生共享冲突
        let locked = junk.join("locked.bin");
        fs::write(&locked, [0u8; 5]).unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();
        fs::write(junk.join("ok.bin"), [0u8; 7]).unwrap();

        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        // 正常文件被删除，被占用文件失败但不中断整批
        assert_eq!(report.deleted_files, 1);
        assert_eq!(report.failed_count, 1);
        assert!(!report.failed.is_empty());
        assert!(locked.exists());

        drop(handle);
    }

    #[cfg(windows)]
    #[test]
    fn clean_rejects_protected_target() {
        if std::env::var_os("SystemRoot").is_none() {
            return;
        }
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "evil"
category = "temp"
title = "恶意规则"
paths = ['%SystemRoot%\System32']
"#;
        let set = RuleSet::from_toml_strs(&[toml]).unwrap();
        let report = set.clean(
            &["evil".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, true),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 0);
        assert!(
            report.skipped_rules.iter().any(|s| s.contains("受保护")),
            "实际：{:?}",
            report.skipped_rules
        );
    }

    #[test]
    fn clean_subset_matches_scan_attribution() {
        // 审查发现：子集清理时不能删除"扫描时归属其它规则"的文件（确认数字才可信）
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("a.log"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.txt"), [0u8; 20]).unwrap();

        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "logs"
category = "temp"
title = "日志"
paths = [{p}]
include = ["**/*.log"]

[[rule]]
id = "rest"
category = "temp"
title = "其余"
paths = [{p}]
"#,
            p = tomldir(&junk)
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();

        let scan = scan_with(&set);
        let rest_group = scan.groups.iter().find(|g| g.rule_id == "rest").unwrap();
        assert_eq!(rest_group.files, 1, "a.log 应归属 logs，rest 只认领 b.txt");

        // 只选 rest：只应删除 b.txt，a.log 属于未选中的 logs
        let report = set.clean(
            &["rest".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 1);
        assert!(junk.join("a.log").exists());
        assert!(!junk.join("b.txt").exists());
    }

    #[test]
    fn cancelled_clean_does_not_misreport_rules() {
        let toml = r#"
[categories]
temp = "临时文件"

[[rule]]
id = "r1"
category = "temp"
title = "R1"
paths = ["C:\\__purimaster_no_such_a__"]

[[rule]]
id = "r2"
category = "temp"
title = "R2"
paths = ["C:\\__purimaster_no_such_b__"]
"#;
        let set = RuleSet::from_toml_strs_with_protection(&[toml], &Protection::default()).unwrap();
        let report = set.clean(
            &["r1".to_string(), "r2".to_string()],
            &AtomicBool::new(true),
            clean_opts(purimaster_platform::DeleteMode::Trash, true),
            &mut |_| {},
        );
        assert!(report.cancelled);
        assert!(
            !report
                .skipped_rules
                .iter()
                .any(|s| s.contains("规则不存在")),
            "取消不应把存在的规则误报为不存在：{:?}",
            report.skipped_rules
        );
    }

    #[test]
    fn clean_reports_non_directory_target() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir.txt");
        fs::write(&file, [0u8; 3]).unwrap();
        let toml = rule_toml(&tomldir(&file), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, true),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 0);
        assert_eq!(report.skipped, 1);
        assert!(
            report
                .skipped_rules
                .iter()
                .any(|s| s.contains("目标不是目录")),
            "实际：{:?}",
            report.skipped_rules
        );
    }

    // -----------------------------------------------------------------------
    // 进度回调测试（P0：清理/扫描进度事件）
    // -----------------------------------------------------------------------

    #[test]
    fn scan_progress_reports_rule_boundary_and_final_counts() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.bin"), [0u8; 20]).unwrap();

        let toml = rule_toml(&tomldir(&junk), None, None);
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();

        let mut calls = Vec::new();
        set.scan(&AtomicBool::new(false), &mut |p| calls.push(p))
            .unwrap();
        // 首条：规则开始，尚未发现文件
        assert_eq!(calls[0].rule_index, 0);
        assert_eq!(calls[0].rule_count, 1);
        assert_eq!(calls[0].current_title, "测试规则");
        assert_eq!(calls[0].files, 0);
        // 末条：累计到最终计数
        let last = calls.last().unwrap();
        assert_eq!(last.files, 2);
        assert_eq!(last.bytes, 30);
    }

    #[test]
    fn clean_progress_reports_running_counts() {
        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();
        fs::write(junk.join("b.bin"), [0u8; 20]).unwrap();

        let mut calls = Vec::new();
        set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |p| calls.push(p),
        );
        // 首条：规则开始（0 删除），末条：全部完成
        assert_eq!(calls[0].deleted_files, 0);
        assert_eq!(calls[0].current_title, "测试规则");
        let last = calls.last().unwrap();
        assert_eq!(last.deleted_files, 2);
        assert_eq!(last.freed_bytes, 30);
    }

    #[cfg(windows)]
    #[test]
    fn clean_progress_reports_failures_too() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let (set, junk) = junk_set(dir.path());
        // 与 clean_failure_does_not_abort_batch 相同手法：独占句柄制造共享冲突
        let locked = junk.join("locked.bin");
        fs::write(&locked, [0u8; 5]).unwrap();
        let handle = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&locked)
            .unwrap();

        let mut failed_seen = 0u64;
        set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |p| failed_seen = failed_seen.max(p.failed_count),
        );
        assert_eq!(failed_seen, 1, "失败计数应通过进度可见");
        drop(handle);
    }

    // -----------------------------------------------------------------------
    // scan_only 与 serde 契约测试（模拟用户测试 F-11 / F-41）
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // 筛选算法：min_age 文件年龄与通配目标展开（对标 CleanerML/winapp2）
    // -----------------------------------------------------------------------

    /// 把文件修改时间设为 now - age_secs（测试夹具：SetFileTime，仅 Windows；
    /// 非 Windows 平台 no-op——min_age 逻辑本身有独立于平台的单测覆盖）
    #[cfg(windows)]
    fn set_mtime(path: &Path, age_secs: u64) {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::{FILETIME, HANDLE};
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, SetFileTime};

        // FILETIME = 100ns 自 1601-01-01；UNIX epoch 差 11,644,473,600 秒
        let target = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - age_secs;
        let ft100ns = (target + 11_644_473_600) * 10_000_000;
        let ft = FILETIME {
            dwLowDateTime: (ft100ns & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (ft100ns >> 32) as u32,
        };
        // 改 mtime 需要 FILE_WRITE_ATTRIBUTES 访问权：custom_access_mask + 该位，
        // 只读打开会静默失败（SetFileTime 返回假成功之外的静默丢弃）
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0001_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        let file = std::fs::OpenOptions::new()
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .access_mode(GENERIC_WRITE | FILE_WRITE_ATTRIBUTES)
            .open(path)
            .unwrap();
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file) as HANDLE;
        unsafe {
            let ok = SetFileTime(handle, std::ptr::null(), std::ptr::null(), &ft);
            if ok == 0 {
                panic!(
                    "SetFileTime 失败（GetLastError={}）",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    #[cfg(not(windows))]
    fn set_mtime(_path: &Path, _age_secs: u64) {}

    #[test]
    fn parse_min_age_units() {
        assert_eq!(parse_min_age("24h"), Ok(Duration::from_secs(86400)));
        assert_eq!(parse_min_age("7d"), Ok(Duration::from_secs(7 * 86400)));
        assert_eq!(parse_min_age("2w"), Ok(Duration::from_secs(14 * 86400)));
        // 非法输入
        assert!(parse_min_age("24").is_err());
        assert!(parse_min_age("h").is_err());
        assert!(parse_min_age("0d").is_err());
        assert!(parse_min_age("24x").is_err());
        assert!(parse_min_age("-1d").is_err());
    }

    #[test]
    fn min_age_skips_recent_files_and_reports_fresh_count() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        let fresh_file = junk.join("fresh.bin");
        let old_file = junk.join("old.bin");
        fs::write(&fresh_file, [0u8; 10]).unwrap();
        fs::write(&old_file, [0u8; 20]).unwrap();
        set_mtime(&fresh_file, 60); // 1 分钟前
        set_mtime(&old_file, 3 * 86400); // 3 天前
        // 夹具自检：backdate 必须真实生效，否则后续断言无意义
        let age = old_file
            .metadata()
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap();
        assert!(
            age > Duration::from_secs(86400),
            "测试夹具未能回拨 mtime（实际年龄 {age:?}），SetFileTime 失败"
        );

        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "test-rule"
category = "temp"
title = "测试规则"
min_age = "24h"
paths = [{paths}]
"#,
            paths = tomldir(&junk)
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();

        let result = scan_with(&set);
        let group = &result.groups[0];
        // 只统计老文件
        assert_eq!(group.files, 1);
        assert_eq!(group.bytes, 20);
        assert_eq!(group.fresh_files, 1, "近期文件应单独计数展示");
        assert!(group.top_files[0].path.ends_with("old.bin"));

        // 清理一致：只有老文件被删
        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 1);
        assert!(fresh_file.exists(), "近期文件不得被清理");
        assert!(!old_file.exists());
    }

    #[test]
    fn glob_paths_expand_to_existing_directories() {
        let base = tempfile::tempdir().unwrap();
        // 模拟多 Profile：profile-a / profile-b 存在，profile-c 目录不测（不创建）
        let pa = base.path().join("User Data").join("Profile A");
        let pb = base.path().join("User Data").join("Profile B");
        fs::create_dir_all(pa.join("Cache")).unwrap();
        fs::create_dir_all(pb.join("Cache")).unwrap();
        fs::write(pa.join("Cache").join("a.bin"), [0u8; 10]).unwrap();
        fs::write(pb.join("Cache").join("b.bin"), [0u8; 20]).unwrap();

        // 目录名带空格：toml 字面量串安全；glob 模式展开 Profile 目录
        let pattern = base.path().join("User Data").join("*").join("Cache");
        let toml = format!(
            r#"
[categories]
browser = "浏览器缓存"

[[rule]]
id = "profiles"
category = "browser"
title = "多 Profile 缓存"
paths = ['{pattern}']
"#,
            pattern = pattern.display()
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();

        let rule = &set.rules()[0];
        assert_eq!(
            rule.targets.len(),
            2,
            "应展开为两个现存 Profile 的 Cache 目录"
        );

        let result = scan_with(&set);
        assert_eq!(result.total_files, 2);
        assert_eq!(result.total_bytes, 30);
    }

    #[test]
    fn description_survives_to_group() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "test-rule"
category = "temp"
title = "测试规则"
description = "这是一段给用户看的说明"
paths = [{paths}]
"#,
            paths = tomldir(&junk)
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();
        let result = scan_with(&set);
        assert_eq!(result.groups[0].description, "这是一段给用户看的说明");
    }

    // -----------------------------------------------------------------------
    // 定向清理（用户指定目录的临时规则集）
    // -----------------------------------------------------------------------

    #[test]
    fn targeted_ruleset_scans_and_cleans_user_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.log"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("b.tmp"), [0u8; 20]).unwrap();

        // 全部文件
        let set = RuleSet::targeted(dir.path(), &[], None, &Protection::default()).unwrap();
        let result = scan_with(&set);
        assert_eq!(result.total_files, 2);
        assert_eq!(result.groups[0].title, "定向清理");
        assert!(result.groups[0].description.contains("你指定的目录"));

        // 清理走同一管线
        let report = set.clean(
            &["targeted".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 2);
        assert!(!dir.path().join("a.log").exists());
    }

    #[test]
    fn targeted_ruleset_respects_include_glob() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.log"), [0u8; 10]).unwrap();
        fs::write(dir.path().join("b.tmp"), [0u8; 20]).unwrap();

        let set = RuleSet::targeted(
            dir.path(),
            &["*.log".to_string()],
            None,
            &Protection::default(),
        )
        .unwrap();
        let result = scan_with(&set);
        assert_eq!(result.total_files, 1);
        assert!(result.groups[0].top_files[0].path.ends_with("a.log"));
    }

    #[test]
    fn targeted_ruleset_rejects_protected_target() {
        // 保护检查在构造时执行：sensitive 目录的目标直接带 problem，扫描为空
        let dir = tempfile::tempdir().unwrap();
        let sensitive = dir.path().join("sensitive");
        fs::create_dir(&sensitive).unwrap();
        fs::write(sensitive.join("x.bin"), [0u8; 5]).unwrap();
        let protection = Protection {
            sensitive: vec![sensitive.clone()],
            ancestor_only: vec![],
        };

        let set = RuleSet::targeted(&sensitive, &[], None, &protection).unwrap();
        assert!(
            set.rules()[0].targets[0].problem.is_some(),
            "受保护目标应在构造期被标记"
        );
        let result = scan_with(&set);
        assert_eq!(result.total_files, 0);
        assert!(!result.groups[0].problems.is_empty(), "问题应可见");
    }

    #[test]
    fn targeted_ruleset_bad_glob_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = RuleSet::targeted(dir.path(), &["[".to_string()], None, &Protection::default())
            .unwrap_err();
        assert!(err.to_string().contains("文件类型过滤非法"), "实际：{err}");
    }

    // -----------------------------------------------------------------------
    // scan_only 与 serde 契约测试（模拟用户测试 F-11 / F-41，原位置）
    // -----------------------------------------------------------------------

    #[test]
    fn scan_only_rule_is_scanned_but_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk");
        fs::create_dir(&junk).unwrap();
        fs::write(junk.join("a.bin"), [0u8; 10]).unwrap();

        let toml = format!(
            r#"
[categories]
temp = "临时文件"

[[rule]]
id = "test-rule"
category = "temp"
title = "测试规则"
scan_only = true
paths = [{paths}]
"#,
            paths = tomldir(&junk)
        );
        let set =
            RuleSet::from_toml_strs_with_protection(&[&toml], &Protection::default()).unwrap();

        // 扫描正常统计
        let result = scan_with(&set);
        assert_eq!(result.total_files, 1);
        assert!(result.groups[0].scan_only);

        // 清理跳过：文件保留，报告说明原因
        let report = set.clean(
            &["test-rule".to_string()],
            &AtomicBool::new(false),
            clean_opts(purimaster_platform::DeleteMode::Permanent, false),
            &mut |_| {},
        );
        assert_eq!(report.deleted_files, 0);
        assert!(junk.join("a.bin").exists(), "scan_only 规则不得删除文件");
        assert!(
            report.skipped_rules.iter().any(|s| s.contains("仅扫描")),
            "实际：{:?}",
            report.skipped_rules
        );
    }

    #[test]
    fn progress_and_report_serde_fields_are_camel_case() {
        // IPC 契约快照：字段名漂移会让前端拿到 undefined（模拟用户测试 F-41 建议）
        let scan = serde_json::to_value(JunkScanProgress {
            rule_index: 0,
            rule_count: 8,
            current_title: "t".into(),
            files: 1,
            bytes: 2,
        })
        .unwrap();
        assert_eq!(
            scan,
            serde_json::json!({
                "ruleIndex": 0, "ruleCount": 8, "currentTitle": "t", "files": 1, "bytes": 2
            })
        );

        let clean = serde_json::to_value(CleanProgress {
            deleted_files: 1,
            freed_bytes: 2,
            failed_count: 3,
            current_title: "t".into(),
        })
        .unwrap();
        assert_eq!(
            clean,
            serde_json::json!({
                "deletedFiles": 1, "freedBytes": 2, "failedCount": 3, "currentTitle": "t"
            })
        );

        let report = serde_json::to_value(CleanReport {
            dry_run: true,
            mode: "trash",
            deleted_files: 0,
            freed_bytes: 0,
            failed_count: 0,
            failed: vec![],
            skipped: 0,
            skipped_rules: vec![],
            cancelled: false,
        })
        .unwrap();
        assert_eq!(
            report,
            serde_json::json!({
                "dryRun": true, "mode": "trash", "deletedFiles": 0, "freedBytes": 0,
                "failedCount": 0, "failed": [], "skipped": 0, "skippedRules": [],
                "cancelled": false
            })
        );
    }
}
