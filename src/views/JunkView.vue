<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { humanCount, humanSize } from "../lib/format";
import type { JunkCompletePayload, JunkGroup, JunkScanResult, Risk } from "../types";

const scanning = ref(false);
const result = ref<JunkScanResult | null>(null);
const error = ref("");
const expanded = ref<Set<string>>(new Set());

let clientId: string | null = null;
const unlisteners: UnlistenFn[] = [];
/** 组件已卸载：阻止迟到的 listen 注册与自动扫描（审查发现：卸载后 resolve 会泄漏监听器） */
let disposed = false;

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
  } catch (err) {
    error.value = `事件监听注册失败：${err}`;
    return;
  }
  // 进入页面自动扫描一次（组件已卸载则跳过）
  if (!disposed) startScan();
});

onUnmounted(() => {
  disposed = true;
  // 切走页面时取消仍在运行的扫描：否则后端线程继续遍历磁盘，且前端已无取消入口
  if (clientId !== null) {
    invoke("cancel_scan", { clientId }).catch(() => {});
  }
  unlisteners.forEach((u) => u());
});

async function startScan() {
  error.value = "";
  result.value = null;
  expanded.value = new Set();
  const id =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
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

function groupTargetsSummary(group: JunkGroup): string {
  return group.targets.length > 0 ? group.targets.join("；") : "（目标不存在）";
}
</script>

<template>
  <section class="view">
    <div class="toolbar">
      <button v-if="!scanning" class="primary" @click="startScan">重新扫描</button>
      <button v-else class="danger" @click="cancelScan">取消</button>
      <span class="hint">本版本为只读扫描；删除功能（回收站优先、二次确认）在下一步实现</span>
    </div>

    <p v-if="scanning" class="status">正在扫描垃圾文件…</p>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <p class="summary">
        共发现 <strong>{{ humanCount(result.totalFiles) }}</strong> 个可清理文件，合计
        <strong>{{ humanSize(result.totalBytes) }}</strong>
        <span v-if="result.skipped > 0"> · 跳过 {{ humanCount(result.skipped) }} 项（无权限）</span>
      </p>

      <p v-if="result.totalFiles === 0 && !scanning" class="empty">未发现可清理的垃圾文件 ✓</p>

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

.hint {
  font-size: 12px;
  color: var(--text-secondary);
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
</style>
