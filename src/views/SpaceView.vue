<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, provide, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import DirTree from "../components/DirTree.vue";
import ReportModal from "../components/ReportModal.vue";
import { humanCount, humanDuration, humanSize } from "../lib/format";
import { FileSelectionKey } from "../lib/selection";
import type {
  CleanReport,
  DriveInfo,
  ScanCompletePayload,
  ScanProgressPayload,
} from "../types";

const scanRoot = ref("C:\\");
/** 发起扫描时冻结的根路径（结果树按它拼接路径，避免用户中途改输入框导致错乱） */
const scannedRoot = ref("");
const scanning = ref(false);
const progress = ref({ files: 0, bytes: 0 });
/** 扫描已用时长（进度行展示，纯前端计时） */
const elapsedMs = ref(0);
const result = ref<ScanCompletePayload | null>(null);
const error = ref("");
/** 用户主动取消后的提示（区别于错误，非红色） */
const cancelNotice = ref("");
/** 本机可用盘符（首屏快捷入口；固定盘/可移动盘才作为按钮展示） */
const drives = ref<DriveInfo[]>([]);

// ---------------------------------------------------------------------------
// 手动勾选删除：树里勾文件 → 底部选择栏 → 确认弹窗 → delete_files 命令
// ---------------------------------------------------------------------------

const selected = ref(new Map<string, number>());
provide(FileSelectionKey, {
  selected,
  toggle: (path, size, on) => {
    const next = new Map(selected.value);
    if (on) {
      next.set(path, size);
    } else {
      next.delete(path);
    }
    selected.value = next;
  },
  clear: () => {
    selected.value = new Map();
  },
});
function clearSelection() {
  selected.value = new Map();
}
const selectedBytes = computed(() =>
  [...selected.value.values()].reduce((sum, size) => sum + size, 0)
);
const selectedList = computed(() =>
  [...selected.value.entries()].map(([path, size]) => ({ path, size }))
);

const showDeleteConfirm = ref(false);
const deleteMode = ref<"trash" | "permanent">("trash");
const deleteDryRun = ref(false);
const deleteConfirmWord = ref("");
const deleteConfirmInput = ref<HTMLInputElement | null>(null);
const deleting = ref(false);
const deleteProgress = ref("");
let deleteClientId: string | null = null;
const showReport = ref(false);
const report = ref<CleanReport | null>(null);

/** 永久删除需要键入确认词（与垃圾清理确认弹窗同一安全默认） */
const needsDeleteWord = computed(
  () => deleteMode.value === "permanent" && !deleteDryRun.value
);
const canStartDelete = computed(
  () => !needsDeleteWord.value || deleteConfirmWord.value.trim() === "删除"
);

// 确认弹窗打开时聚焦确认词输入框
watch(showDeleteConfirm, (open) => {
  if (open) {
    void nextTick(() => deleteConfirmInput.value?.focus());
  }
});

/** Esc 关闭弹窗：删除确认 → 报告 */
function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  if (showDeleteConfirm.value) {
    showDeleteConfirm.value = false;
    return;
  }
  if (showReport.value && report.value) {
    showReport.value = false;
  }
}

function openDeleteConfirm() {
  deleteMode.value = "trash";
  deleteDryRun.value = false;
  deleteConfirmWord.value = "";
  showDeleteConfirm.value = true;
}

async function confirmDelete() {
  if (selected.value.size === 0 || deleting.value || !canStartDelete.value) return;
  showDeleteConfirm.value = false;
  const id =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  deleteClientId = id;
  deleting.value = true;
  deleteProgress.value = "";
  try {
    await invoke("delete_files", {
      clientId: id,
      files: [...selected.value.keys()],
      permanent: deleteMode.value === "permanent",
      dryRun: deleteDryRun.value,
    });
  } catch (err) {
    error.value = String(err);
    deleting.value = false;
  }
}

function cancelDeleting() {
  if (deleteClientId === null) return;
  invoke("cancel_scan", { clientId: deleteClientId }).catch(() => {});
}

/**
 * 当前扫描的前端关联 id。
 * 关键：在 invoke 之前生成，事件在 invoke 返回前到达也能正确关联
 * （2026-11-03 审查结论：否则小目录的 complete 事件会被丢弃，UI 永久卡"扫描中"）。
 */
let clientId: string | null = null;
const unlisteners: UnlistenFn[] = [];
/** 组件已卸载：阻止迟到的 listen 注册（卸载后 resolve 会泄漏监听器） */
let disposed = false;
/** 计时器句柄：扫描期间每 500ms 更新一次已用时长 */
let elapsedTimer: ReturnType<typeof setInterval> | undefined = undefined;

