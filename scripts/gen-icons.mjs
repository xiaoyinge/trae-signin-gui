// 生成 Tauri 所需图标（纯 Node，无依赖）：应用图标 + 托盘两态
// 用法: node scripts/gen-icons.mjs
import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";

const OUT = path.resolve("src-tauri/icons");
mkdirSync(OUT, { recursive: true });

// ── PNG 编码（RGBA8） ──
const CRC_TABLE = (() => {
  const t = new Int32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}
function encodePng(w, h, rgba) {
  const sig = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 4 + 1)] = 0; // filter none
    rgba.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  }
  return Buffer.concat([
    sig,
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ── 图标绘制 ──
const clamp01 = (x) => Math.max(0, Math.min(1, x));
const smooth = (edge0, edge1, x) => {
  const t = clamp01((x - edge0) / (edge1 - edge0));
  return t * t * (3 - 2 * t);
};
const distSeg = (px, py, x1, y1, x2, y2) => {
  const dx = x2 - x1, dy = y2 - y1;
  const L2 = dx * dx + dy * dy || 1e-9;
  let t = ((px - x1) * dx + (py - y1) * dy) / L2;
  t = Math.max(0, Math.min(1, t));
  const gx = x1 + t * dx - px, gy = y1 + t * dy - py;
  return Math.hypot(gx, gy);
};

/**
 * 画一个"圆角方块 + 白色对勾"图标；redDot 时右上角加红点
 * 底色 teal #0D9488，红点 #EF4444
 */
function renderIcon(size, { redDot = false } = {}) {
  const px = Buffer.alloc(size * size * 4);
  const s = size;
  const cx = s / 2, cy = s / 2;
  const r = s * 0.46;                    // 圆角半径度量
  const corner = s * 0.22;               // 圆角
  const setPx = (i, [R, G, B, A]) => {
    const a = A / 255;
    px[i] = Math.round(R * a + px[i] * (1 - a));
    px[i + 1] = Math.round(G * a + px[i + 1] * (1 - a));
    px[i + 2] = Math.round(B * a + px[i + 2] * (1 - a));
    px[i + 3] = Math.max(px[i + 3], A);
  };
  for (let y = 0; y < s; y++) {
    for (let x = 0; x < s; x++) {
      const i = (y * s + x) * 4;
      // 到圆角方形内部的符号距离（简化：超椭圆/圆角处理）
      const nx = Math.abs(x + 0.5 - cx), ny = Math.abs(y + 0.5 - cy);
      const qx = Math.max(nx - (s / 2 - corner), 0);
      const qy = Math.max(ny - (s / 2 - corner), 0);
      const d = Math.hypot(qx, qy) - corner; // >0 在外
      const alpha = Math.round(255 * (1 - smooth(-0.7, 0.7, d)));
      if (alpha > 0) setPx(i, [0x0d, 0x94, 0x88, alpha]);
      // 白色对勾（全部使用归一化 0..1 坐标）
      const u = (x + 0.5) / s, v = (y + 0.5) / s;
      const w = 0.085;
      const dCheck = Math.min(
        distSeg(u, v, 0.28, 0.53, 0.44, 0.68),
        distSeg(u, v, 0.44, 0.68, 0.72, 0.34),
      );
      const aCheck = Math.round(255 * (1 - smooth(w * 0.55, w * 0.85, dCheck)));
      if (aCheck > 0) setPx(i, [255, 255, 255, aCheck]);
      // 红点（托盘未签状态）
      if (redDot) {
        const dxr = (x + 0.5 - s * 0.78) , dyr = (y + 0.5 - s * 0.22);
        const dr = Math.hypot(dxr, dyr) - s * 0.16;
        const aDot = Math.round(255 * (1 - smooth(-0.7, 0.7, dr)));
        if (aDot > 0) setPx(i, [0xef, 0x44, 0x44, aDot]);
      }
    }
  }
  return encodePng(s, s, px);
}

// ICO = ICONDIR + entries（内嵌 PNG，Vista+ 支持）
function makeIco(pngBuffers) {
  const count = pngBuffers.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(count, 4);
  const entries = [];
  let offset = 6 + count * 16;
  const blobs = [];
  for (const [size, png] of pngBuffers) {
    const e = Buffer.alloc(16);
    e[0] = size === 256 ? 0 : size; // width
    e[1] = size === 256 ? 0 : size;
    e[4] = 1; e[6] = 32; // planes, bpp
    e.writeUInt32LE(png.length, 8);
    e.writeUInt32LE(offset, 12);
    entries.push(e);
    blobs.push(png);
    offset += png.length;
  }
  return Buffer.concat([header, ...entries, ...blobs]);
}

writeFileSync(path.join(OUT, "icon.png"), renderIcon(256));
writeFileSync(path.join(OUT, "128x128.png"), renderIcon(128));
writeFileSync(path.join(OUT, "128x128@2x.png"), renderIcon(256));
writeFileSync(path.join(OUT, "32x32.png"), renderIcon(32));
writeFileSync(path.join(OUT, "icon.ico"), makeIco([[256, renderIcon(256)], [32, renderIcon(32)]]));
writeFileSync(path.join(OUT, "tray-normal.png"), renderIcon(32));
writeFileSync(path.join(OUT, "tray-red.png"), renderIcon(32, { redDot: true }));
console.log("icons written to", OUT);
