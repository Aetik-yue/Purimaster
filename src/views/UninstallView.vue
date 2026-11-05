<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import ReportModal from "../components/ReportModal.vue";
import { humanCount, humanSize } from "../lib/format";
import type {
  CleanReport,
  InstalledProgram,
  ResidueReport,
  UninstallCompletePayload,
  UninstallProgressPayload,
} from "../types";

const programs = ref<InstalledProgram[]>([]);
const loading = ref(false);
const error = ref("");
const filter = ref("");

/** 已勾选的软件（registryKey → 程序） */
const selected = ref(new Map<string, InstalledProgram>());

// 卸载流程
const showConfirm = ref(false);
const running = ref(false);
const progressRows = ref<{ name: string; status: string }[]>([]);
const results = ref<{ name: string; ok: boolean; detail: string }[]>([]);

// 残留（卸载完成后自动扫描，逐项勾选清除）
const residues = ref<ResidueReport[]>([]);
const residueChecked = ref(new Set<string>());
const scanningResidue = ref(false);
const deletingResidue = ref(false);
const showReport = ref(false);
const report = ref<CleanReport | null>(null);

let clientId: string | null = null;
let lastUninstalled: InstalledProgram[] = [];
const unlisteners: UnlistenFn[] = [];
let disposed = false;

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

const filtered = computed(() => {
  const f = filter.value.trim().toLowerCase();
  if (!f) return programs.value;
  return programs.value.filter(
    (p) =>
      p.displayName.toLowerCase().includes(f) ||
      p.publisher.toLowerCase().includes(f)
  );
});

const selectedPrograms = computed(() => [...selected.value.values()]);

function isSelected(p: InstalledProgram): boolean {
  return selected.value.has(p.registryKey);
}

function toggleSelect(p: InstalledProgram) {
  const next = new Map(selected.value);
  if (next.has(p.registryKey)) {
    next.delete(p.registryKey);
  } else {
    next.set(p.registryKey, p);
  }
  selected.value = next;
}

function allSelected(): boolean {
  return (
    filtered.value.length > 0 &&
    filtered.value.every((p) => selected.value.has(p.registryKey))
  );
}

function toggleSelectAll() {
  if (allSelected()) {
    selected.value = new Map();
  } else {
    const next = new Map(selected.value);
    for (const p of filtered.value) {
      next.set(p.registryKey, p);
    }
    selected.value = next;
  }
}

function genId(): string {
  return typeof crypto.randomUUID === "function"
    ? crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function sizeLabel(p: InstalledProgram): string {
  return p.estimatedSizeKb > 0 ? humanSize(p.estimatedSizeKb * 1024) : "未知";
}

async function load() {
  loading.value = true;
  error.value = "";
  try {
    programs.value = await invoke<InstalledProgram[]>("list_programs");
  } catch (err) {
    error.value = String(err);
  } finally {
    loading.value = false;
  }
}

function openConfirm() {
  showConfirm.value = true;
}

async function confirmUninstall() {
  if (selectedPrograms.value.length === 0 || running.value) return;
  showConfirm.value = false;
  error.value = "";
  results.value = [];
  residues.value = [];
  residueChecked.value = new Set();
  progressRows.value = selectedPrograms.value.map((p) => ({
    name: p.displayName,
    status: "排队中",
  }));
  lastUninstalled = selectedPrograms.value;
  running.value = true;
  const id = genId();
  clientId = id;
  try {
    await invoke("uninstall_programs", {
      clientId: id,
      programs: selectedPrograms.value,
    });
  } catch (err) {
    error.value = String(err);
    running.value = false;
  }
}

/** 卸载完成后对成功项自动扫描残留 */
async function scanResidues() {
  scanningResidue.value = true;
  try {
    for (const prog of lastUninstalled) {
      const r = await invoke<ResidueReport>("scan_residue", { program: prog });
      if (r.files.length > 0 || r.registry.length > 0) {
        residues.value.push(r);
        for (const f of r.files) residueChecked.value.add(`F|${f.path}`);
        for (const k of r.registry) residueChecked.value.add(`R|${k}`);
      }
    }
  } catch (err) {
    error.value = String(err);
  } finally {
    scanningResidue.value = false;
  }
}

const hasResidues = computed(() =>
  residues.value.some((r) => r.files.length > 0 || r.registry.length > 0)
);

function residueFilesChecked(): string[] {
  return [...residueChecked.value]
    .filter((id) => id.startsWith("F|"))
    .map((id) => id.slice(2));
}

function residueRegistryChecked(): string[] {
  return [...residueChecked.value]
    .filter((id) => id.startsWith("R|"))
    .map((id) => id.slice(2));
}

async function deleteResidue() {
  const files = residueFilesChecked();
  const registry = residueRegistryChecked();
  if (files.length === 0 && registry.length === 0) return;
  deletingResidue.value = true;
  const id = genId();
  clientId = id;
  try {
    await invoke("delete_residue", { clientId: id, files, registry, dryRun: false });
  } catch (err) {
    error.value = String(err);
    deletingResidue.value = false;
  }
}

function closeReport() {
  showReport.value = false;
  report.value = null;
  // 已删除的残留从面板移除；列表刷新以反映卸载结果
  residues.value = residues.value.filter(
    (r) =>
      r.files.some((f) => residueChecked.value.has(`F|${f.path}`)) ||
      r.registry.some((k) => residueChecked.value.has(`R|${k}`))
  );
  void load();
}

onMounted(async () => {
  try {
    await onEvent<UninstallProgressPayload>("uninstall://progress", (payload) => {
      if (payload.clientId !== clientId) return;
      const row = progressRows.value[payload.index - 1];
      if (row) row.status = payload.status;
    });
    await onEvent<UninstallCompletePayload>("uninstall://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      results.value = payload.results;
      running.value = false;
      selected.value = new Map();
      void scanResidues();
    });
    await onEvent<{ clientId: string; report: CleanReport }>("clean://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      report.value = payload.report;
      showReport.value = true;
      deletingResidue.value = false;
    });
  } catch (err) {
    error.value = `事件监听注册失败：${err}`;
  }
  if (disposed) return;
  await load();
});

