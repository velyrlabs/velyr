import { value } from "./dep.mjs";
// Posted from a timer: the isolate publishes its code cache before the first loop turn.
setTimeout(() => postMessage(value), 20);
