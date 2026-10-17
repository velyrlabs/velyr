// RT-19: 1000 concurrent reads of a 1 MiB file through the bounded blocking pool.
const path = "benches/fixtures/.fs-readfile-burst.bin";
await Velyr.writeFile(path, new Uint8Array(1 << 20));
const reads = Array.from({ length: 1000 }, () => Velyr.readFile(path));
const sizes = await Promise.all(reads);
if (sizes.some((bytes) => bytes.byteLength !== 1 << 20)) throw new Error("short read");
