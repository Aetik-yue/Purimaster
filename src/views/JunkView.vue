<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { humanCount, humanSize } from "../lib/format";
import type {
  CleanCompletePayload,
  CleanReport,
  JunkCompletePayload,
  JunkGroup,
  JunkScanResult,
  Risk,
} from "../types";

const scanning = ref(false);
const cleaning = ref(false);
const result = ref<JunkScanResult | null>(null);
const error = ref("");
const expanded = ref<Set<string>>(new Set());
/** 已勾选的规则 id */
const selected = ref<Set<string>>(new Set());

// 确认弹窗状态
const showConfirm = ref(false);
const mode = ref<"trash" | "permanent">("trash");
const dryRun = ref(false);

// 清理报告
const report = ref<CleanReport | null>(null);
const showReport = ref(false);

let clientId: string | null = null;
let cleanClientId: string | null = null;
const unlisteners: UnlistenFn[] = [];
/** 组件已卸载：阻止迟到的 listen 注册与自动扫描 */
let disposed = false;

const riskLabel: Record<Risk, string> = {
  low: "低风险",
  medium: "中风险",
  high: "高风险",
};

/** 按分类分组，只显示有文件或问题的规则 */
const grouped = computed(() => {
  const r = result.value;
  if (!r) return [];
  return Object.entries(r.categories)
    .map(([id, title]) => ({
      id,
      title,
      groups: r.groups.filter(
        (g) => g.category === id && (g.files > 0 || g.problems.length > 0)
      ),
    }))
    .filter((c) => c.groups.length > 0);
});

const selectableGroups = computed(
  () => result.value?.groups.filter((g) => g.files > 0) ?? []
);
const allSelected = computed(
  () =>
    selectableGroups.value.length > 0 &&
    selectableGroups.value.every((g) => selected.value.has(g.ruleId))
);
const selectedGroups = computed(
  () => result.value?.groups.filter((g) => selected.value.has(g.ruleId)) ?? []
);
const selectedFiles = computed(() =>
  selectedGroups.value.reduce((s, g) => s + g.files, 0)
);
const selectedBytes = computed(() =>
  selectedGroups.value.reduce((s, g) => s + g.bytes, 0)
);

function genId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
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
  try {
    await onEvent<JunkCompletePayload>("junk://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      result.value = payload.result;
      scanning.value = false;
    });
    await onEvent<{ clientId: string }>("junk://cancelled", (payload) => {
      if (payload.clientId !== clientId) return;
      scanning.value = false;
    });
    await onEvent<CleanCompletePayload>("clean://complete", (payload) => {
      if (payload.clientId !== cleanClientId) return;
      report.value = payload.report;
      showReport.value = true;
      cleaning.value = false;
      selected.value = new Set();
    });
  } catch (err) {
    error.value = `事件监听注册失败：${err}`;
    return;
  }
  // 进入页面自动扫描一次（组件已卸载则跳过）
  if (disposed) return;

  // 清理在切页后仍会完成：取回上次未查看的报告，避免逐文件结果丢失
  try {
    const pending = await invoke<CleanReport | null>("take_clean_report");
    if (pending && !disposed) {
      report.value = pending;
      showReport.value = true;
    }
  } catch {
    // 报告恢复失败不影响主流程
  }
  if (!disposed) startScan();
});

onUnmounted(() => {
  disposed = true;
  // 只取消扫描；已确认的清理**不**取消（用户已同意删除，中途停止只会留下部分删除），
  // 报告由 take_clean_report 在下次进入页面时取回
  if (clientId !== null) invoke("cancel_scan", { clientId }).catch(() => {});
  unlisteners.forEach((u) => u());
});

async function startScan() {
  if (cleaning.value) return;
  error.value = "";
  result.value = null;
  expanded.value = new Set();
  selected.value = new Set();
  const id = genId();
  clientId = id;
  scanning.value = true;
  try {
    await invoke("scan_junk", { clientId: id });
  } catch (err) {
    error.value = String(err);
    scanning.value = false;
  }
}

