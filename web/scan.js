// One part of a scan, for search.js.
"use strict";

// Browsers start WebAssembly on a slow compiler, and only use the fast code it swaps in on the next call.
// One long call never gets it, and took 6 times as long.
const SEEDS_PER_CALL = 1 << 14;

let wasm = null;

function scanSeeds(from, to, presses, now) {
  const pointer = wasm.alloc(presses.length * 8);
  new Float64Array(wasm.memory.buffer, pointer, presses.length).set(presses);
  wasm.scan_range(from, to, pointer, presses.length / 2, now);
  return new Uint8Array(wasm.memory.buffer, wasm.out_ptr(), wasm.out_len()).slice();
}

onmessage = async ({ data }) => {
  if (data.module) {
    wasm = (await WebAssembly.instantiate(data.module)).exports;
    return;
  }
  const { from, to, presses, now } = data;
  const parts = [];
  for (let seed = from; seed < to; seed += SEEDS_PER_CALL) {
    parts.push(scanSeeds(seed, Math.min(seed + SEEDS_PER_CALL, to), presses, now));
  }
  const found = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    found.set(part, at);
    at += part.length;
  }
  postMessage(found.buffer, [found.buffer]);
};
