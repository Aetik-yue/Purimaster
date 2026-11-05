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
      <h1>
        <!-- 定稿 Logo：design/logos/final.svg 内联（P 字标），currentColor 随标题文字色 -->
        <svg
          class="logo"
          viewBox="0 0 512 512"
          width="22"
          height="22"
          aria-hidden="true"
        >
          <g fill="currentColor">
            <rect x="148" y="80" width="64" height="352" />
            <path
              d="M 361.727 213.623 A 104 104 0 1 0 295.57 289.728 L 298.949 288.401 L 302.21 286.804 L 305.259 284.795 L 307.984 282.244 L 310.25 279.036 L 311.907 275.068 L 312.786 270.258 L 312.703 264.54 L 311.462 257.868 L 308.852 250.219 L 302.119 245.909 L 296.64 242.431 L 292.212 239.756 L 288.641 237.835 L 285.75 236.6 L 283.376 235.964 L 281.373 235.822 L 279.614 236.055 L 277.992 236.531 L 276.417 237.105 A 48 48 0 1 1 306.951 201.98 Z"
            />
          </g>
        </svg>
        Purimaster <span class="sub">净化大师</span>
      </h1>
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
  padding: var(--space-4) var(--space-6) var(--space-5);
  gap: var(--space-3);
}

.topbar {
  display: flex;
  align-items: center;
  gap: var(--space-6);
  flex-shrink: 0;
}

/* 字标：P 字标 logo（final.svg 内联，currentColor 随文字色）+ 品牌名 */
h1 {
  font-size: var(--font-xl);
  font-weight: 700;
  letter-spacing: -0.01em;
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

.logo {
  display: block;
  flex-shrink: 0;
}

.sub {
  font-size: var(--font-xs);
  font-weight: 400;
  color: var(--text-secondary);
  letter-spacing: 0.08em;
}

/* 页签：下划线式，靠字重与 2px 强调线区分层级，不做胶囊堆叠 */
.tabs {
  display: flex;
  gap: var(--space-2);
  border-bottom: 1px solid var(--border);
}

.tabs button {
  padding: var(--space-2) var(--space-3) var(--space-3);
  border: none;
  border-bottom: 2px solid transparent;
  border-radius: 0;
  margin-bottom: -1px;
  background: transparent;
  color: var(--text-secondary);
  transition: color var(--dur-fast) var(--ease),
    border-color var(--dur-fast) var(--ease);
}

.tabs button:hover {
  color: var(--text);
}

.tabs button.active {
  color: var(--text);
  border-bottom-color: var(--accent);
  font-weight: 600;
}

.ipc-chip {
  margin-left: auto;
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.ipc-chip.ok {
  color: var(--success);
}

.ipc-chip.bad {
  color: var(--danger);
  font-weight: 600;
}

.ipc-error {
  color: var(--danger);
  font-size: var(--font-xs);
}

/* 关窗保护弹窗 */
.overlay {
  position: fixed;
  inset: 0;
  background: var(--scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
  /* 显隐动效：0.2s 淡入（--dur 区间），无位移，与报告弹窗一致 */
  animation: overlay-in var(--dur) var(--ease);
}

@keyframes overlay-in {
  from {
    opacity: 0;
  }
}

.modal {
  width: 400px;
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

.modal h3 {
  font-size: var(--font-lg);
}

.modal-line {
  color: var(--text-secondary);
  font-size: var(--font-sm);
}

.warn-text {
  color: var(--danger);
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
  background: var(--bg);
  border-color: var(--border-strong);
}

.modal-actions .danger-btn {
  background: var(--danger);
  border-color: transparent;
  color: #fff;
}

.modal-actions .danger-btn:hover {
  background: var(--danger-hover);
  border-color: transparent;
}
</style>
