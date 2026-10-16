// Knobs of the large-app fixture, read from the environment when the runtime allows it.
// Every value has a default, so a runtime without `process.env` (or without the permission)
// still runs the same app.
const env = (typeof process !== "undefined" && process.env) || {};

function read(name) {
  try {
    return env[name];
  } catch (e) {
    return undefined;
  }
}

function int(name, fallback) {
  const value = Number(read(name));
  return Number.isFinite(value) && value > 0 ? Math.floor(value) : fallback;
}

export const config = {
  // Live objects kept in the cache for the whole life of the process.
  cacheObjects: int("LARGE_APP_CACHE_OBJECTS", 50000),
  // Items serialized by /json, rows rendered by /template, transforms chained by /transform.
  jsonItems: int("LARGE_APP_JSON_ITEMS", 400),
  templateRows: int("LARGE_APP_TEMPLATE_ROWS", 150),
  transformSteps: int("LARGE_APP_TRANSFORM_STEPS", 24),
};
