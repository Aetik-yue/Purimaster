/// purimaster_core::NodeSummary 的前端镜像
export interface NodeSummary {
  id: number;
  /** 目录名；根节点无文件名时为完整路径 */
  name: string;
  size: number;
  files: number;
  dirs: number;
  /** 因权限等原因未能读取：大小未知（区别于空目录） */
  unreadable: boolean;
  /** 自身可读但子树内存在无权限目录：总大小不含这部分 */
  unknownSubtree: boolean;
  hasChildren: boolean;
}

/** purimaster_core::FileEntry */
export interface FileEntry {
  name: string;
  size: number;
}

/** purimaster_core::FileListing（list_files 返回） */
export interface FileListing {
  files: FileEntry[];
  /** 排序后的全量文件数（分页时不变） */
  total: number;
}

/** purimaster_platform::DriveInfo（list_drives 返回） */
export interface DriveInfo {
  /** 盘符根路径，如 "C:\" */
  root: string;
  /** "fixed" | "removable" | "network" | "cdrom" | "ramdisk" | "unknown" */
  kind: string;
}

/** scan://progress 事件负载 */
export interface ScanProgressPayload {
  clientId: string;
  filesSeen: number;
  bytesSeen: number;
}

/** scan://complete 事件负载 */
export interface ScanCompletePayload {
  clientId: string;
  filesSeen: number;
  bytesSeen: number;
  skipped: number;
  /** 被跳过的符号链接/联接点数量（防环路，不计入大小） */
  junctions: number;
  root: NodeSummary;
  /** 根节点的直接子目录（更深层按需拉取） */
  rootChildren: NodeSummary[];
}

/** purimaster_rules::JunkScanProgress（junk://progress 事件负载） */
export interface JunkScanProgressPayload {
  clientId: string;
  /** 当前规则下标（0 起） */
  ruleIndex: number;
  ruleCount: number;
  currentTitle: string;
  files: number;
  bytes: number;
}

// ---------------------------------------------------------------------------
// 垃圾清理（purimaster-rules）
// ---------------------------------------------------------------------------

export type Risk = "low" | "medium" | "high";

/** purimaster_rules::JunkFile */
export interface JunkFile {
  path: string;
  size: number;
}

/** purimaster_rules::JunkGroup */
export interface JunkGroup {
  ruleId: string;
  category: string;
  title: string;
  risk: Risk;
  /** 仅扫描：清理阶段会跳过该规则 */
  scanOnly: boolean;
  /** 面向用户的一句话说明（可能为空） */
  description: string;
  /** 因年龄策略跳过的近期文件数（检测透明化：这些文件没算进来） */
  freshFiles: number;
  files: number;
  bytes: number;
  targets: string[];
  topFiles: JunkFile[];
  problems: string[];
}

/** purimaster_rules::JunkScanResult */
export interface JunkScanResult {
  /** 分类 id → 显示名 */
  categories: Record<string, string>;
  groups: JunkGroup[];
  totalFiles: number;
  totalBytes: number;
  skipped: number;
}

/** junk://complete 事件负载 */
export interface JunkCompletePayload {
  clientId: string;
  result: JunkScanResult;
}

/** targeted://complete 事件负载（定向清理扫描结果，结构与规则扫描一致） */
export interface TargetedCompletePayload {
  clientId: string;
  result: JunkScanResult;
}

/** purimaster_rules::CleanFileFailure */
export interface CleanFileFailure {
  path: string;
  error: string;
}

/** purimaster_rules::CleanReport */
export interface CleanReport {
  dryRun: boolean;
  mode: "trash" | "permanent";
  deletedFiles: number;
  freedBytes: number;
  failedCount: number;
  failed: CleanFileFailure[];
  /** 因读取失败/目标异常而跳过的条目数 */
  skipped: number;
  skippedRules: string[];
  cancelled: boolean;
}

/** purimaster_rules::CleanProgress（clean://progress 事件负载） */
export interface CleanProgressPayload {
  clientId: string;
  deletedFiles: number;
  freedBytes: number;
  failedCount: number;
  currentTitle: string;
}

/** clean://complete 事件负载 */
export interface CleanCompletePayload {
  clientId: string;
  report: CleanReport;
}

// ---------------------------------------------------------------------------
// 软件卸载（ADR-0004：官方卸载器优先，注册表删除守卫，残留逐项确认）
// ---------------------------------------------------------------------------

/** purimaster_platform::InstalledProgram */
export interface InstalledProgram {
  displayName: string;
  displayVersion: string;
  publisher: string;
  installLocation: string;
  uninstallString: string;
  /** EstimatedSize（KB），0 = 未提供 */
  estimatedSizeKb: number;
  installDate: string;
  registryKey: string;
}

/** purimaster_platform::ResidueDir */
export interface ResidueDir {
  path: string;
  size: number;
}

/** purimaster_platform::ResidueReport */
export interface ResidueReport {
  programName: string;
  files: ResidueDir[];
  registry: string[];
}

/** uninstall://complete 里的单个结果 */
export interface UninstallResult {
  name: string;
  ok: boolean;
  detail: string;
}

/** uninstall://progress 事件负载 */
export interface UninstallProgressPayload {
  clientId: string;
  index: number;
  total: number;
  name: string;
  status: string;
}

/** uninstall://complete 事件负载 */
export interface UninstallCompletePayload {
  clientId: string;
  results: UninstallResult[];
}

/** app://close-busy 事件负载（关窗被阻止：有任务进行中） */
export interface CloseBusyPayload {
  /** 正在运行的任务数 */
  running: number;
  /** 是否包含清理任务（退出会中断删除且已删部分无报告，需要更强的警告文案） */
  cleaning: boolean;
}
