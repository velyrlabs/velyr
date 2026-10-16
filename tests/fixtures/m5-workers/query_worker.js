const state = async (variable) =>
  `${variable}:${(await Velyr.permissions.query({ name: "env", variable })).state}`;
self.postMessage(`${await state("HOME")} ${await state("USER")}`);
