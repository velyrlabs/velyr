try {
  new Worker(new URL("./square.js", import.meta.url), { type: "classic" });
} catch (error) {
  console.log(error instanceof TypeError, error.message.includes("classic"));
}
