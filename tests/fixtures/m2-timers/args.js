const id = setTimeout((a, b) => console.log(a, b), 50, "x", "y");
console.log(typeof id);
setTimeout(() => console.log("t1"), 100);
setTimeout(() => console.log("t0"), 0);
setTimeout(() => {
  Promise.resolve().then(() => console.log("micro"));
  console.log("macro");
}, 150);
clearTimeout(undefined);
clearTimeout(123456);
clearInterval("nope");
