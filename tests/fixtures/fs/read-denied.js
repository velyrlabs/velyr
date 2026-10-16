try {
  await Velyr.readTextFile("fixture.txt");
} catch (error) {
  console.log(error.code);
}
