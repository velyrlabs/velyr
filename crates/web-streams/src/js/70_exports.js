
  // =====================================================================================
  // Native source hooks (Rust -> stream), for fetch/net. Internal, not part of the web API.
  //
  // Reachable only as globalThis[Symbol.for("velyr.internal.streams")] (non-enumerable).
  //
  // createNativeReadable(resourceId, opRead, opClose?)
  //   opRead(resourceId) -> Promise<Uint8Array | ArrayBuffer | null>; null (or undefined) means EOF.
  //   It is called once per pull, never concurrently. opClose(resourceId) runs on cancel().
  //   Returns a byte ReadableStream, so default and BYOB readers both work.
  // createNativeWritable(resourceId, opWrite, opClose?)
  //   opWrite(resourceId, chunk: Uint8Array) -> Promise<void>, called in order, one at a time.
  //   opClose(resourceId) runs when the stream is closed and when it is aborted.
  // isDisturbed(stream)
  //   Whether the stream was read from, cancelled or piped, even if its reader was released.
  // =====================================================================================

  function createNativeReadable(resourceId, opRead, opClose) {
    return new ReadableStream({
      type: "bytes",
      async pull(controller) {
        const chunk = await opRead(resourceId);
        if (chunk === null || chunk === undefined) {
          controller.close();
          controller.byobRequest?.respond(0);
          return;
        }
        controller.enqueue(chunk instanceof ArrayBuffer ? new Uint8Array(chunk) : chunk);
      },
      cancel() {
        return opClose?.(resourceId);
      },
    });
  }

  function createNativeWritable(resourceId, opWrite, opClose) {
    const close = () => opClose?.(resourceId);
    return new WritableStream({
      write: (chunk) => opWrite(resourceId, chunk),
      close,
      abort: close,
    });
  }

  // isDisturbed(stream) -> boolean: the standard's [[disturbed]] flag of a ReadableStream (it was
  // read from, cancelled or piped), which the public API only exposes as `locked`. Used by
  // Request/Response for `bodyUsed`; false for anything that is not a ReadableStream.
  function isDisturbed(stream) {
    return records.get(stream)?.disturbed === true;
  }

  Object.defineProperty(global, Symbol.for("velyr.internal.streams"), {
    value: Object.freeze({ createNativeReadable, createNativeWritable, isDisturbed }),
    enumerable: false,
    configurable: false,
    writable: false,
  });

  for (const Class of [
    ReadableStream,
    ReadableStreamDefaultReader,
    ReadableStreamBYOBReader,
    ReadableStreamDefaultController,
    ReadableByteStreamController,
    ReadableStreamBYOBRequest,
    WritableStream,
    WritableStreamDefaultWriter,
    WritableStreamDefaultController,
    TransformStream,
    TransformStreamDefaultController,
    CountQueuingStrategy,
    ByteLengthQueuingStrategy,
    TextEncoderStream,
    TextDecoderStream,
    CompressionStream,
    DecompressionStream,
  ]) {
    define(Class.name, Class);
  }
})(globalThis);
