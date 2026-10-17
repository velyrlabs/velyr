{
  using res = { [Symbol.dispose]() { console.log("disposed"); } };
  console.log("body", typeof res);
}
