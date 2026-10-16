import { z } from 'zod';

const User = z.object({ name: z.string().min(1), age: z.number().int().positive(), tags: z.array(z.string()).default([]) });
const ok = User.parse({ name: 'a', age: 3 });
if (ok.tags.length !== 0 || ok.age !== 3) throw new Error(JSON.stringify(ok));
const bad = User.safeParse({ name: '', age: -1 });
if (bad.success || bad.error.issues.length !== 2) throw new Error(JSON.stringify(bad));
const union = z.union([z.string(), z.number()]).transform((v) => String(v));
if (union.parse(5) !== '5') throw new Error('transform failed');
console.log('ok');
