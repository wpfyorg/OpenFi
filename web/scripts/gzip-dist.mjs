// Post-build: gzip every file in dist/ and remove the original. The Rust binary embeds dist/
// and serves `<file>.gz` with Content-Encoding: gzip, so shipping only the compressed copies
// keeps the router binary small (every browser accepts gzip).
import { readdirSync, readFileSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { gzipSync, constants } from "node:zlib";

const walk = (dir) =>
  readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    return statSync(p).isDirectory() ? walk(p) : [p];
  });

let before = 0;
let after = 0;
for (const file of walk(new URL("../dist", import.meta.url).pathname)) {
  if (file.endsWith(".gz") || file.split("/").pop().startsWith(".")) continue;
  const raw = readFileSync(file);
  const gz = gzipSync(raw, { level: constants.Z_BEST_COMPRESSION });
  writeFileSync(`${file}.gz`, gz);
  unlinkSync(file);
  before += raw.length;
  after += gz.length;
}
console.log(`gzip-dist: ${(before / 1024).toFixed(0)} KB -> ${(after / 1024).toFixed(0)} KB`);
