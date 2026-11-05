<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { FileEntry, FileListing, NodeSummary } from "../types";
import { humanCount, humanSize } from "../lib/format";
import { FileSelectionKey, type FileSelection } from "../lib/selection";

/** 单个目录最多渲染多少行子目录（更多折叠为提示） */
const MAX_CHILD_ROWS = 100;
/** list_files 返回条数上限（与后端默认一致） */
const FILE_LIMIT = 200;

// 递归组件：文件名即组件名（Vue 3.3+ 隐式自引用）
const props = defineProps<{
  node: NodeSummary;
  /** 扫描会话 id（与扫描结果索引匹配） */
  clientId: string;
  /** 本目录的完整路径（逐级拼接） */
  dirPath: string;
  depth?: number;
  /** 根节点随 complete 事件一起到达的子目录 */
  initialChildren?: NodeSummary[];
}>();

const depth = computed(() => props.depth ?? 0);
const expanded = ref(depth.value < 1);

const children = ref<NodeSummary[] | null>(props.initialChildren ?? null);
const loadingChildren = ref(false);
const childrenError = ref("");
/** 当前展示的子目录条数上限（"显示更多"分段放开，后端返回的是全量） */
const childLimit = ref(MAX_CHILD_ROWS);

const files = ref<FileListing | null>(null);
const loadingFiles = ref(false);
const filesError = ref("");

const visibleChildren = computed(() => children.value?.slice(0, childLimit.value) ?? []);
const omittedChildren = computed(
  () => (children.value?.length ?? 0) - visibleChildren.value.length
);
const omittedFiles = computed(
  () => (files.value ? files.value.total - files.value.files.length : 0)
);

/** 手动勾选删除的状态（SpaceView 提供；缺省空实现保证组件可独立渲染） */
const noSelection: FileSelection = {
  selected: ref(new Map<string, number>()),
  toggle: () => {},
  clear: () => {},
};
const selection = inject(FileSelectionKey, noSelection);

/** 子目录完整路径 */
function childPath(name: string): string {
  return props.dirPath.endsWith("\\") || props.dirPath.endsWith("/")
    ? props.dirPath + name
    : props.dirPath + "\\" + name;
}

/** 勾选/取消一个待删除文件（勾选状态集中存放在 SpaceView） */
function onFileCheck(f: FileEntry, ev: Event) {
  const checked = (ev.target as HTMLInputElement).checked;
  selection.toggle(childPath(f.name), f.size, checked);
}

async function loadChildren() {
  if (!props.node.hasChildren || children.value !== null || loadingChildren.value) return;
  loadingChildren.value = true;
  childrenError.value = "";
  try {
    children.value = await invoke<NodeSummary[]>("dir_children", {
      clientId: props.clientId,
      nodeId: props.node.id,
    });
  } catch (err) {
    childrenError.value = String(err);
  } finally {
    loadingChildren.value = false;
  }
}

async function loadFiles() {
  if (files.value !== null || loadingFiles.value) return;
  loadingFiles.value = true;
  filesError.value = "";
  try {
    files.value = await invoke<FileListing>("list_files", {
      dir: props.dirPath,
      limit: FILE_LIMIT,
      offset: 0,
    });
  } catch (err) {
    filesError.value = String(err);
  } finally {
    loadingFiles.value = false;
  }
}

/** "显示更多文件"：按已加载数作为 offset 追加下一页（后端按大小降序全量排序） */
const loadingMoreFiles = ref(false);
async function loadMoreFiles() {
  if (files.value === null || loadingMoreFiles.value) return;
  loadingMoreFiles.value = true;
  filesError.value = "";
  try {
    const more = await invoke<FileListing>("list_files", {
      dir: props.dirPath,
      limit: FILE_LIMIT,
      offset: files.value.files.length,
    });
    files.value = { files: [...files.value.files, ...more.files], total: more.total };
  } catch (err) {
    filesError.value = String(err);
  } finally {
    loadingMoreFiles.value = false;
  }
}

function toggle() {
  expanded.value = !expanded.value;
  if (expanded.value) {
    loadChildren();
    loadFiles();
  }
}

/** 截断提示处"显示更多"：每次多放开一页（后端 children() 返回全量，纯前端分段） */
function showMoreChildren() {
  childLimit.value += MAX_CHILD_ROWS;
}

// 根节点默认展开，启动时即加载
onMounted(() => {
  if (expanded.value) {
    loadChildren();
    loadFiles();
  }
});
</script>