function stopTimer() {
  if (elapsedTimer !== undefined) {
    clearInterval(elapsedTimer);
    elapsedTimer = undefined;
  }
}

/** 注册事件监听；若注册完成时组件已卸载，立即注销，避免泄漏 */
async function onEvent<T>(event: string, handler: (payload: T) => void) {
  const unlisten = await listen<T>(event, (e) => {
    if (!disposed) handler(e.payload);
  });
  if (disposed) {
    unlisten();
  } else {
    unlisteners.push(unlisten);
  }
}

onMounted(async () => {
  window.addEventListener("keydown", onGlobalKeydown);
  // 盘符列表失败不影响主流程（快捷入口而已，用户仍可手输路径）
  try {
    drives.value = await invoke<DriveInfo[]>("list_drives");
  } catch {
    drives.value = [];
  }
  try {
    await onEvent<ScanProgressPayload>("scan://progress", (payload) => {
      if (payload.clientId !== clientId) return;
      progress.value = { files: payload.filesSeen, bytes: payload.bytesSeen };
    });
    await onEvent<ScanCompletePayload>("scan://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      result.value = payload;
      scanning.value = false;
      stopTimer();
    });
    await onEvent<{ clientId: string }>("scan://cancelled", (payload) => {
      if (payload.clientId !== clientId) return;
      scanning.value = false;
      stopTimer();
      cancelNotice.value = "扫描已取消";
    });
    await onEvent<{ clientId: string; message: string }>("scan://failed", (payload) => {
      if (payload.clientId !== clientId) return;
      error.value = payload.message;
      scanning.value = false;
      stopTimer();
    });
    await onEvent<{ clientId: string; currentTitle: string }>("clean://progress", (payload) => {
      if (payload.clientId !== deleteClientId) return;
      deleteProgress.value = payload.currentTitle;
    });
    await onEvent<{ clientId: string; report: CleanReport }>("clean://complete", (payload) => {
      if (payload.clientId !== deleteClientId) return;
      report.value = payload.report;
      showReport.value = true;
      deleting.value = false;
      selected.value = new Map();
    });
  } catch (err) {
    error.value = `事件监听注册失败：${err}`;
    console.error("listen 注册失败", err);
  }
});

onUnmounted(() => {
  disposed = true;
  window.removeEventListener("keydown", onGlobalKeydown);
  stopTimer();
  // KeepAlive 下切页签不会触发本钩子：扫描在后台继续，事件由仍存活的监听接收。
  // 只有应用整体退出（或组件真正销毁）才走到这里，此时取消以停止无用扫描。
  if (clientId !== null) {
    invoke("cancel_scan", { clientId }).catch(() => {});
  }
  unlisteners.forEach((u) => u());
});

/** 裸盘符 "C:" 归一化为 "C:\"：Win32 语义下前者按该盘当前目录解析，会静默扫错 */
function normalizeRoot(raw: string): string {
  const trimmed = raw.trim();
  return /^[a-zA-Z]:$/.test(trimmed) ? trimmed + "\\" : trimmed;
}

/** 快捷入口只展示本地固定盘与可移动盘（网络盘/光驱扫描慢或不适用） */
const driveButtons = computed(() =>
  drives.value.filter((d) => d.kind === "fixed" || d.kind === "removable")
);

function pickDrive(root: string) {
  scanRoot.value = root;
  startScan();
}

async function startScan() {
  if (scanning.value) return; // 防同帧双击重复发起（后端另有单飞兜底）
  error.value = "";
  cancelNotice.value = "";
  result.value = null;
  progress.value = { files: 0, bytes: 0 };
  elapsedMs.value = 0;
  const root = normalizeRoot(scanRoot.value);
  const id =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  clientId = id;
  scannedRoot.value = root;
  scanning.value = true;
  // 已用时长每 500ms 刷新（无总量百分比，但时长能区分"在推进"与"卡死"）
  const startedAt = Date.now();
  elapsedTimer = setInterval(() => {
    elapsedMs.value = Date.now() - startedAt;
  }, 500);
  try {
    await invoke("start_scan", { clientId: id, root });
  } catch (err) {
    error.value = String(err);
    scanning.value = false;
    stopTimer();
  }
}

function cancelScan() {
  if (clientId === null) return;
  invoke("cancel_scan", { clientId }).catch((err) => {
    error.value = `取消失败：${err}`;
  });
}
</script>

