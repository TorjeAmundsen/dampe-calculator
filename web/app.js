// The page: what crates/dampe/src/manuallive.rs and the manual layout of main.slint are to the program.
// The state of the read is worked out in search.js. Nothing is ever sent to the game.
"use strict";

const WAITING = "Talk to Dampe, then press once Navi is done";
const DEFAULT_LAG_MS = 330;
const INPUT_LATENCY_MS = { n64: 100, vc: 150 };
const LAG_RANGE_MS = [-500, 2000];
const RECENT_PRESSES = 6;
const SHOWN_ROWS = 3;
// The bar fills over this much of the countdown, and the moment to press stays up this long
const BAR_SECONDS = 5;
const PRESS_SECONDS = 0.6;
// A flash on each of the last seconds, and their tones
const PULSE_AT = [3, 2, 1];
const PULSE_SECONDS = 0.12;
const TICK_TONE = [880, 0.12];
const GO_TONE = [1760, 0.4];
const NOT_FOCUSED = "This tab isn't focused, so keys won't count";

const element = (id) => document.getElementById(id);

function stored(key, fallback) {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

function store(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private windows may not keep it, and everything still works for this visit
  }
}

function validLag(text) {
  const lag = Number(text);
  return text !== "" && Number.isFinite(lag) && lag >= LAG_RANGE_MS[0] && lag <= LAG_RANGE_MS[1] ? lag : null;
}

const settings = {
  platform: stored("platform", "vc") === "n64" ? "n64" : "vc",
  lag: validLag(stored("lag", "")) ?? DEFAULT_LAG_MS,
  sound: stored("sound", "off") === "on",
  volume: Number(stored("volume", "50")),
};

const leadSeconds = () => (INPUT_LATENCY_MS[settings.platform] + settings.lag) / 1000;

// All clocks count from here, since each worker's own performance.now() starts somewhere else
const started = performance.timeOrigin + performance.now();
const secondsNow = () => (performance.timeOrigin + performance.now() - started) / 1000;

const logLines = [];
function log(text) {
  const elapsed = secondsNow();
  const minutes = String(Math.floor(elapsed / 60)).padStart(2, "0");
  const seconds = (elapsed % 60).toFixed(1).padStart(4, "0");
  logLines.push(`[${minutes}:${seconds}] ${text}`);
}

// What's on the page
let status = { t: "collecting", n: 0 };
let counts = [];
let presses = 0;
let readings = null;
let cueAt = null;
let dugSaid = true;
let shownPercent = 0;
let warning = null;

function describe(status) {
  switch (status.t) {
    case "collecting":
      return status.n === 1 ? "1 flutter so far" : `${status.n} flutters so far`;
    case "narrowing":
      return `${status.n} states still fit`;
    default:
      return "Locked: waiting for a good moment";
  }
}

function setStatus(text, color = "white") {
  element("status").textContent = text;
  element("status").style.color = color;
}

const statusColor = (status) => (status.t === "locked" ? "var(--reached)" : "white");
const showStatus = () => setStatus(describe(status), statusColor(status));

function setLatest(text) {
  warning = null;
  element("latest").textContent = text;
  element("latest").classList.remove("warning");
}

function setWarning(text) {
  warning = text;
  element("latest").textContent = text;
  element("latest").classList.add("warning");
}

const showRecent = () => (element("recent").textContent = counts.slice(-RECENT_PRESSES).join(" "));

function setLikely(percent) {
  element("box").classList.toggle("likely", percent !== null);
  element("likely-text").textContent = percent === null ? "" : `${percent}% chance`;
}

const setMargin = (ms) => (element("margin").textContent = ms === null ? "" : `+-${ms.toFixed(0)} ms`);

function clearCue() {
  cueAt = null;
  setLikely(null);
  setMargin(null);
}

// Sound

let audio = null;
let playing = [];

function audioContext() {
  audio ??= new AudioContext();
  // Browsers only let it start from a key press or a click, which everything here comes after
  audio.resume();
  return audio;
}

function playTone([frequency, seconds], at) {
  const context = audioContext();
  const oscillator = new OscillatorNode(context, { frequency });
  const gain = new GainNode(context, { gain: (settings.volume / 100) * 0.3 });
  oscillator.connect(gain).connect(context.destination);
  oscillator.start(at);
  oscillator.stop(at + seconds);
  return oscillator;
}

function stopCountdownSound() {
  for (const oscillator of playing) {
    try {
      oscillator.stop();
    } catch {
      // It already finished
    }
  }
  playing = [];
}

/** The tones for a countdown reaching zero at `seconds`. Ones whose time has passed are left out. */
function startCountdownSound(seconds) {
  stopCountdownSound();
  if (!settings.sound) {
    return;
  }
  const context = audioContext();
  const offset = context.currentTime - secondsNow();
  const tones = PULSE_AT.map((before) => [TICK_TONE, seconds - before]).concat([[GO_TONE, seconds]]);
  playing = tones.filter(([, at]) => at > secondsNow()).map(([tone, at]) => playTone(tone, at + offset));
}

// The search

