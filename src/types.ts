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
  /** 截断前的文件总数 */
  total: number;
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
  root: NodeSummary;
  /** 根节点的直接子目录（更深层按需拉取） */
  rootChildren: NodeSummary[];
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
