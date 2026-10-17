// 10000 setTimeout(fn, 1) that fire; exits when the last one has run (cpu_seconds per 10k).
let remaining = 10000;
for (let i = 0; i < 10000; i++) {
  setTimeout(() => {
    remaining--;
  }, 1);
}