const search = new Worker("search.js");
search.onerror = () => setWarning("Couldn't start: this has to be opened from a web server");
search.onmessage = ({ data }) => {
  if (data.failed) {
    setWarning(`Couldn't start: ${data.failed}`);
  } else if (data.pressed) {
    showPress(data.pressed.count, data.pressed.flipped);
  } else {
    for (const event of data.events) {
      handle(event);
    }
    if (data.readings) {
      readings = data.readings;
      showReadings();
    }
  }
};

function startRead() {
  search.postMessage({ start: { lead: leadSeconds(), started } });
}

function startOver() {
  stopCountdownSound();
  clearCue();
  status = { t: "collecting", n: 0 };
  counts = [];
  presses = 0;
  readings = null;
  dugSaid = true;
  showRecent();
  if (warning !== NOT_FOCUSED) {
    setLatest("");
  }
  setStatus(WAITING);
  showArea();
  log("Starting over.");
  startRead();
}

function showPress(count, flipped) {
  if (flipped) {
    counts[counts.length - 1] = 7 - counts[counts.length - 1];
    setLatest(`Flutter ${presses}: now ${counts[counts.length - 1]}`);
  } else {
    counts.push(count);
    presses += 1;
    setLatest(`Flutter ${presses}: pressed ${count}`);
    if (presses === 1) {
      setStatus(describe({ t: "collecting", n: 1 }));
    }
  }
  showRecent();
}

const shownPercentOf = (chance) => Math.min(Math.round(chance * 100), 99);

function runCue(boxSeconds, margin) {
  const at = boxSeconds - leadSeconds();
  cueAt = at;
  dugSaid = false;
  startCountdownSound(at);
  setMargin(margin);
  return at;
}

function handle(event) {
  switch (event.t) {
    case "collecting":
    case "narrowing":
    case "locked": {
      status = event;
      log(describe(status));
      showStatus();
      const top = readings?.top[0];
      if (status.t === "locked" && top) {
        log(`Locked on ${top.seed.toString(16).padStart(6, "0")}, Link ${top.link}`);
      }
      break;
    }
    case "cue": {
      const at = runCue(event.seconds, event.margin);
      log(`Press A at zero, in ${(at - secondsNow()).toFixed(1)} s. +-${event.margin.toFixed(0)} ms.`);
      setLikely(null);
      setStatus("Locked: follow the countdown", "var(--reached)");
      break;
    }
    case "likely": {
      const at = runCue(event.seconds, event.margin);
      const percent = shownPercentOf(event.chance);
      log(`${percent}%: press A at zero, in ${(at - secondsNow()).toFixed(1)} s.`);
      shownPercent = percent;
      setLikely(percent);
      setStatus(`${percent}% chance`, "var(--likely)");
      break;
    }
    case "chance": {
      const percent = shownPercentOf(event.chance);
      if (percent !== shownPercent) {
        shownPercent = percent;
        setLikely(percent);
        setStatus(`${percent}% chance`, "var(--likely)");
      }
      break;
    }
    case "likelyCancelled":
      stopCountdownSound();
      clearCue();
      log("Not likely any more. Don't press yet.");
      setStatus("Keep pressing");
      break;
    case "likelyEnded":
      setLikely(null);
      showStatus();
      break;
    case "cueCancelled":
      stopCountdownSound();
      clearCue();
      log("Presses don't match. Don't press yet.");
      setStatus("Cancelled: don't press yet", "var(--likely)");
      break;
  }
}

// What's drawn

function percentText(share) {
  const percent = share * 100;
  if (percent >= 99.95) return ">99.9%";
  if (percent > 0 && percent < 0.1) return "<0.1%";
  return `${percent.toFixed(1)}%`;
}

let settingsOpen = false;

function showArea() {
  const hasReadings = Boolean(readings && readings.top.length > 0);
  element("settings").hidden = !settingsOpen;
  element("help").hidden = settingsOpen || hasReadings;
  element("candidates").hidden = settingsOpen || !hasReadings;
}

function tally(count, name, color) {
  const number = document.createElement("span");
  const value = document.createElement("b");
  value.textContent = count;
  value.style.color = count === 0 ? "var(--faint)" : color;
  number.append(value, name);
  return number;
}

function showReadings() {
  showArea();
  if (!readings) {
    return;
  }
  const { total, notFound, top } = readings;
  element("summary").textContent =
    notFound === 0
      ? `${total} candidates`
      : `${total} candidates. Chance it's none of them: ${percentText(notFound)}`;
  const now = secondsNow();
  const rows = top.slice(0, SHOWN_ROWS).map((reading) => {
    const row = document.createElement("div");
    row.className = reading.proven ? "row proven" : "row";
    const fill = document.createElement("div");
    fill.className = "fill";
    fill.style.width = `${reading.share * 100}%`;
    const chance = document.createElement("span");
    chance.className = "chance";
    chance.textContent = percentText(reading.share);
    const tallies = document.createElement("span");
    tallies.className = "tallies";
    tallies.append(
      tally(reading.hits, "hit", "var(--reached)"),
      tally(reading.misses, "miss", "var(--blinking)"),
      tally(reading.strays, "stray", "var(--blob)"),
    );
    const time = document.createElement("span");
    time.className = "time";
    const left = reading.seconds === null ? null : reading.seconds - leadSeconds() - now;
    time.textContent = left === null ? "over 60 s" : left >= 0 ? `${left.toFixed(1)} s` : "passed";
    row.append(fill, chance, tallies, time);
    return row;
  });
  element("rows").replaceChildren(...rows);
}

