/** 字节数 → 人类可读大小 */
export function humanSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  const text = i === 0 || v >= 100 ? Math.round(v).toString() : v.toFixed(1);
  return `${text} ${units[i]}`;
}

/** 数字 → 千分位字符串 */
export function humanCount(n: number): string {
  return n.toLocaleString("zh-CN");
}

/** 毫秒 → 人类可读时长（进度行"已用 mm:ss"用） */
export function humanDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m > 0 ? `${m} 分 ${s} 秒` : `${s} 秒`;
}
