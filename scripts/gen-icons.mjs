// 生成 Tauri 所需图标（纯 Node，无依赖）：应用图标 + 托盘两态
// 图标源 = scripts/assets/app-icon-source.png（Trae CN 官方 logo 提取 + 白底蓝色 recolor，512x512）
// 用法: node scripts/gen-icons.mjs
import { inflateSync, deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync, readFileSync } from "node:fs";
import path from "node:path";

const OUT = path.resolve("src-tauri/icons");
const SOURCE = path.resolve("scripts/assets/app-icon-source.png");
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

// ── 源图：解码 + 双线性缩放 ──
const clamp01 = (x) => Math.max(0, Math.min(1, x));
const smooth = (edge0, edge1, x) => {
  const t = clamp01((x - edge0) / (edge1 - edge0));
  return t * t * (3 - 2 * t);
};

function decodePng(buf) {
  let off = 8;
  let w = 0, h = 0, depth = 0, type = 0, interlace = 0;
  let palette = null;
  const idats = [];
  while (off < buf.length) {
    const len = buf.readUInt32BE(off);
    const typeStr = buf.subarray(off + 4, off + 8).toString("ascii");
    const data = buf.subarray(off + 8, off + 8 + len);
    if (typeStr === "IHDR") {
      w = data.readUInt32BE(0);
      h = data.readUInt32BE(4);
      depth = data[8];
      type = data[9];
      interlace = data[12];
    } else if (typeStr === "PLTE") {
      palette = Buffer.from(data);
    } else if (typeStr === "IDAT") {
      idats.push(data);
    } else if (typeStr === "IEND") break;
    off += 12 + len;
  }
  if (depth !== 8) throw new Error(`unsupported bit depth ${depth}`);
  if (interlace !== 0) throw new Error("interlaced png unsupported");
  const channels = type === 6 ? 4 : type === 2 ? 3 : type === 3 ? 1 : null;
  if (!channels) throw new Error(`unsupported color type ${type}`);
  const raw = inflateSync(Buffer.concat(idats));
  const stride = w * channels;
  const px = Buffer.alloc(w * h * 4);
  const paeth = (a, b, c) => {
    const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
    return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
  };
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    const cur = Buffer.alloc(stride);
    for (let i = 0; i < stride; i++) {
      const x = line[i];
      const a = i >= channels ? cur[i - channels] : 0;
      const b = prev[i];
      const c = i >= channels ? prev[i - channels] : 0;
      cur[i] =
        f === 0 ? x : f === 1 ? x + a : f === 2 ? x + b : f === 3 ? x + ((a + b) >> 1) : x + paeth(a, b, c);
    }
    for (let x = 0; x < w; x++) {
      const o = (y * w + x) * 4;
      if (type === 6) {
        px[o] = cur[x * 4]; px[o + 1] = cur[x * 4 + 1]; px[o + 2] = cur[x * 4 + 2]; px[o + 3] = cur[x * 4 + 3];
      } else if (type === 2) {
        px[o] = cur[x * 3]; px[o + 1] = cur[x * 3 + 1]; px[o + 2] = cur[x * 3 + 2]; px[o + 3] = 255;
      } else {
        const pi = cur[x] * 3;
        px[o] = palette[pi]; px[o + 1] = palette[pi + 1]; px[o + 2] = palette[pi + 2]; px[o + 3] = 255;
      }
    }
    prev = cur;
  }
  return { w, h, rgba: px };
}