<template>
  <div>
    <div class="row" :style="{ paddingLeft: depth * 20 + 8 + 'px' }" @click="toggle">
      <span class="caret" :class="{ open: expanded, hidden: !node.hasChildren }">▸</span>
      <span class="name" :title="dirPath">
        {{ node.name }}
        <span v-if="node.unreadable" class="badge" title="无权限读取，大小未知">无权限</span>
        <span
          v-else-if="node.unknownSubtree"
          class="badge"
          title="子树内存在无权限目录，其大小未计入，实际占用可能更大"
          >含未知</span
        >
      </span>
      <span class="meta">{{ humanCount(node.files) }} 文件 · {{ humanCount(node.dirs) }} 目录</span>
      <span class="size">{{ node.unreadable ? "未知" : humanSize(node.size) }}</span>
    </div>

    <div v-if="expanded">
      <!-- 子目录（懒加载） -->
      <div
        v-if="loadingChildren"
        class="file-note"
        :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
      >
        加载子目录…
      </div>
      <div
        v-else-if="childrenError"
        class="file-note error"
        :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
      >
        {{ childrenError }}
      </div>
      <template v-else>
        <DirTree
          v-for="child in visibleChildren"
          :key="child.id"
          :node="child"
          :client-id="clientId"
          :dir-path="childPath(child.name)"
          :depth="depth + 1"
        />
        <div
          v-if="omittedChildren > 0"
          class="file-note"
          :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
        >
          <span>…按大小降序，还有 {{ humanCount(omittedChildren) }} 个子目录未显示</span>
          <button class="show-more" @click.stop="showMoreChildren">显示更多</button>
        </div>
      </template>

      <!-- 直属文件明细（懒加载） -->
      <div
        v-if="loadingFiles"
        class="file-note"
        :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
      >
        加载文件清单…
      </div>
      <div
        v-else-if="filesError"
        class="file-note error"
        :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
      >
        读取失败：{{ filesError }}
      </div>
      <template v-else-if="files">
        <div
          v-if="files.files.length > 0"
          class="file-note muted"
          :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
        >
          （实时读取，可能与扫描时的目录大小快照不同步）
        </div>
        <div
          v-for="f in files.files"
          :key="f.name"
          class="row file-row"
          :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
        >
          <input
            type="checkbox"
            class="file-check"
            :checked="selection.selected.value.has(childPath(f.name))"
            :title="selection.selected.value.has(childPath(f.name)) ? '取消勾选' : '勾选后在底部删除所选'"
            @click.stop
            @change="onFileCheck(f, $event)"
          />
          <span class="name" :title="f.name">{{ f.name }}</span>
          <span class="size">{{ humanSize(f.size) }}</span>
        </div>
        <div
          v-if="omittedFiles > 0"
          class="file-note"
          :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
        >
          <span>…还有 {{ humanCount(omittedFiles) }} 个更小的文件未显示</span>
          <button class="show-more" :disabled="loadingMoreFiles" @click.stop="loadMoreFiles">
            {{ loadingMoreFiles ? "加载中…" : "显示更多文件" }}
          </button>
        </div>
        <div
          v-else-if="files.files.length === 0"
          class="file-note"
          :style="{ paddingLeft: (depth + 1) * 20 + 8 + 'px' }"
        >
          （无直属文件）
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 32px;
  padding-right: 12px;
  border-radius: 6px;
  cursor: pointer;
  user-select: none;
}

.row:hover {
  background: rgba(47, 111, 237, 0.06);
}

.caret {
  width: 14px;
  text-align: center;
  color: var(--text-secondary);
  transition: transform 0.12s;
  flex-shrink: 0;
}

.caret.open {
  transform: rotate(90deg);
}

.caret.hidden {
  visibility: hidden;
}

.name {
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-row .name {
  font-weight: 400;
  color: var(--text-secondary);
}

.file-row {
  cursor: default;
}

.file-check {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  cursor: pointer;
  accent-color: var(--accent);
}

.badge {
  display: inline-block;
  margin-left: 6px;
  padding: 1px 6px;
  font-size: 11px;
  font-weight: 400;
  color: #b45309;
  background: #fef3c7;
  border-radius: 4px;
}

.meta {
  margin-left: auto;
  color: var(--text-secondary);
  font-size: 12px;
  flex-shrink: 0;
}

.size {
  width: 88px;
  text-align: right;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

.file-note {
  height: 28px;
  display: flex;
  align-items: center;
  color: var(--text-secondary);
  font-size: 12px;
}

.file-note.error {
  color: var(--danger);
}

.file-note.muted {
  color: var(--text-secondary);
  opacity: 0.75;
}

.show-more {
  margin-left: 8px;
  padding: 1px 8px;
  font-size: 12px;
  background: transparent;
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--accent);
  cursor: pointer;
}

.show-more:hover {
  border-color: var(--accent);
}
</style>
