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

.modal-line {
  color: var(--text-secondary);
}

.warn {
  color: #b91c1c;
  font-size: 12px;
}

.detail-line {
  font-size: 12px;
  color: var(--text-secondary);
  padding: 2px 0;
  word-break: break-all;
}

.detail-line.muted {
  font-size: 11px;
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
</style>