onUnmounted(() => {
  disposed = true;
  unlisteners.forEach((u) => u());
});
</script>

<template>
  <section class="view">
    <div class="toolbar">
      <button class="ghost" :disabled="loading || running" @click="load">
        {{ loading ? "加载中…" : "刷新列表" }}
      </button>
      <input
        v-model="filter"
        class="filter-input"
        type="text"
        placeholder="按名称或发布者筛选…"
        :disabled="running"
      />
      <span class="spacer"></span>
      <button
        class="primary"
        :disabled="selected.size === 0 || running"
        @click="openConfirm"
      >
        卸载所选（{{ selected.size }}）
      </button>
    </div>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="running" class="progress-panel">
      <p class="panel-title">正在卸载（请逐个完成弹出的官方卸载器窗口，可能请求管理员权限）</p>
      <div v-for="row in progressRows" :key="row.name" class="progress-row">
        <span class="progress-name">{{ row.name }}</span>
        <span class="progress-status">{{ row.status }}</span>
      </div>
    </div>

    <div v-else-if="results.length > 0" class="progress-panel">
      <p class="panel-title">卸载结果</p>
      <div v-for="r in results" :key="r.name" class="progress-row">
        <span class="progress-name">{{ r.name }}</span>
        <span class="progress-status" :class="r.ok ? 'ok' : 'fail'">
          {{ r.ok ? "✓" : "✗" }} {{ r.detail }}
        </span>
      </div>
    </div>

    <div v-if="scanningResidue" class="status">正在扫描残留…</div>
    <div v-if="deletingResidue" class="status">正在清除残留…</div>

    <!-- 残留面板：逐项勾选后清除 -->
    <div v-if="hasResidues && !scanningResidue" class="residue-panel">
      <p class="panel-title">
        发现残留（已勾选项将被永久删除——残留目录无回收站；注册表键已按保护清单校验）
      </p>
      <div v-for="r in residues" :key="r.programName" class="residue-group">
        <h4 class="residue-name">{{ r.programName }}</h4>
        <label v-for="f in r.files" :key="f.path" class="residue-row">
          <input
            v-model="residueChecked"
            type="checkbox"
            :value="`F|${f.path}`"
            :disabled="deletingResidue"
          />
          <span class="residue-path" :title="f.path">{{ f.path }}</span>
          <span class="residue-size">{{ humanSize(f.size) }}</span>
        </label>
        <label v-for="k in r.registry" :key="k" class="residue-row">
          <input
            v-model="residueChecked"
            type="checkbox"
            :value="`R|${k}`"
            :disabled="deletingResidue"
          />
          <span class="residue-path" :title="k">[注册表] {{ k }}</span>
        </label>
      </div>
      <div class="modal-actions">
        <button
          class="danger"
          :disabled="deletingResidue || residueChecked.size === 0"
          @click="deleteResidue"
        >
          清除已勾选残留
        </button>
      </div>
    </div>

    <div class="list">
      <div class="list-head">
        <label class="select-all">
          <input type="checkbox" :checked="allSelected()" @change="toggleSelectAll" />
          全选
        </label>
        <span class="hint-text">
          勾选后点右上角「卸载所选」；将逐个启动官方卸载器。卸载完成后自动扫描文件与注册表残留供你确认清除
        </span>
      </div>
      <div v-for="p in filtered" :key="p.registryKey" class="prog-row">
        <input
          type="checkbox"
          class="file-check"
          :checked="isSelected(p)"
          @click.stop
          @change="toggleSelect(p)"
        />
        <div class="prog-main" :title="p.installLocation || p.displayName">
          <div class="prog-line">
            <span class="prog-name">{{ p.displayName }}</span>
            <span v-if="p.displayVersion" class="prog-version">{{ p.displayVersion }}</span>
          </div>
          <div class="prog-sub">
            {{ p.publisher || "未知发布者" }}
            <template v-if="p.installLocation"> · {{ p.installLocation }}</template>
          </div>
        </div>
        <span class="prog-size">{{ sizeLabel(p) }}</span>
      </div>
      <div v-if="filtered.length === 0 && !loading" class="empty-hint">
        没有匹配的软件。
      </div>
    </div>

    <!-- 卸载确认弹窗 -->
    <div v-if="showConfirm" class="overlay" @click.self="showConfirm = false">
      <div class="modal">
        <h3>确认卸载</h3>
        <div class="confirm-list">
          <div v-for="p in selectedPrograms" :key="p.registryKey" class="confirm-row">
            <span>{{ p.displayName }}</span>
            <span class="confirm-size">{{ sizeLabel(p) }}</span>
          </div>
        </div>
        <p class="modal-line">
          共 <strong>{{ humanCount(selectedPrograms.length) }}</strong> 个软件，将
          <strong>逐个启动官方卸载器</strong>：请在每个弹出的窗口中完成卸载，
          过程中可能请求管理员权限。
        </p>
        <p class="detail-line muted">
          卸载完成后会自动扫描文件与注册表残留，逐项确认后才会删除（注册表删除受保护清单约束，
          仅允许该软件自己的键）。
        </p>
        <div class="modal-actions">
          <button @click="showConfirm = false">取消</button>
          <button class="primary" @click="confirmUninstall">开始卸载</button>
        </div>
      </div>
    </div>

    <ReportModal
      v-if="showReport && report"
      :report="report"
      extra-note="残留目录为永久删除（不走回收站）；注册表键的删除结果见上方明细。"
      @close="closeReport"
    />
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
  gap: 10px;
  flex-shrink: 0;
}