function cancelScan() {
  if (clientId === null) return;
  invoke("cancel_scan", { clientId }).catch((err) => {
    error.value = `取消失败：${err}`;
  });
}

function toggleGroup(ruleId: string) {
  const next = new Set(expanded.value);
  if (next.has(ruleId)) {
    next.delete(ruleId);
  } else {
    next.add(ruleId);
  }
  expanded.value = next;
}

function toggleSelect(ruleId: string) {
  const next = new Set(selected.value);
  if (next.has(ruleId)) {
    next.delete(ruleId);
  } else {
    next.add(ruleId);
  }
  selected.value = next;
}

function toggleSelectAll() {
  if (allSelected.value) {
    selected.value = new Set();
  } else {
    selected.value = new Set(selectableGroups.value.map((g) => g.ruleId));
  }
}

/** 打开确认弹窗：每次都重置为安全默认（回收站、非试运行） */
function openConfirm() {
  mode.value = "trash";
  dryRun.value = false;
  showConfirm.value = true;
}

async function confirmClean() {
  if (selected.value.size === 0 || cleaning.value) return;
  showConfirm.value = false;
  error.value = "";
  report.value = null;
  const id = genId();
  cleanClientId = id;
  cleaning.value = true;
  try {
    await invoke("clean_junk", {
      clientId: id,
      ruleIds: [...selected.value],
      permanent: mode.value === "permanent",
      dryRun: dryRun.value,
    });
  } catch (err) {
    error.value = String(err);
    cleaning.value = false;
  }
}

function cancelCleaning() {
  if (cleanClientId === null) return;
  invoke("cancel_scan", { clientId: cleanClientId }).catch(() => {});
}

function closeReport() {
  showReport.value = false;
  report.value = null;
  startScan();
}

function groupTargetsSummary(group: JunkGroup): string {
  return group.targets.length > 0 ? group.targets.join("；") : "（目标不存在）";
}
</script>

