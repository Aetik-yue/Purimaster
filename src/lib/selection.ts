import type { InjectionKey, Ref } from "vue";

/**
 * 空间分析的手动文件勾选状态（跨递归树节点共享，由 SpaceView 提供）。
 *
 * 只保存"勾了什么"，删除动作由 SpaceView 统一发起——树节点只负责勾选，
 * 不各自调删除，保证删除入口仍然单点。
 */
export interface FileSelection {
  /** 完整路径 → 文件大小（字节） */
  selected: Ref<Map<string, number>>;
  toggle: (path: string, size: number, on: boolean) => void;
  clear: () => void;
}

export const FileSelectionKey: InjectionKey<FileSelection> = Symbol("file-selection");
