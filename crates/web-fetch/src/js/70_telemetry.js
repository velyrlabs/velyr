
  // ---- Velyr.telemetry -------------------------------------------------------------------

  const telemetryOp = { start: global.op_telemetry_start, end: global.op_telemetry_end };
  delete global.op_telemetry_start;
  delete global.op_telemetry_end;

  // The span a piece of code runs in follows its async context (AsyncLocalStorage's frame, when
  // the runtime has one), so a `fetch` or a nested span made after an `await` finds its parent.
  const SPAN_FRAME_KEY = Symbol("velyr.telemetry.span");
  const asyncContext = () => global[Symbol.for("velyr.asyncContext")];

  function currentSpanId() {
    return asyncContext()?.current()?.get(SPAN_FRAME_KEY) ?? null;
  }

  function runInSpan(id, fn) {
    const context = asyncContext();
    if (id === null || id === undefined || context === undefined) return fn();
    const frame = new Map(context.current());
    frame.set(SPAN_FRAME_KEY, id);
    return context.run(frame, fn, undefined, []);
  }

  function failureMessage(error) {
    return String(error !== null && typeof error === "object" && "message" in error ? error.message : error);
  }

  // Runs `fn` inside an OpenTelemetry span called `name`; the span ends when `fn` returns, or
  // when the promise it returns settles. A no-op wrapper while no exporter is configured.
  function telemetrySpan(name, fn) {
    if (typeof name !== "string") throw new TypeError("Velyr.telemetry.span: name must be a string");
    if (typeof fn !== "function") throw new TypeError("Velyr.telemetry.span: fn must be a function");
    const id = telemetryOp.start(name, currentSpanId());
    if (id === null) return fn();
    let result;
    try {
      result = runInSpan(id, fn);
    } catch (error) {
      telemetryOp.end(id, failureMessage(error));
      throw error;
    }
    if (result === null || typeof result !== "object" || typeof result.then !== "function") {
      telemetryOp.end(id, null);
      return result;
    }
    return Promise.resolve(result).then(
      (value) => { telemetryOp.end(id, null); return value; },
      (error) => { telemetryOp.end(id, failureMessage(error)); throw error; },
    );
  }

  const telemetry = Object.freeze({ span: telemetrySpan });
  Object.defineProperty(global.Velyr, "telemetry", { value: telemetry, writable: false, configurable: true, enumerable: true });
