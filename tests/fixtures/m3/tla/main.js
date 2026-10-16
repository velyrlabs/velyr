import { greet, message } from "./lib.js";

console.log(greet("velyr"));
console.log(message);
await new Promise((resolve) => setTimeout(resolve, 5));
console.log("after timer");
console.log(import.meta.url.startsWith("file://"));
