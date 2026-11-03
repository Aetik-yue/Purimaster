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
