// 10000 pending 30 s timers, then all cleared: peak RSS vs idle-hold.js is the per-timer cost.
const handles = [];
for (let i = 0; i < 10000; i++) handles.push(setTimeout(() => {}, 30000));
for (const handle of handles) clearTimeout(handle);
