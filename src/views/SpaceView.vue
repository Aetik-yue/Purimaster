<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import DirTree from "../components/DirTree.vue";
import { humanCount, humanSize } from "../lib/format";
import type { ScanCompletePayload, ScanProgressPayload } from "../types";

const scanRoot = ref("C:\\");
/** 发起扫描时冻结的根路径（结果树按它拼接路径，避免用户中途改输入框导致错乱） */
const scannedRoot = ref("");
const scanning = ref(false);
const progress = ref({ files: 0, bytes: 0 });
const result = ref<ScanCompletePayload | null>(null);
const error = ref("");

/**
 * 当前扫描的前端关联 id。
 * 关键：在 invoke 之前生成，事件在 invoke 返回前到达也能正确关联
 * （2026-11-03 审查结论：否则小目录的 complete 事件会被丢弃，UI 永久卡"扫描中"）。
 */
let clientId: string | null = null;
const unlisteners: UnlistenFn[] = [];
/** 组件已卸载：阻止迟到的 listen 注册（卸载后 resolve 会泄漏监听器） */
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

onMounted(async () => {
  try {
    await onEvent<ScanProgressPayload>("scan://progress", (payload) => {
      if (payload.clientId !== clientId) return;
      progress.value = { files: payload.filesSeen, bytes: payload.bytesSeen };
    });
    await onEvent<ScanCompletePayload>("scan://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      result.value = payload;
      scanning.value = false;
    });
    await onEvent<{ clientId: string }>("scan://cancelled", (payload) => {
      if (payload.clientId !== clientId) return;
      scanning.value = false;
    });
    await onEvent<{ clientId: string; message: string }>("scan://failed", (payload) => {
      if (payload.clientId !== clientId) return;
      error.value = payload.message;
      scanning.value = false;
    });
  } catch (err) {
    error.value = `事件监听注册失败：${err}`;
    console.error("listen 注册失败", err);
  }
});

onUnmounted(() => {
  disposed = true;
  // 切走页面时取消仍在运行的扫描，避免后台线程继续跑且前端失去取消入口
  if (clientId !== null) {
    invoke("cancel_scan", { clientId }).catch(() => {});
  }
  unlisteners.forEach((u) => u());
});

async function startScan() {
  error.value = "";
  result.value = null;
  progress.value = { files: 0, bytes: 0 };
  const id =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  clientId = id;
  scannedRoot.value = scanRoot.value;
  scanning.value = true;
  try {
    await invoke("start_scan", { clientId: id, root: scanRoot.value });
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
    </p>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <p class="summary">
        {{ scannedRoot }} · 共 {{ humanCount(result.filesSeen) }} 个文件，占用
        {{ humanSize(result.bytesSeen) }}
        <span v-if="result.skipped > 0"> · 跳过 {{ humanCount(result.skipped) }} 项（无权限）</span>
      </p>
      <div class="tree">
        <DirTree
          :node="result.root"
          :initial-children="result.rootChildren"
          :client-id="result.clientId"
          :dir-path="scannedRoot"
        />
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
  gap: 10px;
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
  gap: 8px;
}

.summary {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.tree {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 8px;
}
</style>
