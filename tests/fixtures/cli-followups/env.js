console.log(Velyr.env.get("FOO"));
try {
  Velyr.env.get("HOME");
} catch (e) {
  console.log(e.code);
}