.ghost {
  background: transparent;
  border: 1px solid var(--border);
  padding: 6px 14px;
}

.filter-input {
  flex: 1;
  max-width: 420px;
}

.spacer {
  flex: 1;
}

.error {
  color: var(--danger);
}

.status {
  color: var(--accent);
}

.progress-panel,
.residue-panel {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 10px 14px;
  flex-shrink: 0;
}

.panel-title {
  font-size: 13px;
  color: var(--text);
  margin-bottom: 6px;
}

.progress-row {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 3px 0;
  font-size: 13px;
}

.progress-name {
  font-weight: 500;
  flex-shrink: 0;
}

.progress-status {
  color: var(--text-secondary);
  text-align: right;
}

.progress-status.ok {
  color: #1a7f47;
}

.progress-status.fail {
  color: var(--danger);
}

.residue-name {
  font-size: 13px;
  margin: 6px 0 3px;
}

.residue-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 3px 0;
  font-size: 12px;
  cursor: pointer;
}

.residue-path {
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.residue-size {
  margin-left: auto;
  flex-shrink: 0;
  font-variant-numeric: tabular-nums;
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

.modal-actions button.danger {
  background: var(--danger);
  border-color: transparent;
  color: #fff;
}

.modal-actions button.primary {
  background: var(--accent);
  border-color: transparent;
  color: #fff;
}

.list {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 8px 14px;
}

.list-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 6px 0;
  border-bottom: 1px solid var(--border);
}

.select-all {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  cursor: pointer;
  user-select: none;
}

.hint-text {
  font-size: 12px;
  color: var(--text-secondary);
}

.prog-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 6px;
  border-radius: 8px;
  border-bottom: 1px solid var(--border);
}

.prog-row:hover {
  background: rgba(47, 111, 237, 0.05);
}

.file-check {
  width: 13px;
  height: 13px;
  flex-shrink: 0;
  cursor: pointer;
  accent-color: var(--accent);
}

.prog-main {
  min-width: 0;
  flex: 1;
}

.prog-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
}

.prog-name {
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.prog-version {
  font-size: 12px;
  color: var(--text-secondary);
  flex-shrink: 0;
}

.prog-sub {
  font-size: 12px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.prog-size {
  width: 88px;
  text-align: right;
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
  color: var(--text-secondary);
}

.empty-hint {
  padding: 24px;
  text-align: center;
  color: var(--text-secondary);
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

.modal h3 {
  font-size: 16px;
}

.confirm-list {
  max-height: 200px;
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
  font-size: 13px;
}

.detail-line {
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.6;
}

.detail-line.muted {
  font-size: 11px;
}
</style>
