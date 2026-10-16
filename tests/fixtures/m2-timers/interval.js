let runs = 0;
const id = setInterval(() => {
  console.log("tick " + ++runs);
  if (runs === 3) clearInterval(id);
}, 1);
