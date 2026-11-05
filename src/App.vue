<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import SpaceView from "./views/SpaceView.vue";
import JunkView from "./views/JunkView.vue";
import UninstallView from "./views/UninstallView.vue";
import type { CloseBusyPayload } from "./types";

type Tab = "space" | "junk" | "uninstall";

// 默认进垃圾清理页：目标用户带着"C 盘满了"的诉求打开软件，
// 打开即扫、首屏即见可清理量（模拟用户测试 F-02）
const tab = ref<Tab>("junk");
/** IPC 事件通路自检状态：null=检测中 true=正常 false=异常 */
const ipcOk = ref<boolean | null>(null);
const ipcError = ref("");

/** 关窗保护弹窗（app://close-busy：有任务进行中） */
const showCloseConfirm = ref(false);
const closeRunning = ref(0);
const closeCleaning = ref(false);
const closeError = ref("");

const unlisteners: UnlistenFn[] = [];

onMounted(async () => {
  try {
    unlisteners.push(
      await listen("scan://hello", () => {
        ipcOk.value = true;
      })
    );
    unlisteners.push(
      await listen<CloseBusyPayload>("app://close-busy", (e) => {
        closeRunning.value = e.payload.running;
        closeCleaning.value = e.payload.cleaning;
        closeError.value = "";
        showCloseConfirm.value = true;
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

/** 关窗确认框选"继续等待"：先复位后端关窗保护，成功后再关弹窗；
 * 复位失败保持弹窗并提示——残留标志会让下次关窗跳过确认直接退出（F-16/F-37） */
async function keepWaiting() {
  try {
    await invoke("allow_close");
    showCloseConfirm.value = false;
  } catch {
    closeError.value = "复位失败，请重试或选择退出";
  }
}

/** 关窗确认框选"仍要退出"：后端取消全部任务并退出进程 */
async function forceQuit() {
  try {
    await invoke("quit_app");
  } catch {
    // 退出流程中的失败无需处理
  }
}
</script>

<template>
  <main class="page">
    <header class="topbar">
      <h1>Purimaster <span class="sub">净化大师</span></h1>
      <nav class="tabs">
        <button :class="{ active: tab === 'space' }" @click="tab = 'space'">空间分析</button>
        <button :class="{ active: tab === 'junk' }" @click="tab = 'junk'">垃圾清理</button>
        <button :class="{ active: tab === 'uninstall' }" @click="tab = 'uninstall'">软件卸载</button>
      </nav>
      <!-- IPC 自检只在异常时打扰用户；"正常 ✓"是开发者信息，平时不显示（F-01） -->
      <span v-if="ipcOk !== true" class="ipc-chip" :class="{ bad: ipcOk === false }">
        应用内部通信：{{ ipcOk === false ? "异常，请重启应用" : "检测中…" }}
      </span>
    </header>
    <p v-if="ipcError" class="ipc-error">{{ ipcError }}</p>

    <!-- KeepAlive：切页签保留视图状态，扫描/清理结果不因切页丢失 -->
    <KeepAlive>
      <SpaceView v-if="tab === 'space'" />
      <JunkView v-else-if="tab === 'junk'" />
      <UninstallView v-else />
    </KeepAlive>

    <!-- 关窗保护：任务进行中确认 -->
    <div v-if="showCloseConfirm" class="overlay">
      <div class="modal">
        <h3>任务进行中</h3>
        <p v-if="closeCleaning" class="modal-line warn-text">
          有清理任务正在进行——现在退出会中断删除，<strong>已删除的文件将没有报告</strong>。
          如需收回空间，请等待清理完成。
        </p>
        <p v-else class="modal-line">
          有 <strong>{{ closeRunning }}</strong> 个扫描任务正在进行，现在退出会中断任务。
        </p>
        <p v-if="closeError" class="modal-line warn-text">{{ closeError }}</p>
        <div class="modal-actions">
          <button @click="keepWaiting">继续等待</button>
          <button class="danger-btn" @click="forceQuit">仍要退出</button>
        </div>
      </div>
    </div>
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

/* 关窗保护弹窗 */
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
  width: 400px;
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

.modal-line {
  color: var(--text-secondary);
  font-size: 13px;
}

.warn-text {
  color: #b91c1c;
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

.modal-actions .danger-btn {
  background: var(--danger);
  border-color: transparent;
  color: #fff;
}
</style>
