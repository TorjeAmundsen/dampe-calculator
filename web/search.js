// Follows the presses, with the solver in WebAssembly, off the page's thread. A scan of every state takes
// about 17 s on one thread, so its parts go to workers of their own, on half of the cores like the program.
"use strict";

const TICK_MS = 50;

let wasm = null;
let started = 0;
let pool = [];
let waiting = [];
// Which read a scan was for: its results are no use to a newer one
let readId = 0;

const secondsNow = () => (performance.timeOrigin + performance.now() - started) / 1000;

function output() {
  return new Uint8Array(wasm.memory.buffer, wasm.out_ptr(), wasm.out_len());
}

function runScan({ now, presses }, id) {
  const scanStarted = performance.now();
  const states = wasm.states();
  const chunk = Math.ceil(states / pool.length);
  let left = pool.length;
  const parts = [];
  pool.forEach((worker, i) => {
    worker.onmessage = ({ data }) => {
      parts.push(new Uint8Array(data));
      left -= 1;
      if (left === 0 && id === readId) {
        finishScan(parts, performance.now() - scanStarted);
      }
    };
    worker.postMessage({ from: i * chunk, to: Math.min((i + 1) * chunk, states), presses, now });
  });
}

function finishScan(parts, milliseconds) {
  const bytes = parts.reduce((sum, part) => sum + part.length, 0);
  const pointer = wasm.alloc(bytes);
  let at = pointer;
  for (const part of parts) {
    new Uint8Array(wasm.memory.buffer, at, part.length).set(part);
    at += part.length;
  }
  wasm.finish_scan(pointer, bytes / 8, milliseconds / 1000);
}

function tick() {
  wasm.tick(secondsNow());
  const bytes = output();
  if (bytes.length === 0) {
    return;
  }
  const { events, readings, scan } = JSON.parse(new TextDecoder().decode(bytes));
  if (events.length > 0 || readings) {
    postMessage({ events, readings });
  }
  if (scan) {
    runScan(scan, readId);
  }
}

function handle(message) {
  if (message.start) {
    started = message.start.started;
    readId += 1;
    wasm.start(message.start.lead);
  } else if (message.press) {
    const { count, seconds } = message.press;
    const flipped = wasm.press(count, seconds) === 1;
    postMessage({ pressed: { count, flipped } });
  }
}

async function load() {
  const module = await WebAssembly.compileStreaming(fetch("dampe_web.wasm"));
  wasm = (await WebAssembly.instantiate(module)).exports;
  const workers = Math.max(Math.floor((navigator.hardwareConcurrency || 4) / 2), 1);
  pool = Array.from({ length: workers }, () => {
    const worker = new Worker("scan.js");
    worker.postMessage({ module });
    return worker;
  });
  waiting.forEach(handle);
  waiting = null;
  setInterval(tick, TICK_MS);
}

onmessage = ({ data }) => {
  if (waiting) {
    waiting.push(data);
  } else {
    handle(data);
  }
};

load().catch((error) => postMessage({ failed: String(error) }));
