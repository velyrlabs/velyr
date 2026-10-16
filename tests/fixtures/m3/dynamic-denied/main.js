try {
  await import("/etc/hosts");
} catch (error) {
  console.log(error.code);
}
await import("/etc/hosts");
