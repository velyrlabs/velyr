async function code(fn) {
  try {
    return await fn();
  } catch (error) {
    return error.code ?? `no-code:${error.message}`;
  }
}

console.log(JSON.stringify({
  publicFile: await code(() => Velyr.readTextFile("public.txt")),
  secretFile: await code(() => Velyr.readTextFile("secret/data.txt")),
  secretDir: await code(() => Velyr.readTextFile("./secret/../secret/data.txt")),
}));
