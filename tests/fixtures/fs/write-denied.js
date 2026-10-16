try {
  await Velyr.writeTextFile("should-not-exist.txt", "x");
} catch (error) {
  console.log(error.code);
}
