// 渲染 Purimaster 定稿 Logo 为 1024×1024 应用图标 PNG（透明背景）。
// 用法: node scripts/render-icon.mjs
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Resvg } from "@resvg/resvg-js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const svgPath = path.join(root, "design", "logos", "final.svg");
const outPath = path.join(root, "design", "logos", "icon-1024.png");

const svg = readFileSync(svgPath, "utf8");

// resvg 默认透明背景（不设置 background 即可）；SVG 为纯 path/rect 图形、无文字，
// 关闭系统字体加载以加速。viewBox 为 512×512 正方形，按宽度 fit 到 1024 即得 1024×1024。
const resvg = new Resvg(svg, {
  fitTo: { mode: "width", value: 1024 },
  font: { loadSystemFonts: false },
});

const rendered = resvg.render();
const png = rendered.asPng();

if (rendered.width !== 1024 || rendered.height !== 1024) {
  throw new Error(
    `渲染尺寸不符: ${rendered.width}x${rendered.height}，期望 1024x1024`,
  );
}

writeFileSync(outPath, png);
console.log(
  `OK: ${path.relative(root, outPath)} (${png.length} bytes, ${rendered.width}x${rendered.height})`,
);
