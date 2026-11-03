<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import SpaceView from "./views/SpaceView.vue";
import JunkView from "./views/JunkView.vue";

type Tab = "space" | "junk";

const tab = ref<Tab>("space");
/** IPC 事件通路自检状态：null=检测中 true=正常 false=异常 */
const ipcOk = ref<boolean | null>(null);
const ipcError = ref("");

const unlisteners: UnlistenFn[] = [];

onMounted(async () => {
  try {
    unlisteners.push(
      await listen("scan://hello", () => {
        ipcOk.value = true;
      })
    );
    await invoke("ipc_hello");
  } catch (err) {
    ipcOk.value = false;
    ipcError.value = `IPC 自检失败：${err}`;
    console.error("IPC 自检失败", err);
    return;
  }
  // 3 秒内未收到自检事件则判定异常
  setTimeout(() => {
    if (ipcOk.value !== true) {
      ipcOk.value = false;
      ipcError.value = "IPC 事件通路异常：自检事件 3 秒内未返回";
    }
  }, 3000);
});

onUnmounted(() => unlisteners.forEach((u) => u()));
</script>

<template>
  <main class="page">
    <header class="topbar">
      <h1>Purimaster <span class="sub">净化大师</span></h1>
      <nav class="tabs">
        <button :class="{ active: tab === 'space' }" @click="tab = 'space'">空间分析</button>
        <button :class="{ active: tab === 'junk' }" @click="tab = 'junk'">垃圾清理</button>
      </nav>
      <span class="ipc-chip" :class="{ ok: ipcOk === true, bad: ipcOk === false }">
        IPC：{{ ipcOk === true ? "正常 ✓" : ipcOk === false ? "异常 ✗" : "检测中…" }}
      </span>
    </header>
    <p v-if="ipcError" class="ipc-error">{{ ipcError }}</p>

    <SpaceView v-if="tab === 'space'" />
    <JunkView v-else />
  </main>
</template>

<style scoped>
.page {
  height: 100%;
  display: flex;
  flex-direction: column;
  padding: 16px 24px 20px;
  gap: 12px;
}

.topbar {
  display: flex;
  align-items: center;
  gap: 20px;
  flex-shrink: 0;
}

h1 {
  font-size: 20px;
}

.sub {
  font-size: 12px;
  font-weight: 400;
  color: var(--text-secondary);
  margin-left: 6px;
}

.tabs {
  display: flex;
  gap: 4px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 3px;
}

.tabs button {
  padding: 5px 16px;
  border-radius: 6px;
  background: transparent;
  color: var(--text-secondary);
}

.tabs button.active {
  background: var(--accent);
  color: #fff;
}

.ipc-chip {
  margin-left: auto;
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

.ipc-error {
  color: var(--danger);
  font-size: 12px;
}
</style>