<template>
  <section class="view">
    <div class="toolbar">
      <button v-if="scanning" class="danger" @click="cancelScan">取消扫描</button>
      <button v-else-if="cleaning" class="danger" @click="cancelCleaning">取消清理</button>
      <button v-else class="primary" @click="startScan">重新扫描</button>
      <span v-if="cleaning" class="status">正在清理…</span>
    </div>

    <p v-if="scanning" class="status">正在扫描垃圾文件…</p>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <p class="summary">
        共发现 <strong>{{ humanCount(result.totalFiles) }}</strong> 个可清理文件，合计
        <strong>{{ humanSize(result.totalBytes) }}</strong>
        <span v-if="result.skipped > 0"> · 跳过 {{ humanCount(result.skipped) }} 项</span>
      </p>

      <p v-if="result.totalFiles === 0 && !scanning" class="empty">未发现可清理的垃圾文件 ✓</p>

      <div class="selection-bar">
        <label class="select-all">
          <input
            type="checkbox"
            :checked="allSelected"
            :disabled="selectableGroups.length === 0 || cleaning"
            @change="toggleSelectAll"
          />
          全选
        </label>
        <span class="selection-summary">
          已选 {{ humanCount(selectedFiles) }} 个文件 · {{ humanSize(selectedBytes) }}
        </span>
        <button
          class="primary"
          :disabled="selectedFiles === 0 || cleaning || scanning"
          @click="openConfirm"
        >
          清理选中
        </button>
      </div>

      <div class="groups">
        <div v-for="cat in grouped" :key="cat.id" class="category">
          <h2 class="category-title">
            {{ cat.title }}
            <span class="category-total">
              {{ humanCount(cat.groups.reduce((s, g) => s + g.files, 0)) }} 文件 ·
              {{ humanSize(cat.groups.reduce((s, g) => s + g.bytes, 0)) }}
            </span>
          </h2>

          <div v-for="g in cat.groups" :key="g.ruleId" class="group">
            <div class="group-row" @click="toggleGroup(g.ruleId)">
              <span class="check" @click.stop>
                <input
                  v-if="g.files > 0"
                  type="checkbox"
                  :checked="selected.has(g.ruleId)"
                  :disabled="cleaning"
                  @change="toggleSelect(g.ruleId)"
                />
              </span>
              <span class="caret" :class="{ open: expanded.has(g.ruleId) }">▸</span>
              <span class="group-title">{{ g.title }}</span>
              <span class="badge" :class="'risk-' + g.risk">{{ riskLabel[g.risk] }}</span>
              <span class="group-meta">
                {{ humanCount(g.files) }} 文件 · {{ humanSize(g.bytes) }}
              </span>
            </div>

            <div v-if="expanded.has(g.ruleId)" class="group-detail">
              <div class="detail-line"><span class="detail-label">目标</span>{{ groupTargetsSummary(g) }}</div>
              <div v-for="p in g.problems" :key="p" class="detail-line problem">⚠ {{ p }}</div>
              <div v-if="g.topFiles.length > 0" class="files">
                <div v-for="f in g.topFiles" :key="f.path" class="file-row" :title="f.path">
                  <span class="file-path">{{ f.path }}</span>
                  <span class="file-size">{{ humanSize(f.size) }}</span>
                </div>
                <p v-if="g.files > g.topFiles.length" class="detail-line muted">
                  仅预览最大的 {{ g.topFiles.length }} 个文件，共 {{ humanCount(g.files) }} 个
                </p>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 确认弹窗 -->
    <div v-if="showConfirm" class="overlay">
      <div class="modal">
        <h3>确认清理</h3>
        <p class="modal-line">
          将清理 <strong>{{ humanCount(selectedFiles) }}</strong> 个文件，共
          <strong>{{ humanSize(selectedBytes) }}</strong>。
        </p>
        <label class="option">
          <input v-model="mode" type="radio" value="trash" />
          移入回收站（推荐，可恢复）
        </label>
        <label class="option">
          <input v-model="mode" type="radio" value="permanent" />
          永久删除（不可恢复）
        </label>
        <label class="option">
          <input v-model="dryRun" type="checkbox" />
          仅试运行（执行全部安全检查但不删除）
        </label>
        <p v-if="mode === 'permanent' && !dryRun" class="warn">
          ⚠ 永久删除的文件无法从回收站恢复。请确认这些内容确实可以丢弃。
        </p>
        <div class="modal-actions">
          <button @click="showConfirm = false">取消</button>
          <button class="primary" @click="confirmClean">开始清理</button>
        </div>
      </div>
    </div>

    <!-- 清理报告 -->
    <div v-if="showReport && report" class="overlay">
      <div class="modal wide">
        <h3>
          {{ report.dryRun ? "试运行完成" : "清理完成" }}
          <span v-if="report.cancelled" class="cancel-tag">（已取消，以下为已完成部分）</span>
        </h3>
        <p class="modal-line">
          {{ report.dryRun ? "将" : "已" }}{{ report.mode === "trash" ? "移入回收站" : "永久删除" }}
          <strong>{{ humanCount(report.deletedFiles) }}</strong> 个文件<template
            v-if="report.mode === 'trash'"
            >，清空回收站后释放 <strong>{{ humanSize(report.freedBytes) }}</strong></template
          ><template v-else
            >，{{ report.dryRun ? "可释放" : "释放" }}
            <strong>{{ humanSize(report.freedBytes) }}</strong></template
          >。
        </p>
        <p v-if="report.skipped > 0" class="detail-line muted">
          另有 {{ humanCount(report.skipped) }} 个条目因读取失败或目标异常被跳过。
        </p>
        <p v-if="report.failedCount > 0" class="warn">
          失败 {{ humanCount(report.failedCount) }} 个文件（如被占用、只读或权限不足）：
        </p>
        <div v-if="report.failed.length" class="failures">
          <div v-for="f in report.failed" :key="f.path" class="fail-row">
            <span class="fail-path" :title="f.path">{{ f.path }}</span>
            <span class="fail-err">{{ f.error }}</span>
          </div>
        </div>
        <details v-if="report.skippedRules.length" class="skipped">
          <summary>跳过的规则/目标（{{ report.skippedRules.length }}）</summary>
          <div v-for="s in report.skippedRules" :key="s" class="detail-line">{{ s }}</div>
        </details>
        <div class="modal-actions">
          <button class="primary" @click="closeReport">好的</button>
        </div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.view {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
}