<template>
  <section class="view">
    <div class="toolbar">
      <input
        v-model="scanRoot"
        class="path-input"
        :disabled="scanning"
        placeholder="输入要分析的目录，如 C:\"
        @keydown.enter="startScan"
      />
      <button v-if="!scanning" class="primary" @click="startScan">开始扫描</button>
      <button v-else class="danger" @click="cancelScan">取消</button>
    </div>
    <p v-if="scanning" class="status">
      正在扫描… 已遍历 {{ humanCount(progress.files) }} 个文件（{{ humanSize(progress.bytes) }}）
      · 已用 {{ humanDuration(elapsedMs) }}
    </p>
    <p v-if="cancelNotice && !scanning" class="status">{{ cancelNotice }}</p>
    <p v-if="error" class="error">{{ error }}</p>

    <!-- 空态首屏：盘符快捷入口，替代原先一片空白 -->
    <div v-if="!result && !scanning" class="empty-hint">
      <div class="empty-text">
        <p class="empty-title">看看空间都去哪了</p>
        <p class="empty-sub">输入或选择要分析的目录，或点击下方盘符快速开始</p>
      </div>
      <div v-if="driveButtons.length > 0" class="drives">
        <button
          v-for="d in driveButtons"
          :key="d.root"
          class="drive-btn"
          :class="{ active: scanRoot.toUpperCase() === d.root.toUpperCase() }"
          @click="pickDrive(d.root)"
        >
          {{ d.root }}
        </button>
      </div>
    </div>

    <p v-if="deleting" class="status">
      正在删除… {{ deleteProgress }}
      <button class="ghost delete-cancel" @click="cancelDeleting">取消</button>
    </p>

    <div v-if="result" class="result">
      <!-- 结果英雄数字：占用空间为第一层级，文件数/跳过/链接点降为说明行 -->
      <div class="hero">
        <div class="hero-label">{{ scannedRoot }} · 空间占用</div>
        <div class="hero-number">{{ humanSize(result.bytesSeen) }}</div>
        <div class="hero-sub">
          共 {{ humanCount(result.filesSeen) }} 个文件<template v-if="result.skipped > 0">
            · 跳过 {{ humanCount(result.skipped) }} 项（无权限或读取失败）</template
          ><template v-if="result.junctions > 0">
            · 另有 {{ humanCount(result.junctions) }} 个链接/联接点未计入（防重复计数）</template
          >
        </div>
      </div>
      <div class="tree">
        <DirTree
          :node="result.root"
          :initial-children="result.rootChildren"
          :client-id="result.clientId"
          :dir-path="scannedRoot"
        />
      </div>
    </div>

    <!-- 手动删除选择栏：勾选文件后悬浮于底部 -->
    <div v-if="selected.size > 0 && !deleting" class="selection-bar">
      <span class="selection-text">
        已选 {{ humanCount(selected.size) }} 个文件（{{ humanSize(selectedBytes) }}）
      </span>
      <button class="ghost" @click="clearSelection">清空</button>
      <button class="danger" @click="openDeleteConfirm">删除所选</button>
    </div>

    <!-- 手动删除确认弹窗 -->
    <div v-if="showDeleteConfirm" class="overlay" @click.self="showDeleteConfirm = false">
      <div class="modal wide">
        <h3>删除所选文件</h3>
        <div class="delete-list">
          <div v-for="f in selectedList" :key="f.path" class="delete-row">
            <span class="delete-path" :title="f.path">{{ f.path }}</span>
            <span class="delete-size">{{ humanSize(f.size) }}</span>
          </div>
          <div v-if="selectedList.length > 50" class="detail-line">
            …共 {{ humanCount(selectedList.length) }} 个文件
          </div>
        </div>
        <p class="modal-line">
          共 <strong>{{ humanCount(selected.size) }}</strong> 个文件 ·
          <strong>{{ humanSize(selectedBytes) }}</strong>
        </p>
        <p class="detail-line option-explain">
          回收站：文件可随时还原，磁盘空间在清空回收站后才真正释放。永久删除：绕过回收站，
          事后无法恢复。系统/个人目录等受保护路径下的文件会被自动拒绝，并在报告中列出。
        </p>
        <div class="option-group">
          <label class="option">
            <input v-model="deleteMode" type="radio" value="trash" />
            移入回收站（推荐，可恢复）
          </label>
          <label class="option">
            <input v-model="deleteMode" type="radio" value="permanent" />
            永久删除（不可恢复）
          </label>
          <label class="option">
            <input v-model="deleteDryRun" type="checkbox" />
            先预演（只检查，不删除任何文件）
          </label>
        </div>
        <template v-if="needsDeleteWord">
          <p class="warn danger-note">
            ⚠ 永久删除将绕过回收站直接删除文件，事后无法通过任何方式恢复。
            请输入「删除」以确认执行。
          </p>
          <input
            ref="deleteConfirmInput"
            v-model="deleteConfirmWord"
            class="confirm-word"
            type="text"
            placeholder="输入：删除"
            @keydown.enter="canStartDelete && confirmDelete()"
          />
        </template>
        <div class="modal-actions">
          <button @click="showDeleteConfirm = false">取消</button>
          <button class="primary" :disabled="!canStartDelete" @click="confirmDelete">
            开始删除
          </button>
        </div>
      </div>
    </div>

    <!-- 手动删除报告（共享组件；关闭后选择已清空） -->
    <ReportModal
      v-if="showReport && report"
      :report="report"
      extra-note="提示：目录大小为扫描时的快照，删除不会自动更新数字；重新扫描可刷新。"
      @close="showReport = false"
    />
  </section>
