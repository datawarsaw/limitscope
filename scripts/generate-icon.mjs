// Renders the LimitScope brand assets without any image libraries:
// the "Dock Tick" quota-bar glyph (dark graphite tile + horizontal quota
// track + teal usage fill + threshold tick), 3x supersampled for smooth
// edges. Writes the 1024px app-icon master (all platform icons are derived
// from it via `npm run icon` → `tauri icon`) plus the dedicated tray PNGs
// (16/20/24/32, tile + monochrome) used for tray-legibility validation.
// Geometry mirrors app-icon.svg — change both together.
import { writeFileSync, mkdirSync } from "node:fs";
import { deflateSync } from "node:zlib";

// ---------- glyph geometry (1024-unit design space) ----------
const CANVAS = 1024;

const TILE_RING = { x0: 32, y0: 32, x1: 992, y1: 992, r: 208, color: "#3a3b44" };
const TILE_FACE = { x0: 56, y0: 56, x1: 968, y1: 968, r: 192, color: "#1e1f24" };
// Quota track: 640 wide, 128 tall, spanning x 192..832.
const TRACK = { x0: 192, y0: 448, x1: 832, y1: 576, r: 64, color: "#40414a" };
// Usage fill: 60% of the track, brand teal.
const FILL = { x0: 192, y0: 448, x1: 576, y1: 576, r: 64, color: "#2dd4bf" };
// Attention-threshold tick at 80% of the track (x = 704), slightly taller.
const TICK = { x0: 672, y0: 400, x1: 736, y1: 624, r: 32, color: "#e9e9ee" };

// Monochrome tray variant: white marks on transparent, no tile — legible on
// a dark taskbar; the tiled default remains the shipped tray icon, which
// carries its own background and survives both taskbar polarities.
const MONO_TRACK = { ...TRACK, color: "rgba(255,255,255,0.30)" };
const MONO_FILL = { ...FILL, color: "#ffffff" };
const MONO_TICK = { ...TICK, color: "#ffffff" };

function hex(color) {
  return [
    parseInt(color.slice(1, 3), 16),
    parseInt(color.slice(3, 5), 16),
    parseInt(color.slice(5, 7), 16),
  ];
}

function rgba(color) {
  if (color.startsWith("#")) {
    const [r, g, b] = hex(color);
    return { r, g, b, a: 255 };
  }
  const match = /rgba\((\d+),(\d+),(\d+),([\d.]+)\)/.exec(color);
  if (!match) throw new Error(`unsupported color: ${color}`);
  return {
    r: Number(match[1]),
    g: Number(match[2]),
    b: Number(match[3]),
    a: Math.round(Number(match[4]) * 255),
  };
}

function insideRoundedRect(px, py, x0, y0, x1, y1, r) {
  const cx = Math.min(Math.max(px, x0 + r), x1 - r);
  const cy = Math.min(Math.max(py, y0 + r), y1 - r);
  const dx = px - cx;
  const dy = py - cy;
  return dx * dx + dy * dy <= r * r;
}

function insideRoundedRectShape(px, py, shape) {
  return insideRoundedRect(px, py, shape.x0, shape.y0, shape.x1, shape.y1, shape.r);
}

const markShapes = [
  { ...TRACK, inside: insideRoundedRectShape },
  { ...FILL, inside: insideRoundedRectShape },
  { ...TICK, inside: insideRoundedRectShape },
];

const tileShapes = [
  { ...TILE_RING, inside: insideRoundedRectShape },
  { ...TILE_FACE, inside: insideRoundedRectShape },
  ...markShapes,
];

const monoShapes = [
  { ...MONO_TRACK, inside: insideRoundedRectShape },
  { ...MONO_FILL, inside: insideRoundedRectShape },
  { ...MONO_TICK, inside: insideRoundedRectShape },
];

/**
 * Renders shapes into a straight-alpha RGBA buffer, size × size, scaling the
 * 1024-unit design space into the canvas and supersampling SS× per axis.
 * `padding` keeps a transparent margin (fraction of the canvas) so small
 * marks never touch the edge.
 */
function render(size, shapes, { ss = 3, padding = 0 } = {}) {
  const scale = ((1 - padding * 2) * size) / CANVAS;
  const offset = padding * size;
  const pixels = Buffer.alloc(size * size * 4);
  const samples = ss * ss;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let rSum = 0;
      let gSum = 0;
      let bSum = 0;
      let aSum = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const px = (x + (sx + 0.5) / ss - offset) / scale;
          const py = (y + (sy + 0.5) / ss - offset) / scale;
          let color = null;
          for (const shape of shapes) {
            if (shape.inside(px, py, shape)) color = shape.color; // later shapes win
          }
          if (color) {
            const { r, g, b, a } = rgba(color);
            // Premultiplied accumulation so partial coverage stays correct.
            rSum += (r * a) / 255;
            gSum += (g * a) / 255;
            bSum += (b * a) / 255;
            aSum += a;
          }
        }
      }
      const i = (y * size + x) * 4;
      const alpha = Math.round(aSum / samples);
      if (alpha === 0) continue;
      pixels[i] = Math.min(255, Math.round((rSum / samples) * (255 / alpha)));
      pixels[i + 1] = Math.min(255, Math.round((gSum / samples) * (255 / alpha)));
      pixels[i + 2] = Math.min(255, Math.round((bSum / samples) * (255 / alpha)));
      pixels[i + 3] = alpha;
    }
  }
  return pixels;
}

// ---------- minimal PNG encoder (RGBA, 8-bit, no filtering) ----------
let crcTable;
function crc32(buf) {
  if (!crcTable) {
    crcTable = new Int32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c;
    }
  }
  let crc = -1;
  for (let i = 0; i < buf.length; i++) {
    crc = (crc >>> 8) ^ crcTable[(crc ^ buf[i]) & 0xff];
  }
  return (crc ^ -1) >>> 0;
}

function chunk(type, data) {
  const out = Buffer.alloc(8 + data.length + 4);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}

function encodePng(size, pixels) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA

  const stride = size * 4 + 1;
  const raw = Buffer.alloc(size * stride);
  for (let y = 0; y < size; y++) {
    pixels.copy(raw, y * stride + 1, y * size * 4, (y + 1) * size * 4);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ---------- outputs ----------
writeFileSync(
  new URL("../app-icon.png", import.meta.url),
  encodePng(1024, render(1024, tileShapes)),
);
console.log("wrote app-icon.png (1024x1024)");

const trayDir = new URL("../src-tauri/icons/tray/", import.meta.url);
mkdirSync(trayDir, { recursive: true });
for (const size of [16, 20, 24, 32]) {
  const tile = encodePng(size, render(size, tileShapes, { ss: 4 }));
  const mono = encodePng(size, render(size, monoShapes, { ss: 4, padding: 0.0625 }));
  writeFileSync(new URL(`limitscope-tray-${size}.png`, trayDir), tile);
  writeFileSync(new URL(`limitscope-tray-mono-${size}.png`, trayDir), mono);
  console.log(`wrote limitscope-tray-${size}.png + mono (${size}x${size})`);
}
