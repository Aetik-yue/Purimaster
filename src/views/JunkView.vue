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

// 会话级缓存：切页回来直接显示上次结果，不重复扫描（清理时后端会重新枚举，显示值略旧是安全的）
let sessionCache: JunkScanResult | null = null;

const scanning = ref(false);
const cleaning = ref(false);
const result = ref<JunkScanResult | null>(sessionCache);
/** 当前展示的是后端缓存的历史结果（而非本次会话扫描） */
const stale = ref(false);
const error = ref("");
/** 展开的规则详情 */
const expanded = ref<Set<string>>(new Set());
/** 已勾选的规则 id（低风险默认勾选） */
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

/** 安全默认勾选：低风险自动勾上，中/高风险留给用户确认 */
function initSelection(r: JunkScanResult) {
  selected.value = new Set(
    r.groups
      .filter((g) => g.files > 0 && g.risk === "low")
      .map((g) => g.ruleId)
  );
}

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
      sessionCache = payload.result;
      stale.value = false;
      initSelection(payload.result);
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

  // 结果来源优先级：会话缓存 → 后端缓存（标记为历史结果）→ 自动扫描
  if (sessionCache !== null) {
    result.value = sessionCache;
    stale.value = false;
    initSelection(sessionCache);
  } else {
    try {
      const cached = await invoke<JunkScanResult | null>("get_junk_cache");
      if (cached && !disposed) {
        result.value = cached;
        sessionCache = cached;
        stale.value = true;
        initSelection(cached);
      }
    } catch {
      // 缓存读取失败则走自动扫描
    }
  }
  if (!disposed && result.value === null) startScan();
});

onUnmounted(() => {
  disposed = true;
  // 只取消空间扫描类任务；垃圾扫描只读且会写入后端缓存，切页不打断
  if (clientId !== null) invoke("cancel_scan", { clientId }).catch(() => {});
  unlisteners.forEach((u) => u());
});