.status {
  color: var(--accent);
}

.error {
  color: var(--danger);
}

.result {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.summary {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.summary strong {
  color: var(--text);
}

.empty {
  color: #1a9e55;
}

.selection-bar {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 8px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.select-all {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  user-select: none;
}

.selection-summary {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.selection-bar button {
  margin-left: auto;
}

.groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 12px 16px;
}

.category {
  margin-bottom: 18px;
}

.category-title {
  font-size: 14px;
  margin-bottom: 6px;
  display: flex;
  align-items: baseline;
  gap: 10px;
}

.category-total {
  font-size: 12px;
  font-weight: 400;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.group-row {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 34px;
  padding: 0 8px;
  border-radius: 6px;
  cursor: pointer;
  user-select: none;
}

.group-row:hover {
  background: rgba(47, 111, 237, 0.06);
}

.check {
  display: flex;
  align-items: center;
}

.caret {
  width: 14px;
  text-align: center;
  color: var(--text-secondary);
  transition: transform 0.12s;
}

.caret.open {
  transform: rotate(90deg);
}

.group-title {
  font-weight: 500;
}

.badge {
  padding: 1px 6px;
  font-size: 11px;
  border-radius: 4px;
}

.risk-low {
  color: #1a7f47;
  background: #e6f6ec;
}

.risk-medium {
  color: #b45309;
  background: #fef3c7;
}

.risk-high {
  color: #b91c1c;
  background: #fee2e2;
}

.group-meta {
  margin-left: auto;
  font-size: 12px;
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.group-detail {
  padding: 4px 8px 8px 30px;
}

.detail-line {
  font-size: 12px;
  color: var(--text-secondary);
  padding: 2px 0;
  word-break: break-all;
}

.detail-line.problem {
  color: #b45309;
}

.detail-line.muted {
  font-size: 11px;
}

.detail-label {
  display: inline-block;
  margin-right: 8px;
  color: var(--text);
}

.files {
  margin-top: 6px;
  border-top: 1px dashed var(--border);
  padding-top: 6px;
}

.file-row {
  display: flex;
  gap: 12px;
  padding: 2px 0;
  font-size: 12px;
}

.file-path {
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-size {
  margin-left: auto;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
}

.overlay {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
}

.modal {
  width: 440px;
  max-width: calc(100vw - 48px);
  background: var(--surface);
  border-radius: 10px;
  padding: 20px 22px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 12px 40px rgba(15, 23, 42, 0.25);
}

.modal.wide {
  width: 620px;
}

.modal h3 {
  font-size: 16px;
}

.cancel-tag {
  font-size: 12px;
  font-weight: 400;
  color: #b45309;
  margin-left: 6px;
}

.modal-line {
  color: var(--text-secondary);
}

.option {
  display: flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
}

.warn {
  color: #b91c1c;
  font-size: 12px;
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 6px;
}

.modal-actions button {
  background: transparent;
  border: 1px solid var(--border);
}

.modal-actions button.primary {
  background: var(--accent);
  border-color: transparent;
  color: #fff;
}

.failures {
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.fail-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 12px;
}

.fail-path {
  color: var(--text-secondary);
  word-break: break-all;
}

.fail-err {
  color: #b91c1c;
}

.skipped {
  font-size: 12px;
  color: var(--text-secondary);
}

.skipped summary {
  cursor: pointer;
}
</style>
