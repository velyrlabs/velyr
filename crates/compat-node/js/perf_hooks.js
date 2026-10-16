'use strict';
// node:perf_hooks. Written for this runtime; no Node source is copied. `performance`,
// `PerformanceObserver` and the entry classes are the web-core globals; this adds the Node-only
// pieces (monitorEventLoopDelay, createHistogram, constants).
//
// Known limits: monitorEventLoopDelay samples with an unref'd repeating timer; histogram values
// are bucketed to whole microseconds; eventLoopUtilization is derived from process CPU time (see
// sampleLoop); timerify records into `options.histogram` but emits no 'function' PerformanceEntry
// (web-core does not let other code create entries).
const errors = require('internal/errors');
const timers = require('timers');

const need = (name) => {
  const value = globalThis[name];
  if (value === undefined) throw new Error(`velyr compat-node: the host did not install ${name}`);
  return value;
};

const process = require('process');
// The loop monitor works on hosts without the web-core `performance` global too.
const nowMs = () => (globalThis.performance ? globalThis.performance.now() : Number(process.hrtime.bigint()) / 1e6);

const BUCKET_NS = 1000;

/** Count histogram over nanosecond values; backs createHistogram and monitorEventLoopDelay. */
class LoopStats {
  #buckets = new Map();
  #count = 0;
  #sum = 0;
  #sumSquares = 0;
  #min = 9223372036854776000;
  #max = 0;
  #exceeds = 0;

