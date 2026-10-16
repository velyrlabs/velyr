'use strict';
// node:v8, the subset packages use. Written for this runtime; no Node source is copied.
//
// getHeapStatistics reads the engine heap from `Velyr.metrics()` when the host installed it and a
// JS turn has ended; otherwise (and for fields the engine does not report) it falls back to
// process.memoryUsage() and zeros. When the engine reports no limit, heap_size_limit is the
// machine's total memory. serialize/deserialize use the engine's structured clone, so their bytes
// only round-trip on the same engine (not Node's wire format). Everything else throws
// ERR_VELYR_UNSUPPORTED; setFlagsFromString is a no-op that warns.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');
const { Buffer } = require('buffer');

const unsupported = (what) => errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `v8.${what} is not supported by this runtime`);
const notSupported = (what) => () => { throw unsupported(what); };

function engineHeap() {
  try {
    const metrics = globalThis.Velyr && globalThis.Velyr.metrics;
    return typeof metrics === 'function' ? metrics().heap : null;
  } catch {
    return null;
  }
}

function getHeapStatistics() {
  const heap = engineHeap();
  const memory = process.memoryUsage();
  const total = heap ? heap.totalBytes : memory.heapTotal;
  const used = heap ? heap.usedBytes : memory.heapUsed;
  const limit = (heap && heap.limitBytes) || Math.max(total, require('os').totalmem());
  const external = (heap && heap.externalBytes) || 0;
  return {
    total_heap_size: total,
    total_heap_size_executable: 0,
    total_physical_size: total,
    total_available_size: Math.max(limit - used, 0),
    used_heap_size: used,
    heap_size_limit: limit,
    malloced_memory: external,
    peak_malloced_memory: external,
    does_zap_garbage: 0,
    number_of_native_contexts: 1,
    number_of_detached_contexts: 0,
    total_global_handles_size: 0,
    used_global_handles_size: 0,
    external_memory: external,
  };
}

function serialize(value) {
  return Buffer.from(ops.v8('serialize', value));
}

function deserialize(buffer) {
  if (!ArrayBuffer.isView(buffer)) throw errors.invalidArgType('buffer', 'Buffer, TypedArray, or DataView', buffer);
  return ops.v8('deserialize', buffer);
}

function setFlagsFromString(flags) {
  errors.validateString(flags, 'flags');
  process.emitWarning(`v8.setFlagsFromString(${JSON.stringify(flags)}) is a no-op in this runtime`, 'V8FlagsWarning');
}

class Unsupported {
  constructor() { throw unsupported(new.target.name); }
}

const startupSnapshot = {
  isBuildingSnapshot: () => false,
  addSerializeCallback: notSupported('startupSnapshot.addSerializeCallback'),
  addDeserializeCallback: notSupported('startupSnapshot.addDeserializeCallback'),
  setDeserializeMainFunction: notSupported('startupSnapshot.setDeserializeMainFunction'),
};

module.exports = {
  DefaultDeserializer: class DefaultDeserializer extends Unsupported {},
  DefaultSerializer: class DefaultSerializer extends Unsupported {},
  Deserializer: class Deserializer extends Unsupported {},
  Serializer: class Serializer extends Unsupported {},
  cachedDataVersionTag: () => 0,
  deserialize,
  getHeapCodeStatistics: notSupported('getHeapCodeStatistics'),
  getHeapSnapshot: notSupported('getHeapSnapshot'),
  getHeapSpaceStatistics: notSupported('getHeapSpaceStatistics'),
  getHeapStatistics,
  serialize,
  setFlagsFromString,
  setHeapSnapshotNearHeapLimit: notSupported('setHeapSnapshotNearHeapLimit'),
  startupSnapshot,
  writeHeapSnapshot: notSupported('writeHeapSnapshot'),
};
