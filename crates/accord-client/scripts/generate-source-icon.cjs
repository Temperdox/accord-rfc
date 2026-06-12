/**
 * Generates a simple 1024x1024 source PNG (solid Accord-indigo) used as input to
 * `tauri icon`, which produces the full platform icon set. Uses only Node
 * built-ins (zlib) so there is no extra dependency.
 */
const zlib = require("zlib");
const fs = require("fs");
const path = require("path");

const W = 1024;
const H = 1024;
const [R, G, B, A] = [0x7c, 0x5c, 0xff, 0xff]; // --accent

// Build raw RGBA scanlines, each prefixed with a 0 filter byte.
const raw = Buffer.alloc((W * 4 + 1) * H);
let o = 0;
for (let y = 0; y < H; y++) {
  raw[o++] = 0; // filter type: none
  for (let x = 0; x < W; x++) {
    raw[o++] = R;
    raw[o++] = G;
    raw[o++] = B;
    raw[o++] = A;
  }
}

// CRC32 (PNG polynomial).
const crcTable = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(type, "ascii");
  const body = Buffer.concat([t, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type: RGBA
const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", zlib.deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]);

const out = path.join(__dirname, "..", "app-icon.png");
fs.writeFileSync(out, png);
console.log("wrote", out, png.length, "bytes");