  record(value) {
    if (typeof value === 'bigint') value = Number(value);
    if (typeof value !== 'number') throw errors.invalidArgType('val', ['number', 'bigint'], value);
    if (!Number.isFinite(value) || value < 1) throw errors.outOfRange('val', '>= 1 && <= 9007199254740991', value);
    const bucket = Math.round(value / BUCKET_NS);
    this.#buckets.set(bucket, (this.#buckets.get(bucket) || 0) + 1);
    this.#count++;
    this.#sum += value;
    this.#sumSquares += value * value;
    if (value < this.#min) this.#min = value;
    if (value > this.#max) this.#max = value;
  }

  recordDelta() { throw new Error('histogram.recordDelta() is not supported'); }
  reset() {
    this.#buckets.clear();
    this.#count = this.#sum = this.#sumSquares = this.#exceeds = this.#max = 0;
    this.#min = 9223372036854776000;
  }

  get count() { return this.#count; }
  get countBigInt() { return BigInt(this.#count); }
  get min() { return this.#count === 0 ? 9223372036854776000 : this.#min; }
  get max() { return this.#max; }
  get mean() { return this.#count === 0 ? NaN : this.#sum / this.#count; }
  get stddev() {
    if (this.#count === 0) return NaN;
    const mean = this.mean;
    return Math.sqrt(Math.max(0, this.#sumSquares / this.#count - mean * mean));
  }
  get exceeds() { return this.#exceeds; }

  percentile(percent) {
    if (typeof percent !== 'number') throw errors.invalidArgType('percentile', 'number', percent);
    if (!(percent > 0 && percent <= 100)) throw errors.outOfRange('percentile', '> 0 && <= 100', percent);
    if (this.#count === 0) return 0;
    const target = Math.ceil((percent / 100) * this.#count);
    let seen = 0;
    for (const bucket of [...this.#buckets.keys()].sort((a, b) => a - b)) {
      seen += this.#buckets.get(bucket);
      if (seen >= target) return bucket * BUCKET_NS;
    }
    return this.#max;
  }

  get percentiles() {
    const map = new Map();
    if (this.#count === 0) { map.set(100, 0); return map; }
    let seen = 0;
    let last = -1;
    for (const bucket of [...this.#buckets.keys()].sort((a, b) => a - b)) {
      seen += this.#buckets.get(bucket);
      const percent = (seen / this.#count) * 100;
      map.set(percent, bucket * BUCKET_NS);
      last = percent;
    }
    if (last !== 100) map.set(100, this.#max);
    return map;
  }

  _exceed() { this.#exceeds++; }
}

const EXCEED_NS = 3_600_000_000_000;

class IntervalHistogram extends LoopStats {
  #resolution;
  #timer;
  #last = 0;

  constructor(resolution) {
    super();
    this.#resolution = resolution;
  }

  enable() {
    if (this.#timer !== undefined) return false;
    this.#last = nowMs();
    this.#timer = timers.setInterval(() => {
      const now = nowMs();
      const delay = Math.max(1, (now - this.#last) * 1e6); // ms to ns: the tick interval plus its lag
      this.#last = now;
      if (delay > EXCEED_NS) this._exceed(); else this.record(delay);
    }, this.#resolution).unref(); // a monitor must not keep the process alive
    return true;
  }

  disable() {
    if (this.#timer === undefined) return false;
    timers.clearInterval(this.#timer);
    this.#timer = undefined;
    return true;
  }
}

function monitorEventLoopDelay(options) {
  if (options !== undefined) errors.validateObject(options, 'options');
  const resolution = options && options.resolution !== undefined ? options.resolution : 10;
  if (typeof resolution !== 'number') throw errors.invalidArgType('options.resolution', 'number', resolution);
  if (!Number.isInteger(resolution) || resolution < 1) throw errors.outOfRange('options.resolution', '>= 1', resolution);
  return new IntervalHistogram(resolution);
}

const createHistogram = () => new LoopStats();

const ops = require('internal/ops');

// The loop has no idle counter reachable from JS, so activity is the process CPU time and idle
// is the rest of the wall clock. Other threads (V8 helpers, the blocking pool) count as active.
function sampleLoop() {
  const wall = process.uptime() * 1000;
  const active = Math.min(ops.os('cputime'), wall);
  return { idle: wall - active, active };
}

function eventLoopUtilization(util1, util2) {
  for (const [name, value] of [['utilization1', util1], ['utilization2', util2]]) {
    if (value !== undefined) errors.validateObject(value, name);
  }
  // Like Node: (a, b) is a minus b, (a) is now minus a, () is the cumulative value.
  const later = util2 ? util1 : sampleLoop();
  const earlier = util2 || util1;
  if (!earlier) return { idle: later.idle, active: later.active, utilization: later.active / ((later.idle + later.active) || 1) };
  const idle = Math.max(0, later.idle - earlier.idle);
  const active = Math.max(0, later.active - earlier.active);
  return { idle, active, utilization: active / ((idle + active) || 1) };
}

/** Wraps `fn` so each call's duration (ns) lands in `options.histogram`; no PerformanceEntry is emitted. */
function timerify(fn, options) {
  errors.validateFunction(fn, 'fn');
  if (options !== undefined) errors.validateObject(options, 'options');
  const histogram = options && options.histogram;
  if (histogram !== undefined && typeof histogram.record !== 'function') {
    throw errors.invalidArgType('options.histogram', 'RecordableHistogram', histogram);
  }
  const record = (start) => histogram && histogram.record(Math.max(1, Math.round((nowMs() - start) * 1e6)));
  function timerified(...args) {
    const start = nowMs();
    const result = new.target ? Reflect.construct(fn, args, fn) : Reflect.apply(fn, this, args);
    if (result instanceof Promise) return result.finally(() => record(start));
    record(start);
    return result;
  }
  Object.defineProperties(timerified, {
    length: { value: fn.length, configurable: true },
    name: { value: `timerified ${fn.name}`, configurable: true },
  });
  return timerified;
}

const constants = Object.freeze({
  NODE_PERFORMANCE_GC_MAJOR: 4, NODE_PERFORMANCE_GC_MINOR: 1, NODE_PERFORMANCE_GC_INCREMENTAL: 8,
  NODE_PERFORMANCE_GC_WEAKCB: 16, NODE_PERFORMANCE_GC_FLAGS_NO: 0, NODE_PERFORMANCE_GC_FLAGS_FORCED: 4,
});

const web = ['PerformanceEntry', 'PerformanceMark', 'PerformanceMeasure', 'PerformanceObserver', 'PerformanceObserverEntryList', 'performance'];
const exported = { eventLoopUtilization, monitorEventLoopDelay, createHistogram, timerify, constants };
for (const name of web) {
  Object.defineProperty(exported, name, { enumerable: true, configurable: true, get: () => need(name) });
}
module.exports = exported;