function draw() {
  const now = secondsNow();
  const box = element("box");
  const countdown = element("countdown");
  const running = cueAt !== null && now - cueAt <= PRESS_SECONDS;
  if (cueAt !== null && now >= cueAt && !dugSaid) {
    // Nothing here can see a dig, and it ends what's been read
    dugSaid = true;
    setLatest("Dug? Start over before talking again");
    if (status.t === "locked") {
      showStatus();
    }
  }
  if (!running) {
    if (cueAt !== null) {
      clearCue();
    }
    box.classList.remove("press", "pulse");
    countdown.classList.add("idle");
    countdown.textContent = "No countdown yet";
    element("bar").style.width = "0";
    return;
  }
  const left = Math.max(cueAt - now, 0);
  const pressing = now >= cueAt;
  box.classList.toggle("press", pressing);
  box.classList.toggle("pulse", !pressing && PULSE_AT.some((second) => second - left >= 0 && second - left < PULSE_SECONDS));
  countdown.classList.remove("idle");
  countdown.textContent = pressing ? "PRESS A" : `${left.toFixed(1)} s`;
  element("bar").style.width = `${Math.min(Math.max(1 - left / BAR_SECONDS, 0), 1) * 100}%`;
}

// A timer, not animation frames: those stop when the window is covered, which capturing it can do
setInterval(draw, 16);
setInterval(() => {
  if (readings && !settingsOpen) {
    showReadings();
  }
}, 100);

// Input

function isTyping(target) {
  return target instanceof HTMLInputElement || target instanceof HTMLSelectElement;
}

window.addEventListener("keydown", (event) => {
  if (event.key === "F8") {
    event.preventDefault();
    if (!event.repeat) {
      startOver();
    }
  } else if ((event.key === "3" || event.key === "4") && !event.repeat && !isTyping(event.target)) {
    // Stamped when it was made, whatever the page is busy with
    const seconds = (performance.timeOrigin + event.timeStamp - started) / 1000;
    log(`press ${event.key} ${seconds.toFixed(3)}`);
    if (settings.sound) {
      audioContext();
    }
    search.postMessage({ press: { count: Number(event.key), seconds } });
  }
});

window.addEventListener("blur", () => setWarning(NOT_FOCUSED));
window.addEventListener("focus", () => {
  if (warning === NOT_FOCUSED) {
    setLatest("");
  }
});

function showTiming() {
  element("timing").textContent = `Input ${INPUT_LATENCY_MS[settings.platform]} ms + lag ${settings.lag} ms`;
}

element("start-over").addEventListener("click", (event) => {
  event.currentTarget.blur();
  startOver();
});
element("open-settings").addEventListener("click", (event) => {
  event.currentTarget.blur();
  settingsOpen = !settingsOpen;
  event.currentTarget.textContent = settingsOpen ? "Done" : "Settings";
  showReadings();
  showArea();
});
element("platform").value = settings.platform;
element("platform").addEventListener("change", (event) => {
  settings.platform = event.target.value;
  store("platform", settings.platform);
  showTiming();
  event.target.blur();
});
element("lag").value = settings.lag;
element("lag").addEventListener("input", (event) => {
  const lag = validLag(event.target.value);
  event.target.classList.toggle("bad", lag === null);
  if (lag !== null) {
    settings.lag = lag;
    store("lag", String(lag));
    showTiming();
  }
});
element("sound").checked = settings.sound;
element("sound").addEventListener("change", (event) => {
  settings.sound = event.target.checked;
  store("sound", settings.sound ? "on" : "off");
  if (settings.sound) {
    playTone(TICK_TONE, audioContext().currentTime);
  }
  event.target.blur();
});
element("volume").value = settings.volume;
element("volume").addEventListener("change", (event) => {
  settings.volume = Number(event.target.value);
  store("volume", String(settings.volume));
  if (settings.sound) {
    playTone(TICK_TONE, audioContext().currentTime);
  }
  event.target.blur();
});
element("log").addEventListener("click", (event) => {
  event.currentTarget.blur();
  const link = document.createElement("a");
  link.href = URL.createObjectURL(new Blob([logLines.join("\n") + "\n"], { type: "text/plain" }));
  link.download = "dampe.log";
  link.click();
  URL.revokeObjectURL(link.href);
});

showTiming();
setStatus(WAITING);
showArea();
log(`Web. Input ${INPUT_LATENCY_MS[settings.platform]} ms + lag ${settings.lag} ms`);
startRead();
if (!document.hasFocus()) {
  setWarning(NOT_FOCUSED);
}
