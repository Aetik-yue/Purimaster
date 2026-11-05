<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import ReportModal from "../components/ReportModal.vue";
import { humanCount, humanSize } from "../lib/format";
import type {
  CleanCompletePayload,
  CleanProgressPayload,
  CleanReport,
  JunkCompletePayload,
  JunkGroup,
  JunkScanProgressPayload,
  JunkScanResult,
  Risk,
  TargetedCompletePayload,
} from "../types";

// 会话级缓存：切页回来直接显示上次结果，不重复扫描（清理时后端会重新枚举，显示值略旧是安全的）
let sessionCache: JunkScanResult | null = null;

const scanning = ref(false);
const cleaning = ref(false);
const result = ref<JunkScanResult | null>(sessionCache);
/** 垃圾扫描进度（规则推进）；null 表示尚未收到进度 */
const junkProgress = ref<JunkScanProgressPayload | null>(null);
/** 清理进度（已完成计数）；null 表示尚未收到进度 */
const cleanProgress = ref<CleanProgressPayload | null>(null);
/** 本次清理是否为预演（清理中报告尚不存在，用发起时的选择判断"已/将释放"文案） */
const cleaningDryRun = ref(false);
/** 用户主动取消后的提示（区别于错误，非红色） */
const cancelNotice = ref("");

// 定向清理：用户指定目录，走与规则清理完全相同的安全管线
const showTargeted = ref(false);
const targetedPath = ref("");
const targetedInclude = ref("");
const targetedSkipRecent = ref(true);
const targetedScanning = ref(false);
const targetedResult = ref<JunkScanResult | null>(null);
const targetedError = ref("");
let targetedClientId: string | null = null;
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
/** 永久删除的确认词（需求：永久删除需二次确认，键入"删除"解锁按钮） */
const confirmWord = ref("");
const confirmWordInput = ref<HTMLInputElement | null>(null);

/** 扫描进度条百分比（按规则推进；未收到进度时为 0） */
const scanPercent = computed(() => {
  const p = junkProgress.value;
  if (!p || p.ruleCount === 0) return 0;
  return Math.round(((p.ruleIndex + 1) / p.ruleCount) * 100);
});

// 弹窗打开时聚焦确认词输入框
watch(showConfirm, (open) => {
  if (open) {
    void nextTick(() => confirmWordInput.value?.focus());
  }
});

/** Esc 关闭弹窗：确认 → 定向清理 → 报告（关窗保护弹窗不响应 Esc——安全确认不跳过） */
function onGlobalKeydown(e: KeyboardEvent) {
  if (e.key !== "Escape") return;
  if (showConfirm.value) {
    showConfirm.value = false;
    return;
  }
  if (showTargeted.value) {
    showTargeted.value = false;
    return;
  }
  if (showReport.value && report.value) {
    closeReport();
  }
}

/** 确认弹窗列表里是否有定向清理（其说明/警示与规则清理一致） */
const targetedGroup = computed(() => targetedResult.value?.groups[0] ?? null);

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
  () => result.value?.groups.filter((g) => g.files > 0 && !g.scanOnly) ?? []
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
/** 选中项里是否存在中/高风险（确认弹窗需要更强警示，F-10） */
const hasRiskSelection = computed(() =>
  selectedGroups.value.some((g) => g.risk !== "low")
);
/** 永久删除（非预演）必须键入确认词"删除"才允许开始（需求：二次确认，F-05） */
const needsConfirmWord = computed(
  () => mode.value === "permanent" && !dryRun.value
);
const canStartClean = computed(
  () => !needsConfirmWord.value || confirmWord.value.trim() === "删除"
);

