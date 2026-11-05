<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { humanCount, humanSize } from "../lib/format";
import type { CleanReport } from "../types";

/**
 * 清理/删除结果报告弹窗——规则清理（JunkView）与手动删除（SpaceView）共用。
 *
 * 只负责展示与"打开回收站"入口；关闭语义（是否重扫等）由父组件决定。
 */
defineProps<{
  report: CleanReport;
  /** 场景附注（如"目录大小为扫描时快照"），显示在统计区下方 */
  extraNote?: string;
}>();

const emit = defineEmits<{ close: [] }>();

/** 回收站模式的最后一步：帮用户走到"清空/还原"（移入 ≠ 释放） */
async function openRecycleBin() {
  try {
    await invoke("open_recycle_bin");
  } catch {
    // 打开失败不打断报告，用户可手动打开回收站
  }
}
</script>

<template>
  <div class="overlay" @click.self="emit('close')">
    <div class="modal wide">
      <h3>
        {{ report.dryRun ? "预演完成（未删除任何文件）" : "清理完成" }}
        <span v-if="report.cancelled" class="cancel-tag">（已取消，以下为已完成部分）</span>
      </h3>
      <p class="modal-line">
        {{ report.dryRun ? "预演结果：将" : "已" }}{{
          report.mode === "trash" ? "移入回收站" : "永久删除"
        }}
        <strong>{{ humanCount(report.deletedFiles) }}</strong> 个文件<template
          v-if="report.mode === 'trash'"
          >。文件可在回收站中还原；磁盘空间将在清空回收站后真正释放（约
          <strong>{{ humanSize(report.freedBytes) }}</strong
          >）</template
        ><template v-else
          >，{{ report.dryRun ? "可释放" : "释放" }}
          <strong>{{ humanSize(report.freedBytes) }}</strong></template
        >。
      </p>
      <p v-if="extraNote" class="detail-line muted">{{ extraNote }}</p>
      <p v-if="report.skipped > 0" class="detail-line muted">
        另有 {{ humanCount(report.skipped) }} 个条目因读取失败或目标异常被跳过。
      </p>
      <p v-if="report.failedCount > 0" class="warn">
        {{
          report.dryRun
            ? `有 ${humanCount(report.failedCount)} 个文件未通过安全检查（未删除，多为无法安全移入回收站）：`
            : `失败 ${humanCount(report.failedCount)} 个文件（如被占用、只读或权限不足）：`
        }}
      </p>
      <div v-if="report.failed.length" class="failures">
        <div v-for="f in report.failed" :key="f.path" class="fail-row">
          <span class="fail-path" :title="f.path">{{ f.path }}</span>
          <span class="fail-err">{{ f.error }}</span>
        </div>
      </div>
      <p v-if="report.failedCount > report.failed.length" class="detail-line muted">
        仅显示前 {{ report.failed.length }} 条，另有
        {{ humanCount(report.failedCount - report.failed.length) }} 条失败未展示。
      </p>
      <details v-if="report.skippedRules.length" class="skipped">
        <summary>跳过的规则/目标（{{ report.skippedRules.length }}）</summary>
        <div v-for="s in report.skippedRules" :key="s" class="detail-line">{{ s }}</div>
      </details>
      <div class="modal-actions">
        <button
          v-if="!report.dryRun && report.mode === 'trash' && report.deletedFiles > 0"
          @click="openRecycleBin"
        >
          打开回收站
        </button>
        <button class="primary" @click="emit('close')">好的</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  background: var(--scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
  /* 显隐动效：0.2s 淡入（--dur 区间），无位移 */
  animation: overlay-in var(--dur) var(--ease);
}

@keyframes overlay-in {
  from {
    opacity: 0;
  }
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

/* 已取消：警示琥珀（存量语义色），不加底色保持克制 */
.cancel-tag {
  font-size: var(--font-xs);
  font-weight: 400;
  color: var(--warning);
  margin-left: var(--space-1);
}

.modal-line {
  color: var(--text-secondary);
  font-size: var(--font-md);
}

/* 数字是报告主角：文件数与字节数提亮到主文本色，与说明文字拉开层级 */
.modal-line strong {
  color: var(--text);
}

.warn {
  color: var(--danger);
  font-size: var(--font-xs);
}

.detail-line {
  font-size: var(--font-sm);
  color: var(--text-secondary);
  padding: 2px 0;
  word-break: break-all;
}

.detail-line.muted {
  font-size: var(--font-xs);
}

.failures {
  max-height: 224px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.fail-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--font-xs);
}

.fail-path {
  color: var(--text-secondary);
  word-break: break-all;
}

.fail-err {
  color: var(--danger);
}

.skipped {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.skipped summary {
  cursor: pointer;
  user-select: none;
  transition: color var(--dur-fast) var(--ease);
}

.skipped summary:hover {
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
  background: var(--bg);
  border-color: var(--border-strong);
}

.modal-actions button.primary {
  background: var(--accent);
  border-color: transparent;
  color: #fff;
}

.modal-actions button.primary:hover {
  background: var(--accent-hover);
}

.modal-actions button.primary:active {
  background: var(--accent-press);
}
</style>