</template>

<style scoped>
.view {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.toolbar {
  display: flex;
  gap: var(--space-2);
}

.path-input {
  flex: 1;
  max-width: 560px;
}

.status {
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

.error {
  color: var(--danger);
}

.result {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

/* 结果英雄数字：不加框，数据即版式（版式规则 1/4） */
.hero-label {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.hero-number {
  font-size: 34px; /* 视图内英雄数字，与 --font-display 同为展示级 */
  font-weight: 700;
  letter-spacing: -0.01em;
  font-variant-numeric: tabular-nums;
  color: var(--text);
  line-height: 1.1;
  margin: var(--space-1) 0;
}

.hero-sub {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

/* 空态首屏 */
.empty-hint {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--space-5);
  color: var(--text-secondary);
}

.empty-text {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  text-align: center;
}

.empty-title {
  font-size: var(--font-lg);
  font-weight: 600;
  color: var(--text);
}

.empty-sub {
  font-size: var(--font-sm);
  color: var(--text-secondary);
}

.drives {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
  justify-content: center;
}

.drive-btn {
  min-width: 96px;
  padding: var(--space-3) var(--space-5);
  background: var(--surface);
  border: 1px solid var(--border);
  font-weight: 500;
}

/* hover = accent-soft 淡底；选中（active）= 松绿描边 + 淡底（版式规则 5） */
.drive-btn:hover {
  background: var(--accent-soft);
  border-color: var(--border-strong);
  color: var(--text);
}

.drive-btn.active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-soft);
}

/* 手动删除 */
.ghost {
  background: transparent;
  border: 1px solid var(--border);
  padding: var(--space-1) var(--space-3);
}

/* 底部选择栏：真浮层才允许阴影（--shadow-2 悬浮条） */
.selection-bar {
  position: fixed;
  left: 50%;
  transform: translateX(-50%);
  bottom: var(--space-4);
  z-index: 40;
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-5);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-2);
}

.selection-text {
  font-size: var(--font-sm);
  font-weight: 500;
  font-variant-numeric: tabular-nums;
}

.delete-cancel {
  margin-left: var(--space-2);
  padding: var(--space-1) var(--space-3);
  font-size: var(--font-xs);
}

.overlay {
  position: fixed;
  inset: 0;
  background: var(--scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
}

.modal {
  width: 460px;
  max-width: calc(100vw - 48px);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
  padding: var(--space-5) var(--space-6);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
  box-shadow: var(--shadow-3);
}

.modal.wide {
  width: 620px;
}

.modal h3 {
  font-size: var(--font-lg);
}

.modal-line {
  color: var(--text-secondary);
}

.modal-line strong {
  color: var(--text);
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  margin-top: var(--space-2);
}

.modal-actions button {
  background: transparent;
  border: 1px solid var(--border);
}

.modal-actions button:hover {
  background: var(--accent-soft);
  border-color: var(--border-strong);
}

.modal-actions button.primary {
  background: var(--accent);
  border-color: transparent;
  color: var(--text-on-fill);
}

.modal-actions button.primary:hover {
  background: var(--accent-hover);
  border-color: transparent;
}

.option {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
}

/* 选项组：1px 细线分隔"删什么"与"怎么删"（版式规则 4） */
.option-group {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  border-top: 1px solid var(--border);
  padding-top: var(--space-3);
}

.option-explain {
  line-height: 1.6;
}

.detail-line {
  font-size: var(--font-xs);
  color: var(--text-secondary);
  word-break: break-all;
}

/* 警示：本视图仅有永久删除警示，红色块表达危险语义（版式规则 7） */
.warn {
  color: var(--danger);
  font-size: var(--font-xs);
}

.warn.danger-note {
  background: var(--danger-soft);
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
}

/* 确认词输入框样式全部交给全局 input（含聚焦软环） */

.delete-list {
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
  padding: var(--space-1) var(--space-3);
}

.delete-row {
  display: flex;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-1) 0;
  font-size: var(--font-sm);
}

.delete-path {
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.delete-size {
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

.tree {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: var(--space-2);
}
</style>
