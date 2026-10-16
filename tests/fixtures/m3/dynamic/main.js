console.log("start");
const ns = await import("./dyn.js");
console.log(ns.value, ns.default);
const again = await import("./dyn.js");
console.log(ns === again);
try {
  await import("./missing.js");
} catch (error) {
  console.log(error.code);
}