/** 安全默认勾选：低风险自动勾上，中/高风险留给用户确认；仅扫描规则不可勾选 */
function initSelection(r: JunkScanResult) {
  selected.value = new Set(
    r.groups
      .filter((g) => g.files > 0 && g.risk === "low" && !g.scanOnly)
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
  window.addEventListener("keydown", onGlobalKeydown);
  try {
    await onEvent<JunkScanProgressPayload>("junk://progress", (payload) => {
      if (payload.clientId !== clientId) return;
      junkProgress.value = payload;
    });
    await onEvent<JunkCompletePayload>("junk://complete", (payload) => {
      if (payload.clientId !== clientId) return;
      result.value = payload.result;
      sessionCache = payload.result;
      stale.value = false;
      initSelection(payload.result);
      scanning.value = false;
      junkProgress.value = null;
    });
    await onEvent<{ clientId: string }>("junk://cancelled", (payload) => {
      if (payload.clientId !== clientId) return;
      scanning.value = false;
      junkProgress.value = null;
      cancelNotice.value = "扫描已取消";
    });
    await onEvent<TargetedCompletePayload>("targeted://complete", (payload) => {
      if (payload.clientId !== targetedClientId) return;
      targetedResult.value = payload.result;
      targetedScanning.value = false;
    });
    await onEvent<{ clientId: string }>("targeted://cancelled", (payload) => {
      if (payload.clientId !== targetedClientId) return;
      targetedScanning.value = false;
    });
    await onEvent<CleanProgressPayload>("clean://progress", (payload) => {
      if (payload.clientId !== cleanClientId) return;
      cleanProgress.value = payload;
    });
    await onEvent<CleanCompletePayload>("clean://complete", (payload) => {
      if (payload.clientId !== cleanClientId) return;
      report.value = payload.report;
      showReport.value = true;
      cleaning.value = false;
      cleanProgress.value = null;
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
  window.removeEventListener("keydown", onGlobalKeydown);
  // KeepAlive 下切页签不会触发本钩子（垃圾扫描只读且写入后端缓存，本就切页不打断）；
  // 应用整体退出时取消仍未完成的扫描
  if (clientId !== null) invoke("cancel_scan", { clientId }).catch(() => {});
  unlisteners.forEach((u) => u());
});

async function startScan() {
  if (cleaning.value) return;
  error.value = "";
  cancelNotice.value = "";
  result.value = null;
  sessionCache = null;
  junkProgress.value = null;
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
  if (group.files === 0 || group.scanOnly || cleaning.value) return;
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

/** 打开确认弹窗：每次都重置为安全默认（回收站、非试运行、空确认词） */
function openConfirm() {
  mode.value = "trash";
  dryRun.value = false;
  confirmWord.value = "";
  showConfirm.value = true;
}

async function confirmClean() {
  if (selected.value.size === 0 || cleaning.value) return;
  showConfirm.value = false;
  error.value = "";
  report.value = null;
  cleanProgress.value = null;
  cleaningDryRun.value = dryRun.value;
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
  invoke("cancel_scan", { clientId: cleanClientId }).catch((err) => {
    // 取消失败必须可见（对比此前静默吞掉，F-38）
    error.value = `取消失败：${err}`;
  });
}

// ---------------------------------------------------------------------------
// 定向清理
// ---------------------------------------------------------------------------

function openTargeted() {
  // 安全默认（与确认弹窗一致）
  mode.value = "trash";
  dryRun.value = false;
  confirmWord.value = "";
  targetedError.value = "";
  showTargeted.value = true;
}

/** 类型过滤输入 → include glob 列表（中英文逗号均可） */
function parseInclude(): string[] {
  return targetedInclude.value
    .split(/[,，]/)
    .map((s) => s.trim())
    .filter(Boolean);
}

async function startTargetedScan() {
  const p = targetedPath.value.trim();
  if (!p) {
    targetedError.value = "请输入要清理的目录路径";
    return;
  }
  targetedError.value = "";
  targetedResult.value = null;
  const id = genId();
  targetedClientId = id;
  targetedScanning.value = true;
  try {
    await invoke("scan_targeted", {
      clientId: id,
      path: p,
      include: parseInclude(),
      skipRecent: targetedSkipRecent.value,
    });
  } catch (err) {
    targetedError.value = String(err);
    targetedScanning.value = false;
  }
}

function cancelTargetedScan() {
  if (targetedClientId === null) return;
  invoke("cancel_scan", { clientId: targetedClientId }).catch(() => {});
}

async function confirmTargetedClean() {
  const group = targetedGroup.value;
  if (!group || group.files === 0 || cleaning.value || !canStartClean.value) return;
  showTargeted.value = false;
  error.value = "";
  report.value = null;
  cleanProgress.value = null;
  cleaningDryRun.value = dryRun.value;
  const id = genId();
  cleanClientId = id;
  cleaning.value = true;
  try {
    await invoke("clean_targeted", {
      clientId: id,
      request: {
        path: targetedPath.value.trim(),
        include: parseInclude(),
        skipRecent: targetedSkipRecent.value,
        permanent: mode.value === "permanent",
        dryRun: dryRun.value,
      },
    });
  } catch (err) {
    error.value = String(err);
    cleaning.value = false;
  }
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
      <span v-if="cleaning" class="status">
        <template v-if="cleanProgress">
          正在清理「{{ cleanProgress.currentTitle }}」… 已处理
          {{ humanCount(cleanProgress.deletedFiles + cleanProgress.failedCount) }} 个文件<template
            v-if="cleanProgress.deletedFiles > 0"
          >
            · 已{{ cleaningDryRun ? "将" : "" }}释放 {{ humanSize(cleanProgress.freedBytes) }}</template
          >
        </template>
        <template v-else>正在清理…</template>
      </span>
    </div>

    <p v-if="cancelNotice && !scanning" class="status">{{ cancelNotice }}</p>
    <p v-if="scanning" class="status big-status">
      <template v-if="junkProgress">
        正在扫描垃圾文件…「{{ junkProgress.currentTitle }}」（{{ junkProgress.ruleIndex + 1 }}/{{
          junkProgress.ruleCount
        }}）· 已发现 {{ humanCount(junkProgress.files) }} 个文件（{{
          humanSize(junkProgress.bytes)
        }}）
      </template>
      <template v-else>正在扫描垃圾文件…</template>
    </p>
    <!-- 细进度条：随规则推进（无总量百分比，规则数是唯一确定刻度） -->
    <div v-if="scanning" class="scan-bar">
      <div class="scan-bar-fill" :style="{ width: scanPercent + '%' }"></div>
    </div>
    <p v-if="error" class="error">{{ error }}</p>

    <div v-if="result" class="result">
      <!-- 主视觉：可清理空间 + 立即清理 -->
      <div class="hero">
        <div class="hero-main">
          <div class="hero-label">已勾选可清理</div>
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
        <button
          class="ghost targeted-btn"
          :disabled="cleaning || scanning"
          title="指定任意目录进行清理，与规则清理走同一套安全检查"
          @click="openTargeted"
        >
          定向清理
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
        <span class="hint-text"
          >低风险项已自动勾选；全选会包含中/高风险项，清理前请逐项确认。可先在确认弹窗勾选「先预演」查看将清理的内容</span
        >
      </div>

      <div class="groups">
        <div v-for="cat in grouped" :key="cat.id" class="category">
          <h2 class="category-title">{{ cat.title }}</h2>

          <div
            v-for="g in cat.groups"
            :key="g.ruleId"
            class="group-row"
            :class="{ selected: selected.has(g.ruleId), dimmed: g.files === 0 || g.scanOnly }"
            @click="toggleSelect(g)"
          >
            <span class="check">
              <input
                v-if="g.files > 0 && !g.scanOnly"
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
                <span v-if="g.scanOnly" class="badge scan-only" title="该规则配置为仅扫描，不参与清理">仅扫描</span>
                <span class="badge" :class="'risk-' + g.risk">{{ riskLabel[g.risk] }}</span>
                <span v-if="g.problems.length > 0" class="problem-dot" title="存在无法读取的目标">⚠</span>
              </div>
              <div class="group-sub">
                {{ humanCount(g.files) }} 个文件<template v-if="g.scanOnly">
                  · 不参与清理（规则声明目标有条件限制）</template
                ><template v-if="g.freshFiles > 0">
                  · 另有 {{ humanCount(g.freshFiles) }} 个近期文件未计入（可能正在使用）</template
                >
              </div>
            </div>
            <div class="group-size">{{ humanSize(g.bytes) }}</div>
            <button class="ghost detail-btn" @click.stop="toggleExpand(g.ruleId)">
              详情 {{ expanded.has(g.ruleId) ? "▴" : "▾" }}
            </button>
          </div>

          <div v-for="g in cat.groups" :key="g.ruleId + '-detail'">
            <div v-if="expanded.has(g.ruleId)" class="group-detail">
              <div v-if="g.description" class="detail-line desc">{{ g.description }}</div>
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

    <!-- 定向清理弹窗 -->
    <div v-if="showTargeted" class="overlay" @click.self="showTargeted = false">
      <div class="modal wide">
        <h3>定向清理</h3>
        <p class="detail-line desc">
          清理你指定的目录。与规则清理走同一套安全检查：系统路径保护（系统/个人目录会被拒绝）、
          逐文件校验、默认移入回收站。
        </p>
        <label class="field">
          <span class="field-label">目录路径</span>
          <input
            v-model="targetedPath"
            type="text"
            placeholder="如 D:\Downloads\old-logs"
            :disabled="targetedScanning"
            @keydown.enter="!targetedScanning && !targetedResult && startTargetedScan()"
          />
        </label>
        <label class="field">
          <span class="field-label">文件类型过滤（可选，逗号分隔，如 *.log, *.tmp）</span>
          <input
            v-model="targetedInclude"
            type="text"
            placeholder="留空 = 全部文件"
            :disabled="targetedScanning"
          />
        </label>
        <label class="option">
          <input v-model="targetedSkipRecent" type="checkbox" :disabled="targetedScanning" />
          跳过最近 24 小时内修改的文件（推荐——正在使用的文件几乎都是新写的）
        </label>

        <div v-if="targetedScanning" class="status">
          正在扫描（大目录可能需要一些时间）…
          <button class="ghost scan-cancel" @click="cancelTargetedScan">取消</button>
        </div>
        <p v-if="targetedError" class="error">{{ targetedError }}</p>

        <template v-if="targetedGroup && !targetedScanning">
          <div class="targeted-summary">
            共 <strong>{{ humanCount(targetedGroup.files) }}</strong> 个文件 ·
            <strong>{{ humanSize(targetedGroup.bytes) }}</strong>
            <template v-if="targetedGroup.freshFiles > 0">
              · 另有 {{ humanCount(targetedGroup.freshFiles) }} 个近期文件未计入</template
            >
          </div>
          <div v-if="targetedGroup.problems.length > 0" class="targeted-problems">
            <div v-for="p in targetedGroup.problems" :key="p" class="detail-line problem">
              ⚠ {{ p }}
            </div>
          </div>
          <div v-if="targetedGroup.topFiles.length > 0" class="targeted-files">
            <div
              v-for="f in targetedGroup.topFiles.slice(0, 20)"
              :key="f.path"
              class="file-row"
              :title="f.path"
            >
              <span class="file-path">{{ f.path }}</span>
              <span class="file-size">{{ humanSize(f.size) }}</span>
            </div>
            <p v-if="targetedGroup.files > targetedGroup.topFiles.length" class="detail-line muted">
              仅预览最大的 20 个文件，共 {{ humanCount(targetedGroup.files) }} 个
            </p>
          </div>
          <template v-if="targetedGroup.files > 0">
            <div class="option-group">
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
                先预演（只检查，不删除任何文件）
              </label>
            </div>
            <template v-if="mode === 'permanent' && !dryRun">
              <p class="warn danger-note">⚠ 永久删除将绕过回收站直接删除，无法恢复。请输入「删除」确认。</p>
              <input
                ref="confirmWordInput"
                v-model="confirmWord"
                class="confirm-word"
                type="text"
                placeholder="输入：删除"
                @keydown.enter="canStartClean && confirmTargetedClean()"
              />
            </template>
          </template>
          <p v-else class="detail-line muted">
            该目录下没有匹配的文件（或都被年龄策略跳过）。
          </p>
        </template>

        <div class="modal-actions">
          <button @click="showTargeted = false">关闭</button>
          <button
            v-if="!targetedResult || targetedScanning"
            class="primary"
            :disabled="targetedScanning || !targetedPath.trim()"
            @click="startTargetedScan"
          >
            扫描该目录
          </button>
          <button
            v-else-if="targetedGroup && targetedGroup.files > 0"
            class="primary"
            :disabled="cleaning || !canStartClean"
            @click="confirmTargetedClean"
          >
            开始清理
          </button>
        </div>
      </div>
    </div>

    <!-- 确认弹窗（Esc / 点击遮罩关闭 = 取消，不做任何删除） -->
    <div v-if="showConfirm" class="overlay" @click.self="showConfirm = false">
      <div class="modal">
        <h3>确认清理</h3>
        <div class="confirm-list">
          <div v-for="g in selectedGroups" :key="g.ruleId" class="confirm-item">
            <div class="confirm-row">
              <span class="confirm-title">
                {{ g.title }}
                <span class="badge" :class="'risk-' + g.risk">{{ riskLabel[g.risk] }}</span>
              </span>
              <span class="confirm-size">{{ humanSize(g.bytes) }}</span>
            </div>
            <div v-if="g.description" class="confirm-desc">{{ g.description }}</div>
          </div>
          <div v-if="selectedGroups.length > 6" class="detail-line muted">
            …等 {{ selectedGroups.length }} 个项目
          </div>
        </div>
        <p class="modal-line">
          共 <strong>{{ humanCount(selectedFiles) }}</strong> 个文件 ·
          <strong>{{ humanSize(selectedBytes) }}</strong>
        </p>
        <p v-if="hasRiskSelection" class="warn">
          ⚠ 所选内容包含中/高风险项目，请确认「详情」里的目标与文件确实可以丢弃。
        </p>
        <div class="option-group">
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
            先预演（只检查，不删除任何文件）
          </label>
        </div>
        <p class="detail-line muted option-explain">
          回收站：文件可随时还原，磁盘空间在清空回收站后才真正释放。永久删除：绕过回收站，
          事后无法恢复。预演：列出将要处理的文件，不动任何东西。个别文件若正被占用会删除失败
          并在报告中列出，不影响其余文件。
        </p>
        <template v-if="mode === 'permanent' && !dryRun">
          <p class="warn danger-note">
            ⚠ 永久删除将绕过回收站直接删除文件，事后无法通过任何方式恢复。
            请输入「删除」以确认执行。
          </p>
          <input
            ref="confirmWordInput"
            v-model="confirmWord"
            class="confirm-word"
            type="text"
            placeholder="输入：删除"
            @keydown.enter="canStartClean && confirmClean()"
          />
        </template>
        <div class="modal-actions">
          <button @click="showConfirm = false">取消</button>
          <button class="primary" :disabled="!canStartClean" @click="confirmClean">
            开始清理
          </button>
        </div>
      </div>
    </div>

    <!-- 清理报告（共享组件；关闭 = 隐藏并自动重扫） -->
    <ReportModal v-if="showReport && report" :report="report" @close="closeReport" />
  </section>
</template>

<style scoped>
.view {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-shrink: 0;
}

/* ghost 按钮统一由全局 button.ghost 提供（hover = accent-soft 淡底） */

.status {
  color: var(--accent);
}

.big-status {
  font-size: var(--font-md);
}

.error {
  color: var(--danger);
}

/* 扫描进度条：4px 细条，填充松绿（版式规则 8） */
.scan-bar {
  height: 4px;
  border-radius: var(--radius-xs);
  background: var(--border);
  overflow: hidden;
  flex-shrink: 0;
}

.scan-bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: var(--radius-xs);
  transition: width var(--dur) var(--ease);
}

.result {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

/* 主视觉区：英雄数字（34px 展示级）与 12px 说明差近 3 倍字号（版式规则 1） */
.hero {
  display: flex;
  align-items: center;
  gap: var(--space-6);
  padding: var(--space-5) var(--space-6);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-lg);
}

.hero-main {
  margin-right: auto;
}

.hero-label {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.hero-number {
  font-size: 34px; /* 视图内英雄数字，与 --font-display 同为展示级 */
  font-weight: 700;
  letter-spacing: -0.01em;
  font-variant-numeric: tabular-nums;
  color: var(--text);
  line-height: 1.1;
  margin: var(--space-1) 0;
}

.hero-sub {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.clean-now {
  font-size: var(--font-lg);
  padding: var(--space-3) var(--space-8);
  border-radius: var(--radius);
}

.empty {
  color: var(--success);
  font-size: var(--font-md);
  padding: var(--space-8) var(--space-6);
  text-align: center;
}

.list-hint {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  flex-shrink: 0;
}

.select-all {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
  user-select: none;
  font-size: var(--font-sm);
}

.hint-text {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: var(--space-2) var(--space-4);
}

.category {
  margin-bottom: var(--space-4);
}

.category-title {
  font-size: var(--font-sm);
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--text-secondary);
  margin: var(--space-2) 0 var(--space-1);
}

.group-row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius);
  cursor: pointer;
  user-select: none;
  border: 1px solid transparent;
  transition: background-color var(--dur-fast) var(--ease),
    border-color var(--dur-fast) var(--ease);
}

/* hover / 选中统一 accent-soft 淡底；选中另加 border-strong 描边（版式规则 5） */
.group-row:hover {
  background: var(--accent-soft);
}

.group-row.selected {
  background: var(--accent-soft);
  border-color: var(--border-strong);
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
  gap: var(--space-2);
}

.group-title {
  font-weight: 500;
}

/* 徽章：--font-xs + 对应语义 soft 底（soft 底由语义色混 surface 得出，
   与 DirTree.vue 徽章同一配方），视觉重量随风险递增 */
.badge {
  padding: 1px var(--space-2);
  font-size: var(--font-xs);
  font-weight: 500;
  line-height: 18px;
  border-radius: var(--radius-sm);
  white-space: nowrap;
}

.risk-low {
  color: var(--success);
  background: color-mix(in srgb, var(--success) 8%, var(--surface));
}

.risk-medium {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 8%, var(--surface));
}

.risk-high {
  color: var(--danger);
  background: var(--danger-soft);
}

.badge.scan-only {
  color: var(--text-secondary);
  background: var(--bg);
}

.problem-dot {
  color: var(--warning);
  font-size: var(--font-xs);
}

.group-sub {
  font-size: var(--font-xs);
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
  font-size: var(--font-xs);
  padding: var(--space-1) var(--space-2);
}

.detail-btn:hover {
  color: var(--accent);
}

.group-detail {
  padding: var(--space-1) var(--space-3) var(--space-2) 40px;
}

.detail-line {
  font-size: var(--font-xs);
  color: var(--text-secondary);
  padding: var(--space-1) 0;
  word-break: break-all;
}

.detail-line.problem {
  color: var(--warning);
}

.detail-label {
  display: inline-block;
  margin-right: var(--space-2);
  color: var(--text);
}

.detail-line.desc {
  color: var(--text);
}

.files {
  margin-top: var(--space-2);
  border-top: 1px dashed var(--border);
  padding-top: var(--space-2);
}

.file-row {
  display: flex;
  gap: var(--space-3);
  padding: var(--space-1) 0;
  font-size: var(--font-xs);
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
  background: var(--scrim);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 50;
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

.cancel-tag {
  font-size: var(--font-xs);
  font-weight: 400;
  color: var(--warning);
  margin-left: var(--space-2);
}

/* 确认清单：内嵌浅底列表，行距 4px 网格 */
.confirm-list {
  max-height: 180px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg);
  padding: var(--space-1) var(--space-3);
}

.confirm-row {
  display: flex;
  justify-content: space-between;
  font-size: var(--font-sm);
}

.confirm-item {
  padding: var(--space-1) 0;
}

.confirm-desc {
  font-size: var(--font-xs);
  color: var(--text-secondary);
  padding-top: var(--space-1);
}

/* 选项组：1px 细线分隔"清什么"与"怎么清"（版式规则 4） */
.option-group {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  border-top: 1px solid var(--border);
  padding-top: var(--space-3);
}

.option-explain {
  line-height: 1.6;
}

.targeted-btn {
  font-size: var(--font-lg);
  padding: var(--space-3) var(--space-5);
  border-radius: var(--radius);
}

.field {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.field-label {
  font-size: var(--font-xs);
  color: var(--text-secondary);
}

.targeted-summary {
  font-size: var(--font-md);
}

.targeted-problems {
  max-height: 96px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: var(--space-1) var(--space-3);
}

.targeted-files {
  max-height: 200px;
  overflow-y: auto;
  border-top: 1px dashed var(--border);
  padding-top: var(--space-2);
}

.scan-cancel {
  margin-left: var(--space-2);
  padding: var(--space-1) var(--space-3);
  font-size: var(--font-xs);
}

.confirm-title {
  display: flex;
  align-items: center;
  gap: var(--space-2);
}

/* 确认词输入框样式全部交给全局 input（含聚焦软环） */

.confirm-size {
  color: var(--text-secondary);
  font-variant-numeric: tabular-nums;
}

.modal-line {
  color: var(--text-secondary);
}

.modal-line strong {
  color: var(--text);
}

.option {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  cursor: pointer;
}

/* 警示分级：琥珀 = 需要确认的注意事项；danger-note 红块 = 永久删除等危险操作 */
.warn {
  color: var(--warning);
  font-size: var(--font-xs);
}

.warn.danger-note {
  color: var(--danger);
  background: var(--danger-soft);
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
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
  border-color: transparent;
}

.failures {
  max-height: 220px;
  overflow-y: auto;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: var(--space-2);
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.fail-row {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
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
}
</style>