async function startScan() {
  if (cleaning.value) return;
  error.value = "";
  result.value = null;
  sessionCache = null;
  stale.value = false;
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

function toggleSelect(group: JunkGroup) {
  if (group.files === 0 || cleaning.value) return;
  const next = new Set(selected.value);
  if (next.has(group.ruleId)) {
    next.delete(group.ruleId);
  } else {
    next.add(group.ruleId);
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

function toggleExpand(ruleId: string) {
  const next = new Set(expanded.value);
  if (next.has(ruleId)) {
    next.delete(ruleId);
  } else {
    next.add(ruleId);
  }
  expanded.value = next;
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
      <button v-else class="ghost" @click="startScan">重新扫描</button>
      <span v-if="cleaning" class="status">正在清理…</span>
    </div>

    <p v-if="scanning" class="status big-status">正在扫描垃圾文件…</p>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <!-- 主视觉：可清理空间 + 立即清理 -->
      <div class="hero">
        <div class="hero-main">
          <div class="hero-number">{{ humanSize(selectedBytes) }}</div>
          <div class="hero-sub">
            已选 {{ humanCount(selectedFiles) }} 个文件（共发现
            {{ humanSize(result.totalBytes) }}）<template v-if="stale">
              · 历史结果，可重新扫描</template
            >
          </div>
        </div>
        <button
          class="primary clean-now"
          :disabled="selectedFiles === 0 || cleaning || scanning"
          :title="selectedFiles === 0 ? '请至少勾选一项' : ''"
          @click="openConfirm"
        >
          立即清理
        </button>
      </div>

      <p v-if="result.totalFiles === 0 && !scanning" class="empty">
        未发现可清理的垃圾文件 ✓
      </p>

      <div class="list-hint">
        <label class="select-all">
          <input
            type="checkbox"
            :checked="allSelected"
            :disabled="selectableGroups.length === 0 || cleaning"
            @change="toggleSelectAll"
          />
          全选
        </label>
        <span class="hint-text">低风险项已自动勾选；点击行勾选/取消，点「详情」查看要删什么</span>
      </div>

      <div class="groups">
        <div v-for="cat in grouped" :key="cat.id" class="category">
          <h2 class="category-title">{{ cat.title }}</h2>

          <div
            v-for="g in cat.groups"
            :key="g.ruleId"
            class="group-row"
            :class="{ selected: selected.has(g.ruleId), dimmed: g.files === 0 }"
            @click="toggleSelect(g)"
          >
            <span class="check">
              <input
                v-if="g.files > 0"
                type="checkbox"
                :checked="selected.has(g.ruleId)"
                :disabled="cleaning"
                @click.stop
                @change="toggleSelect(g)"
              />
            </span>
            <div class="group-main">
              <div class="group-line">
                <span class="group-title">{{ g.title }}</span>
                <span class="badge" :class="'risk-' + g.risk">{{ riskLabel[g.risk] }}</span>
                <span v-if="g.problems.length > 0" class="problem-dot" title="存在无法读取的目标">⚠</span>
              </div>
              <div class="group-sub">{{ humanCount(g.files) }} 个文件</div>
            </div>
            <div class="group-size">{{ humanSize(g.bytes) }}</div>
            <button class="ghost detail-btn" @click.stop="toggleExpand(g.ruleId)">
              详情 {{ expanded.has(g.ruleId) ? "▴" : "▾" }}
            </button>
          </div>

          <div v-for="g in cat.groups" :key="g.ruleId + '-detail'">
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
              <div v-else-if="g.files === 0" class="detail-line muted">（无可清理文件，仅存在异常目标）</div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 确认弹窗 -->
    <div v-if="showConfirm" class="overlay">
      <div class="modal">
        <h3>确认清理</h3>
        <div class="confirm-list">
          <div v-for="g in selectedGroups" :key="g.ruleId" class="confirm-row">
            <span>{{ g.title }}</span>
            <span class="confirm-size">{{ humanSize(g.bytes) }}</span>
          </div>
          <div v-if="selectedGroups.length > 6" class="detail-line muted">
            …等 {{ selectedGroups.length }} 个项目
          </div>
        </div>
        <p class="modal-line">
          共 <strong>{{ humanCount(selectedFiles) }}</strong> 个文件 ·
          <strong>{{ humanSize(selectedBytes) }}</strong>
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
  flex-shrink: 0;
}

.ghost {
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text);
}

.status {
  color: var(--accent);
}

.big-status {
  font-size: 15px;
}

.error {
  color: var(--danger);
}

.result {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

/* 主视觉区 */
.hero {
  display: flex;
  align-items: center;
  gap: 24px;
  padding: 18px 24px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 12px;
}

.hero-main {
  margin-right: auto;
}

.hero-number {
  font-size: 34px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  color: var(--text);
  line-height: 1.15;
}

.hero-sub {
  font-size: 12px;
  color: var(--text-secondary);
}

.clean-now {
  font-size: 16px;
  padding: 12px 34px;
  border-radius: 10px;
}

.clean-now:disabled {
  opacity: 0.45;
}

.empty {
  color: #1a9e55;
  font-size: 15px;
  padding: 24px;
  text-align: center;
}

.list-hint {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-shrink: 0;
}

.select-all {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  user-select: none;
  font-size: 13px;
}

.hint-text {
  font-size: 12px;
  color: var(--text-secondary);
}

.groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 10px 14px;
}

.category {
  margin-bottom: 14px;
}

.category-title {
  font-size: 13px;
  color: var(--text-secondary);
  margin: 8px 0 4px;
}

.group-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
  user-select: none;
  border: 1px solid transparent;
}

.group-row:hover {
  background: rgba(47, 111, 237, 0.06);
}

.group-row.selected {
  background: rgba(47, 111, 237, 0.08);
  border-color: rgba(47, 111, 237, 0.25);
}

.group-row.dimmed {
  opacity: 0.55;
}

.check {
  display: flex;
  align-items: center;
}

.group-main {
  min-width: 0;
}

.group-line {
  display: flex;
  align-items: center;
  gap: 8px;
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

.problem-dot {
  color: #b45309;
  font-size: 12px;
}

.group-sub {
  font-size: 12px;
  color: var(--text-secondary);
}

.group-size {
  margin-left: auto;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

.detail-btn {
  flex-shrink: 0;
  background: transparent;
  border: none;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 4px 8px;
}

.detail-btn:hover {
  color: var(--accent);
}

.group-detail {
  padding: 6px 12px 10px 40px;
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
  width: 460px;
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

.confirm-list {
  max-height: 180px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 6px 10px;
}

.confirm-row {
  display: flex;
  justify-content: space-between;
  padding: 3px 0;
  font-size: 13px;
}

.confirm-size {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
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