function resizeBilinear(src, size) {
  const out = Buffer.alloc(size * size * 4);
  const { w, h, rgba } = src;
  for (let y = 0; y < size; y++) {
    const gy = ((y + 0.5) / size) * h - 0.5;
    const y0 = Math.max(0, Math.min(h - 1, Math.floor(gy)));
    const y1 = Math.min(h - 1, y0 + 1);
    const fy = gy - Math.floor(gy);
    for (let x = 0; x < size; x++) {
      const gx = ((x + 0.5) / size) * w - 0.5;
      const x0 = Math.max(0, Math.min(w - 1, Math.floor(gx)));
      const x1 = Math.min(w - 1, x0 + 1);
      const fx = gx - Math.floor(gx);
      const o = (y * size + x) * 4;
      for (let c = 0; c < 4; c++) {
        const p00 = rgba[(y0 * w + x0) * 4 + c];
        const p01 = rgba[(y0 * w + x1) * 4 + c];
        const p10 = rgba[(y1 * w + x0) * 4 + c];
        const p11 = rgba[(y1 * w + x1) * 4 + c];
        out[o + c] = Math.round(
          p00 * (1 - fx) * (1 - fy) + p01 * fx * (1 - fy) + p10 * (1 - fx) * fy + p11 * fx * fy,
        );
      }
    }
  }
  return out;
}

const SOURCE_IMG = decodePng(readFileSync(SOURCE));
if (SOURCE_IMG.w !== SOURCE_IMG.h) throw new Error("icon source must be square");

// 反色源：蓝底 + 白色图形。≤48px 的场景（窗口标题栏 16px / 托盘 32px）白底蓝图形
// 会糊成一团无法辨认（2026-09-29 用户反馈），小尺寸统一用反色版。
function invertSource(src) {
  const out = Buffer.from(src.rgba);
  const lum = (r, g, b) => 0.299 * r + 0.587 * g + 0.114 * b;
  for (let i = 0; i < out.length; i += 4) {
    if (out[i + 3] === 0) continue;
    // 白(255)→0 映射为蓝底；蓝(≈96)→1 映射为白图形；中间为抗锯齿过渡
    const t = Math.max(0, Math.min(1, (255 - lum(out[i], out[i + 1], out[i + 2])) / (255 - 96)));
    out[i] = Math.round(BLUE[0] + (255 - BLUE[0]) * t);
    out[i + 1] = Math.round(BLUE[1] + (255 - BLUE[1]) * t);
    out[i + 2] = Math.round(BLUE[2] + (255 - BLUE[2]) * t);
  }
  return { w: src.w, h: src.h, rgba: out };
}
const BLUE = [0x25, 0x63, 0xeb];
const INVERTED_SRC = invertSource(SOURCE_IMG);

/**
 * 画应用图标：>48px 用白底蓝 logo（桌面/文件属性），≤48px 用蓝底白图形反色版
 * （窗口标题栏 16px / 托盘 32px，保证小尺寸可辨识）；redDot 时右上角加红点（托盘未签态）
 */
function renderIcon(size, { redDot = false } = {}) {
  const px = resizeBilinear(size <= 48 ? INVERTED_SRC : SOURCE_IMG, size);
  const s = size;
  if (redDot) {
    for (let y = 0; y < s; y++) {
      for (let x = 0; x < s; x++) {
        const i = (y * s + x) * 4;
        const dxr = x + 0.5 - s * 0.78, dyr = y + 0.5 - s * 0.22;
        const dr = Math.hypot(dxr, dyr) - s * 0.16;
        const aDot = Math.round(255 * (1 - smooth(-0.7, 0.7, dr)));
        if (aDot > 0) {
          const a = aDot / 255;
          px[i] = Math.round(0xef * a + px[i] * (1 - a));
          px[i + 1] = Math.round(0x44 * a + px[i + 1] * (1 - a));
          px[i + 2] = Math.round(0x44 * a + px[i + 2] * (1 - a));
          px[i + 3] = Math.max(px[i + 3], aDot);
        }
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
// 同步一份到前端 public/，供侧边栏 logo 引用（与桌面图标同源）
mkdirSync(path.resolve("public"), { recursive: true });
writeFileSync(path.resolve("public/app-icon.png"), readFileSync(path.join(OUT, "icon.png")));
console.log("icons written to", OUT);
