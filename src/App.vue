<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import DirTree from "./components/DirTree.vue";
import { humanCount, humanSize } from "./lib/format";
import type { ScanCompletePayload, ScanProgressPayload } from "./types";

const scanRoot = ref("C:\\");
/** 发起扫描时冻结的根路径（结果树按它拼接路径，避免用户中途改输入框导致错乱） */
const scannedRoot = ref("");
const scanning = ref(false);
const progress = ref({ files: 0, bytes: 0 });
const result = ref<ScanCompletePayload | null>(null);
const error = ref("");
/** IPC 事件通路自检状态：null=检测中 true=正常 false=异常 */
const ipcOk = ref<boolean | null>(null);

/**
 * 当前扫描的前端关联 id。
 * 关键：在 invoke 之前生成，事件在 invoke 返回前到达也能正确关联
 * （2026-11-03 审查结论：否则小目录的 complete 事件会被丢弃，UI 永久卡"扫描中"）。
 */
let clientId: string | null = null;
const unlisteners: UnlistenFn[] = [];

onMounted(async () => {
  try {
    unlisteners.push(
      await listen("scan://hello", () => {
        ipcOk.value = true;
      })
    );
    unlisteners.push(
      await listen<ScanProgressPayload>("scan://progress", (e) => {
        if (e.payload.clientId !== clientId) return;
        progress.value = { files: e.payload.filesSeen, bytes: e.payload.bytesSeen };
      })
    );
    unlisteners.push(
      await listen<ScanCompletePayload>("scan://complete", (e) => {
        if (e.payload.clientId !== clientId) return;
        result.value = e.payload;
        scanning.value = false;
      })
    );
    unlisteners.push(
      await listen<{ clientId: string }>("scan://cancelled", (e) => {
        if (e.payload.clientId !== clientId) return;
        scanning.value = false;
      })
    );
    unlisteners.push(
      await listen<{ clientId: string; message: string }>("scan://failed", (e) => {
        if (e.payload.clientId !== clientId) return;
        error.value = e.payload.message;
        scanning.value = false;
      })
    );
  } catch (err) {
    ipcOk.value = false;
    error.value = `事件监听注册失败：${err}`;
    console.error("listen 注册失败", err);
    return;
  }

  // 通路自检：Rust 回发 scan://hello，若 3 秒内未收到则判定异常
  try {
    await invoke("ipc_hello");
  } catch (err) {
    ipcOk.value = false;
    error.value = `IPC 自检调用失败：${err}`;
    console.error("ipc_hello 调用失败", err);
    return;
  }
  setTimeout(() => {
    if (ipcOk.value !== true) {
      ipcOk.value = false;
      error.value = "IPC 事件通路异常：自检事件 3 秒内未返回（事件发送或监听链路故障）";
    }
  }, 3000);
});

onUnmounted(() => unlisteners.forEach((u) => u()));

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
  <main class="page">
    <header class="header">
      <h1>Purimaster <span class="sub">净化大师 · 磁盘空间分析</span></h1>
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
      <p class="ipc-chip" :class="{ ok: ipcOk === true, bad: ipcOk === false }">
        IPC 事件通道：{{ ipcOk === true ? "正常 ✓" : ipcOk === false ? "异常 ✗" : "检测中…" }}
      </p>
    </header>

    <section v-if="result" class="result">
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
    </section>
  </main>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 20px 24px;
  gap: 16px;
}

.header {
  flex-shrink: 0;
}

h1 {
  font-size: 20px;
  margin-bottom: 14px;
}

.sub {
  font-size: 12px;
  font-weight: 400;
  color: var(--text-secondary);
  margin-left: 8px;
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
  margin-top: 10px;
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

.error {
  margin-top: 10px;
  color: var(--danger);
}

.ipc-chip {
  margin-top: 10px;
  font-size: 12px;
  color: var(--text-secondary);
}

.ipc-chip.ok {
  color: #1a9e55;
}

.ipc-chip.bad {
  color: var(--danger);
  font-weight: 600;
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
